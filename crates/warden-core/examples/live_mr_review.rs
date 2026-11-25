//! End-to-end review of a REAL merge request, with forge writes blocked.
//!
//! Uses the token Warden saved in the Keychain, a temp data dir, the real MCP server, CI runner and
//! Claude Code CLI (subscription). The forge provider is wrapped so every write method (notes,
//! resolve, labels) fails: nothing is posted, whatever the repo's `.warden.yml` says.
//!
//! ```sh
//! cargo run -p warden-core --example live_mr_review -- --base https://gitlab.example.com \
//!     --project group/repo [--iid 41] [--managed] [--local /path/to/clone] [--list]
//! ```
//! `--managed` clones the project into the temp dir over HTTPS with the token (tests the managed
//! clone + authenticated fetch path). Otherwise `--local` (or the clone found by a folder scan).

use std::sync::Arc;

use async_trait::async_trait;
use warden_core::config::AppSettings;
use warden_core::db::Db;
use warden_core::discovery;
use warden_core::forge::{ForgeDiscussion, ForgeProvider, GitlabProvider};
use warden_core::mcp::{McpServer, RunRegistry};
use warden_core::pipeline::Pipeline;
use warden_core::{
    secrets, AppEvent, DiffPosition, EventBus, ForgeKind, MergeRequest, PostingMode, Project,
    RemoteProjectInfo, Result, WardenError,
};

/// Delegates reads; refuses every write.
struct ReadOnly(Arc<dyn ForgeProvider>);

fn blocked(what: &str) -> WardenError {
    println!("    [blocked forge write: {what}]");
    WardenError::Config(format!("forge write blocked in live test: {what}"))
}

