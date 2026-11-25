//! Integration: the whole pipeline with the real MCP server, CI runner, checks and guardrails, a fake
//! `claude` (examples/fake_claude.py, which speaks real MCP over HTTP) and a wiremock GitLab.
//! No real forge and no real model are contacted.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use wiremock::matchers::{method, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use warden_core::config::AppSettings;
use warden_core::db::Db;
use warden_core::forge::provider_for;
use warden_core::mcp::{McpServer, RunRegistry};
use warden_core::pipeline::{ExecOutcome, Pipeline};
use warden_core::{
    EventBus, FindingStatus, ForgeKind, MergeRequest, PostingMode, Project, RunStatus,
};

const BASE_PY: &str = "\"\"\"Helpers that link imported rows back to their Data Import.\"\"\"\n\nimport json\n\nimport frappe\n# legacy linker removed\n\n\ndef get_import(import_name):\n    return frappe.get_doc(\"ECRM Data Import\", import_name)\n";

const HEAD1_PY: &str = "\"\"\"Helpers that link imported rows back to their Data Import.\"\"\"\n\nimport json\n\nimport frappe\n\n@frappe.whitelist()\ndef link_import_rows(import_name, rows):\n    rows = json.loads(rows)\n    for r in rows:\n        frappe.db.sql(f\"update `tabECRM Lead` set import_ref='{import_name}' where name='{r}'\")\n    frappe.db.commit()\n    return len(rows)\n\n\ndef get_import(import_name):\n    return frappe.get_doc(\"ECRM Data Import\", import_name)\n";

const HEAD2_PY: &str = "\"\"\"Helpers that link imported rows back to their Data Import.\"\"\"\n\nimport json\n\nimport frappe\n\n@frappe.whitelist()\ndef link_import_rows(import_name, rows):\n    rows = json.loads(rows)\n    for r in rows:\n        frappe.db.set_value(\"ECRM Lead\", r, \"import_ref\", import_name)\n    return len(rows)\n\n\ndef get_import(import_name):\n    return frappe.get_doc(\"ECRM Data Import\", import_name)\n";

const WARDEN_YML: &str = "version: 1\nci:\n  recipes:\n    - name: lint\n      run: \"python3 -m py_compile {changed_py}\"\n      timeout: \"1m\"\n";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit_file(repo: &Path, rel: &str, content: &str, msg: &str) -> String {
    let p = repo.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, content).unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", msg]);
    git(repo, &["rev-parse", "HEAD"])
}

struct Fixture {
    _tmp: tempfile::TempDir,
    src: PathBuf,
    remote: PathBuf,
    clone: PathBuf,
    base: String,
    head1: String,
    wt_root: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    git(&src, &["config", "user.email", "t@example.invalid"]);
    git(&src, &["config", "user.name", "Test"]);
    std::fs::write(src.join(".warden.yml"), WARDEN_YML).unwrap();
    let base = commit_file(&src, "app/api/import_links.py", BASE_PY, "base");
    git(&src, &["checkout", "-q", "-b", "feature"]);
    let head1 = commit_file(&src, "app/api/import_links.py", HEAD1_PY, "link rows");
    let remote = tmp.path().join("remote.git");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            "--bare",
            src.to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );
    git(
        &src,
        &[
            "push",
            "-q",
            remote.to_str().unwrap(),
            "+feature:refs/merge-requests/1/head",
        ],
    );
    let clone = tmp.path().join("clone");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            remote.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let wt_root = tmp.path().join("worktrees");
    Fixture {
        _tmp: tmp,
        src,
        remote,
        clone,
        base,
        head1,
        wt_root,
    }
}

fn fake_claude() -> String {
    format!("{}/examples/fake_claude.py", env!("CARGO_MANIFEST_DIR"))
}

async fn pipeline(db: &Db, wt_root: &Path) -> (Pipeline, McpServer) {
    let settings = AppSettings {
        claude_cli_path: Some(fake_claude()),
        worktree_root: wt_root.to_string_lossy().into(),
        data_dir: wt_root.to_string_lossy().into(),
        max_turns: 12,
        review_timeout_secs: 120,
        ..Default::default()
    };
    db.save_settings(&settings).await.unwrap();
    let events = EventBus::new();
    let registry = Arc::new(RunRegistry::new());
    let mcp = McpServer::start(0, registry.clone()).await.unwrap();
    let ci = Arc::new(Pipeline::ci_runner(
        db,
        events.clone(),
        wt_root.join("logs"),
    ));
    let p = Pipeline::new(db.clone(), events, registry, ci, mcp.url());
    (p, mcp)
}