#[async_trait]
impl ForgeProvider for ReadOnly {
    async fn whoami(&self) -> Result<String> {
        self.0.whoami().await
    }
    async fn list_projects(&self, s: &str) -> Result<Vec<(String, String)>> {
        self.0.list_projects(s).await
    }
    async fn list_project_details(&self, s: &str) -> Result<Vec<RemoteProjectInfo>> {
        self.0.list_project_details(s).await
    }
    async fn protected_branches(&self, id: &str) -> Result<Vec<String>> {
        self.0.protected_branches(id).await
    }
    fn git_auth_header(&self) -> Option<String> {
        self.0.git_auth_header()
    }
    async fn list_open_mrs(&self, p: &Project) -> Result<Vec<MergeRequest>> {
        self.0.list_open_mrs(p).await
    }
    async fn get_mr(&self, p: &Project, iid: u64) -> Result<MergeRequest> {
        self.0.get_mr(p, iid).await
    }
    async fn list_discussions(&self, p: &Project, iid: u64) -> Result<Vec<ForgeDiscussion>> {
        self.0.list_discussions(p, iid).await
    }
    async fn post_line_note(&self, _: &Project, _: u64, _: &str, _: &DiffPosition) -> Result<String> {
        Err(blocked("post_line_note"))
    }
    async fn post_note(&self, _: &Project, _: u64, _: &str) -> Result<String> {
        Err(blocked("post_note"))
    }
    async fn post_file_note(&self, _: &Project, _: u64, _: &str) -> Result<(String, bool)> {
        Err(blocked("post_file_note"))
    }
    async fn resolve_discussion(&self, _: &Project, _: u64, _: &str) -> Result<()> {
        Err(blocked("resolve_discussion"))
    }
    async fn add_label(&self, _: &Project, _: u64, _: &str) -> Result<()> {
        Err(blocked("add_label"))
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let base = arg(&args, "--base").expect("--base").trim_end_matches('/').to_string();
    let remote_id = arg(&args, "--project").expect("--project");
    let token = secrets::get_token(&secrets::service_key(ForgeKind::Gitlab, &base))?
        .expect("no token saved for this host");
    let real: Arc<dyn ForgeProvider> = Arc::new(GitlabProvider::new(&base, token));
    let provider: Arc<dyn ForgeProvider> = Arc::new(ReadOnly(real));

    let tmp = tempfile::tempdir()?;
    let mut project = Project {
        id: 0,
        forge: ForgeKind::Gitlab,
        base_url: base.clone(),
        remote_id: remote_id.clone(),
        name: remote_id.rsplit('/').next().unwrap_or("p").to_string(),
        local_path: String::new(),
        target_branches: vec![],
        mode: PostingMode::Draft,
        poll_interval_secs: 300,
        enabled: true,
    };

    let mrs = provider.list_open_mrs(&project).await?;
    println!("{} open MRs on {remote_id}", mrs.len());
    for m in mrs.iter().take(10) {
        println!("  !{:<5} {:<60} {} -> {} draft={}", m.iid, m.title, m.source_branch, m.target_branch, m.draft);
    }
    if args.iter().any(|a| a == "--list") {
        return Ok(());
    }
    let iid: u64 = arg(&args, "--iid")
        .map(|s| s.parse().expect("iid"))
        .or_else(|| mrs.iter().find(|m| !m.draft).map(|m| m.iid))
        .expect("no open non-draft MR; pass --iid");

    // Code source.
    if args.iter().any(|a| a == "--managed") {
        let info = provider.list_project_details(&remote_id).await?.into_iter()
            .find(|r| r.remote_id.eq_ignore_ascii_case(&remote_id)).expect("project not found");
        let dest = discovery::managed_clone_dir(tmp.path(), &project);
        let t = std::time::Instant::now();
        discovery::clone_managed(&dest, info.clone_url.as_deref().expect("clone url"), &base, provider.git_auth_header().as_deref()).await?;
        println!("managed clone OK in {:.1}s -> {}", t.elapsed().as_secs_f64(), dest.display());
        project.local_path = dest.to_string_lossy().into();
    } else if let Some(l) = arg(&args, "--local") {
        project.local_path = l;
    } else {
        let (scanned, _) = discovery::scan_repos(&discovery::default_roots());
        let hit = scanned.iter().find(|r| r.remotes.iter().any(|(_, u)| {
            discovery::remote_id_for(u, &base).is_some_and(|id| id.eq_ignore_ascii_case(&remote_id))
        })).expect("no local clone found; pass --local or --managed");
        project.local_path = hit.path.to_string_lossy().into();
        println!("using local clone {}", project.local_path);
    }

    let db = Db::open(&tmp.path().join("warden.db")).await?;
    let settings = AppSettings {
        data_dir: tmp.path().to_string_lossy().into(),
        worktree_root: tmp.path().join("worktrees").to_string_lossy().into(),
        max_turns: arg(&args, "--max-turns").and_then(|t| t.parse().ok()).unwrap_or(20),
        ..Default::default()
    };
    db.save_settings(&settings).await?;
    project.id = db.upsert_project(&project).await?;
    let mut mr = provider.get_mr(&project, iid).await?;
    mr.project_id = project.id;
    let mr_id = db.upsert_mr(&mr).await?;
    println!("reviewing !{} {} ({} -> {})", mr.iid, mr.title, mr.source_branch, mr.target_branch);

    let events = EventBus::new();
    let registry = Arc::new(RunRegistry::new());
    let mcp = McpServer::start(0, registry.clone()).await?;
    let ci = Arc::new(Pipeline::ci_runner(&db, events.clone(), tmp.path().join("logs")));
    let pipeline = Pipeline::new(db.clone(), events.clone(), registry, ci, mcp.url());

    let mut rx = events.subscribe();
    tokio::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            match ev {
                AppEvent::RunUpdated { run } => println!("[run] status={}", run.status),
                AppEvent::RunProgress { message, turn, .. } => println!("[t{turn}] {message}"),
                AppEvent::CiJobUpdated { job } => println!("    [ci] {} -> {}", job.name, job.status),
                AppEvent::Notice { level, message } => println!("[notice:{level}] {message}"),
                _ => {}
            }
        }
    });

    let started = std::time::Instant::now();
    let run = pipeline.queue_mr_review(mr_id, None).await?;
    let run = pipeline.execute_run(run.id, Some(provider.clone())).await?;
    println!("\n==== RESULT: status={} turns={} wall={:.0}s verdict={:?}", run.status, run.turns, started.elapsed().as_secs_f64(), run.verdict);
    if let Some(e) = &run.error {
        println!("error={e}");
    }
    println!("summary={:?}", run.summary);
    for f in db.list_findings(run.id).await? {
        println!("  [{}] {} {} {}:{} rule={:?} conf={:.2} {}{}", f.source, f.status, f.severity, f.file, f.line, f.rule_id, f.confidence, f.title,
            f.guardrail_reason.as_deref().map(|r| format!("  <- {r}")).unwrap_or_default());
    }
    for j in db.list_ci_jobs(run.id).await? {
        println!("  ci {} {} {}ms", j.name, j.status, j.duration_ms);
    }
    Ok(())
}