#[tokio::test]
async fn local_review_stores_rule_and_claude_findings() {
    let fx = fixture();
    let db = Db::open_memory().await.unwrap();
    let (p, mcp) = pipeline(&db, &fx.wt_root).await;

    let run = p
        .review_local(fx.src.to_str().unwrap(), "main", "feature")
        .await
        .unwrap();
    assert_eq!(run.status, RunStatus::AwaitingApproval, "{:?}", run.error);
    assert_eq!(run.verdict.as_deref(), Some("request_changes"));
    assert!(run.turns >= 5);

    let findings = db.list_findings(run.id).await.unwrap();
    let rules: Vec<_> = findings
        .iter()
        .filter(|f| f.source == "rule")
        .map(|f| f.rule_id.clone().unwrap())
        .collect();
    assert!(rules.contains(&"FR-002".to_string()), "{findings:#?}");
    assert!(rules.contains(&"FR-007".to_string()), "{findings:#?}");
    let claude: Vec<_> = findings.iter().filter(|f| f.source == "claude").collect();
    assert_eq!(claude.len(), 1, "{findings:#?}");
    assert_eq!(claude[0].rule_id.as_deref(), Some("FR-001"));
    assert_eq!(claude[0].status, FindingStatus::Draft);
    assert!(claude[0].position.is_some());

    let jobs = db.list_ci_jobs(run.id).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].name, "lint");
    assert_eq!(jobs[0].status, warden_core::CiStatus::Passed);
    assert!(jobs[0].id > 0);

    // Local runs are reported as local and never touch the forge.
    let mr = db.get_mr(run.mr_id).await.unwrap();
    assert_eq!(mr.iid, 0);
    let audit = db.list_audit(Some(run.id), 100).await.unwrap();
    assert!(audit.iter().any(|a| a.kind == "guardrail"));
    assert!(audit.iter().all(|a| a.kind != "post"));
    mcp.stop().await;
}

#[tokio::test]
async fn auto_mode_posts_then_incremental_review_resolves_fixed_threads() {
    let fx = fixture();
    let gl = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex(r"/merge_requests/1/discussions$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
        .mount(&gl)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/merge_requests/1/discussions$"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": "disc-1"})))
        .mount(&gl)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/merge_requests/1/notes$"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": 77})))
        .mount(&gl)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(r"/merge_requests/1$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&gl)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex(r"/merge_requests/1/discussions/disc-1$"))
        .and(query_param("resolved", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1..)
        .mount(&gl)
        .await;

    let db = Db::open_memory().await.unwrap();
    let (p, mcp) = pipeline(&db, &fx.wt_root).await;
    let mut project = Project {
        id: 0,
        forge: ForgeKind::Gitlab,
        base_url: gl.uri(),
        remote_id: "group/app".into(),
        name: "app".into(),
        local_path: fx.clone.to_string_lossy().into(),
        target_branches: vec![],
        mode: PostingMode::Auto,
        poll_interval_secs: 120,
        enabled: true,
    };
    project.id = db.upsert_project(&project).await.unwrap();
    let mut mr = MergeRequest {
        project_id: project.id,
        iid: 1,
        title: "Link import rows".into(),
        author: "dev".into(),
        source_branch: "feature".into(),
        target_branch: "main".into(),
        head_sha: fx.head1.clone(),
        base_sha: fx.base.clone(),
        start_sha: fx.base.clone(),
        state: "opened".into(),
        ..Default::default()
    };
    mr.id = db.upsert_mr(&mr).await.unwrap();
    let provider = provider_for(&project, "test-token".into());

    // ---- run 1: everything kept is auto-posted ----
    let run = p.queue_mr_review(mr.id, None).await.unwrap();
    let out = p
        .execute_queued(run.id, Some(provider.clone()))
        .await
        .unwrap();
    let ExecOutcome::Finished(run) = out else {
        panic!("deferred")
    };
    assert_eq!(run.status, RunStatus::Posted, "{:?}", run.error);
    let findings = db.list_findings(run.id).await.unwrap();
    let posted: Vec<_> = findings
        .iter()
        .filter(|f| f.status == FindingStatus::Posted)
        .collect();
    assert_eq!(posted.len(), 3, "{findings:#?}");
    assert!(posted
        .iter()
        .all(|f| f.forge_note_id.as_deref() == Some("disc-1")));
    let mr_now = db.get_mr(mr.id).await.unwrap();
    assert_eq!(mr_now.last_reviewed_sha.as_deref(), Some(fx.head1.as_str()));

    // Claude never got the token: the mcp-config only carries the run header.
    let reqs = gl.received_requests().await.unwrap();
    assert!(reqs.iter().all(
        |r| r.headers.get("PRIVATE-TOKEN").map(|v| v.to_str().unwrap()) == Some("test-token")
    ));
    assert!(
        reqs.iter().any(|r| r.url.path().ends_with("/notes")),
        "summary note"
    );

    // ---- run 2: the author fixes the SQL and drops the commit ----
    let head2 = commit_file(
        &fx.src,
        "app/api/import_links.py",
        HEAD2_PY,
        "use set_value",
    );
    git(
        &fx.src,
        &[
            "push",
            "-q",
            "-f",
            fx.remote.to_str().unwrap(),
            "+feature:refs/merge-requests/1/head",
        ],
    );
    let mut mr2 = db.get_mr(mr.id).await.unwrap();
    mr2.head_sha = head2.clone();
    db.upsert_mr(&mr2).await.unwrap();
    let run2 = p
        .queue_mr_review(mr.id, Some(fx.head1.clone()))
        .await
        .unwrap();
    assert_eq!(run2.since_sha.as_deref(), Some(fx.head1.as_str()));
    let ExecOutcome::Finished(run2) = p.execute_queued(run2.id, Some(provider)).await.unwrap()
    else {
        panic!("deferred")
    };
    assert!(run2.error.is_none(), "{:?}", run2.error);
    let f2 = db.list_findings(run2.id).await.unwrap();
    // Claude repeats FR-001 -> rejected as a duplicate of the posted thread.
    assert!(f2
        .iter()
        .any(|f| f.rule_id.as_deref() == Some("FR-001") && f.status == FindingStatus::Rejected));
    // The fixed rule findings from run 1 are resolved; FR-001 stays open.
    let first = db.list_findings(run.id).await.unwrap();
    let status_of = |rule: &str| {
        first
            .iter()
            .find(|f| f.rule_id.as_deref() == Some(rule))
            .unwrap()
            .status
    };
    assert_eq!(status_of("FR-002"), FindingStatus::Resolved);
    assert_eq!(status_of("FR-007"), FindingStatus::Resolved);
    assert_eq!(status_of("FR-001"), FindingStatus::Posted);
    mcp.stop().await;
}

#[tokio::test]
async fn usage_gate_defers_and_cancel_marks_queued_runs() {
    let fx = fixture();
    let db = Db::open_memory().await.unwrap();
    let (p, mcp) = pipeline(&db, &fx.wt_root).await;
    let mut s = db.get_settings().await.unwrap();
    s.max_reviews_per_hour = 1;
    db.save_settings(&s).await.unwrap();

    let first = p
        .review_local(fx.src.to_str().unwrap(), "main", "feature")
        .await
        .unwrap();
    assert_eq!(first.status, RunStatus::AwaitingApproval);
    let queued = p
        .queue_local_review(fx.src.to_str().unwrap(), "main", "feature")
        .await
        .unwrap();
    assert_eq!(
        p.execute_queued(queued.id, None).await.unwrap(),
        ExecOutcome::Deferred
    );
    assert_eq!(
        db.get_run(queued.id).await.unwrap().status,
        RunStatus::Queued
    );
    p.cancel(queued.id).await.unwrap();
    assert_eq!(
        db.get_run(queued.id).await.unwrap().status,
        RunStatus::Cancelled
    );
    let usage = p.usage().await.unwrap();
    assert_eq!(usage.reviews_last_hour, 1);
    mcp.stop().await;
}
