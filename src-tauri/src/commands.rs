// OWNER: integration
//! Tauri commands (SPEC "Tauri commands"). Names and parameters mirror `ui/src/lib/api.ts`; Tauri 2
//! maps the JS camelCase argument keys (`mrId`, `baseUrl`, ...) to these snake_case parameters.
//!
//! Tokens: `set_forge_token` validates the token with `whoami`, stores it in the keychain and returns
//! only the username. No command ever returns a token.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::Value;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};

use warden_core::checks::{self, redact_secrets};
use warden_core::config::RuleConfig;
use warden_core::config::{default_rules, merge_overrides, AppSettings, WardenConfig};
use warden_core::discovery;
use warden_core::forge::provider_for_host;
use warden_core::pipeline::{is_local_project, DISABLED_RULES_KEY, LOCAL_BASE_URL};
use warden_core::poster::{PostReport, Poster};
use warden_core::secrets;
use warden_core::stats;
use warden_core::worktree::expand_tilde;
use warden_core::RuleTestResult;
use warden_core::{
    AddProjectItem, AddProjectResult, AppEvent, AuditEvent, CiJob, CiStatus, CiSummary, Discovery,
    EngineStatus, FindingCounts, FindingStatus, ForgeKind, ForgeStatus, MergeRequest, MrRow,
    OpenMrResult, PendingFinding, PostingMode, Project, RemoteProjectInfo, Result, ReviewRun,
    RuleStat, RunDetail, RunRow, SimulatedFinding, Stats, ToolStatus, UsageStats, WardenError,
};

use crate::state::{norm_base, ForgeCheck, ForgeHost, Shared};
use crate::tray;

type St<'a> = State<'a, Shared>;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

// ---------------------------------------------------------------------------------------------
// engine / tools / usage
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn get_engine_status(state: St<'_>) -> Result<EngineStatus> {
    Ok(state.engine().status().await)
}

#[tauri::command]
pub async fn check_engine_deep(state: St<'_>) -> Result<EngineStatus> {
    Ok(state.engine().status_deep().await)
}

#[tauri::command]
pub async fn detect_tools() -> Result<Vec<ToolStatus>> {
    Ok(tokio::task::spawn_blocking(checks::detect_tools).await?)
}

#[tauri::command]
pub async fn get_usage(state: St<'_>) -> Result<UsageStats> {
    state.pipeline.usage().await
}

// ---------------------------------------------------------------------------------------------
// settings
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn get_settings(state: St<'_>) -> Result<AppSettings> {
    state.db.get_settings().await
}

#[tauri::command]
pub async fn save_settings(app: AppHandle, state: St<'_>, settings: AppSettings) -> Result<()> {
    let old = state.db.get_settings().await?;
    let mut s = settings;
    // The data dir is fixed by the app; the MCP port in use only changes on restart.
    s.data_dir = state.data_dir.to_string_lossy().to_string();
    if s.claude_cli_path
        .as_deref()
        .is_some_and(|p| p.trim().is_empty())
    {
        s.claude_cli_path = None;
    }
    if s.model.trim().is_empty() {
        s.model = AppSettings::default().model;
    }
    state.db.save_settings(&s).await?;
    state.set_settings_cache(&s);
    if s.mcp_port != old.mcp_port {
        state.notice(
            "info",
            "MCP port change takes effect after restarting Warden",
        );
    }
    if s.posting_paused != old.posting_paused {
        tray::set_paused_label(&app, s.posting_paused);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// forges
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn set_forge_token(
    state: St<'_>,
    forge: ForgeKind,
    base_url: String,
    token: String,
) -> Result<String> {
    let base = norm_base(&base_url);
    if base.is_empty() || base == LOCAL_BASE_URL.trim_end_matches('/') {
        return Err(WardenError::Config("base URL is required".into()));
    }
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(WardenError::Config("token is empty".into()));
    }
    // Validate before storing; the token itself is never logged or returned.
    let user = provider_for_host(forge, &base, token.clone())
        .whoami()
        .await?;
    let key = secrets::service_key(forge, &base);
    secrets::set_token(&key, &token)?;
    state
        .remember_forge(ForgeHost {
            forge,
            base_url: base.clone(),
        })
        .await?;
    if let Ok(mut c) = state.forge_checks.lock() {
        c.insert(
            key,
            ForgeCheck {
                at: Instant::now(),
                user: Some(user.clone()),
                ok: true,
            },
        );
    }
    let _ = state
        .db
        .audit(
            None,
            "engine",
            serde_json::json!({"event": "forge_token_saved", "forge": forge.as_str(), "base_url": base, "user": user}),
        )
        .await;
    state.request_poll(None);
    Ok(user)
}

#[tauri::command]
pub async fn forge_status(state: St<'_>) -> Result<Vec<ForgeStatus>> {
    let mut out = Vec::new();
    for host in state.known_forges().await? {
        let key = secrets::service_key(host.forge, &host.base_url);
        let cached = state
            .forge_checks
            .lock()
            .ok()
            .and_then(|c| c.get(&key).cloned())
            .filter(|c| c.at.elapsed() < crate::state::FORGE_STATUS_TTL);
        let check = match cached {
            Some(c) => c,
            None => {
                let check = match secrets::get_token(&key) {
                    Ok(Some(token)) => {
                        match provider_for_host(host.forge, &host.base_url, token)
                            .whoami()
                            .await
                        {
                            Ok(u) => ForgeCheck {
                                at: Instant::now(),
                                user: Some(u),
                                ok: true,
                            },
                            Err(e) => {
                                tracing::warn!("whoami for {} failed: {e}", host.base_url);
                                ForgeCheck {
                                    at: Instant::now(),
                                    user: None,
                                    ok: false,
                                }
                            }
                        }
                    }
                    _ => ForgeCheck {
                        at: Instant::now(),
                        user: None,
                        ok: false,
                    },
                };
                if let Ok(mut c) = state.forge_checks.lock() {
                    c.insert(key, check.clone());
                }
                check
            }
        };
        out.push(ForgeStatus {
            forge: host.forge,
            base_url: host.base_url,
            user: check.user,
            ok: check.ok,
        });
    }
    Ok(out)
}

#[tauri::command]
pub async fn search_remote_projects(
    forge: ForgeKind,
    base_url: String,
    query: String,
) -> Result<Vec<(String, String)>> {
    let base = norm_base(&base_url);
    let token = secrets::get_token(&secrets::service_key(forge, &base))?
        .ok_or_else(|| WardenError::Config(format!("no {forge} token saved for {base}")))?;
    provider_for_host(forge, &base, token)
        .list_projects(&query)
        .await
}

// ---------------------------------------------------------------------------------------------
// project discovery (add-project screen)
// ---------------------------------------------------------------------------------------------

fn forge_token(forge: ForgeKind, base: &str) -> Result<String> {
    secrets::get_token(&secrets::service_key(forge, base))?.ok_or_else(|| {
        WardenError::Config(format!(
            "no {forge} token saved for {base} (add it under Connections)"
        ))
    })
}

/// Remote projects the token owner can access, merged with git clones found under `roots`
/// (empty = default folders under $HOME).
#[tauri::command]
pub async fn discover_projects(
    state: St<'_>,
    forge: ForgeKind,
    base_url: String,
    roots: Vec<String>,
) -> Result<Discovery> {
    let base = norm_base(&base_url);
    let token = forge_token(forge, &base)?;
    let provider = provider_for_host(forge, &base, token);
    let roots: Vec<PathBuf> = if roots.iter().all(|r| r.trim().is_empty()) {
        discovery::default_roots()
    } else {
        roots
            .iter()
            .filter(|r| !r.trim().is_empty())
            .map(|r| expand_tilde(Path::new(r.trim())))
            .collect()
    };
    let scan_roots = roots.clone();
    let (remote, scanned) = tokio::join!(
        provider.list_project_details(""),
        tokio::task::spawn_blocking(move || discovery::scan_repos(&scan_roots)),
    );
    let existing = state.db.list_projects().await?;
    Ok(discovery::discovery(
        &base, remote?, &roots, scanned?, &existing,
    ))
}

/// Adds several projects at once. Items without a local path get a Warden-managed clone; items
/// without target branches get the default branch plus protected branches from the forge.
#[tauri::command]
pub async fn add_projects(
    state: St<'_>,
    forge: ForgeKind,
    base_url: String,
    items: Vec<AddProjectItem>,
    mode: PostingMode,
    poll_interval_secs: u32,
) -> Result<Vec<AddProjectResult>> {
    let base = norm_base(&base_url);
    if base == LOCAL_BASE_URL.trim_end_matches('/') {
        return Err(WardenError::Config("reserved base URL".into()));
    }
    let token = forge_token(forge, &base)?;
    let provider = provider_for_host(forge, &base, token);
    // One listing for default branches and clone URLs.
    let remote: HashMap<String, RemoteProjectInfo> = provider
        .list_project_details("")
        .await?
        .into_iter()
        .map(|r| (r.remote_id.to_ascii_lowercase(), r))
        .collect();
    let mut out = Vec::new();
    for item in items {
        let res = add_one(
            &state,
            forge,
            &base,
            &provider,
            &remote,
            &item,
            mode,
            poll_interval_secs,
        )
        .await;
        out.push(match res {
            Ok(id) => AddProjectResult {
                remote_id: item.remote_id.clone(),
                project_id: Some(id),
                error: None,
            },
            Err(e) => AddProjectResult {
                remote_id: item.remote_id.clone(),
                project_id: None,
                error: Some(e.to_string()),
            },
        });
    }
    state.request_poll(None);
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
async fn add_one(
    state: &Shared,
    forge: ForgeKind,
    base: &str,
    provider: &std::sync::Arc<dyn warden_core::forge::ForgeProvider>,
    remote: &HashMap<String, RemoteProjectInfo>,
    item: &AddProjectItem,
    mode: PostingMode,
    poll: u32,
) -> Result<i64> {
    let remote_id = item.remote_id.trim().to_string();
    if remote_id.is_empty() {
        return Err(WardenError::Config("remote project is required".into()));
    }
    let info = remote.get(&remote_id.to_ascii_lowercase());
    let target_branches = match &item.target_branches {
        Some(b) if !b.is_empty() => b.clone(),
        _ => {
            let protected = provider
                .protected_branches(&remote_id)
                .await
                .unwrap_or_default();
            discovery::target_branches(info.and_then(|i| i.default_branch.as_deref()), &protected)
        }
    };
    let name = if item.name.trim().is_empty() {
        remote_id
            .rsplit('/')
            .next()
            .unwrap_or(&remote_id)
            .to_string()
    } else {
        item.name.trim().to_string()
    };
    let mut project = Project {
        id: 0,
        forge,
        base_url: base.to_string(),
        remote_id: remote_id.clone(),
        name,
        local_path: String::new(),
        target_branches,
        mode,
        poll_interval_secs: poll.max(30),
        enabled: true,
    };
    project.local_path = match item.local_path.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => {
            let path = expand_tilde(Path::new(p));
            if !path.join(".git").exists() {
                return Err(WardenError::Config(format!("{p} is not a git repository")));
            }
            p.to_string()
        }
        _ => {
            let clone_url = info
                .and_then(|i| i.clone_url.clone())
                .ok_or_else(|| WardenError::Config(format!("no clone URL for {remote_id}")))?;
            let dest = discovery::managed_clone_dir(&state.data_dir, &project);
            state.notice("info", format!("Cloning {remote_id}…"));
            discovery::clone_managed(
                &dest,
                &clone_url,
                base,
                provider.git_auth_header().as_deref(),
            )
            .await?;
            dest.to_string_lossy().to_string()
        }
    };
    let id = state.db.upsert_project(&project).await?;
    state.events.emit(AppEvent::MrsUpdated { project_id: id });
    Ok(id)
}

/// Native folder picker (macOS `choose folder`). `None` when the user cancels.
#[tauri::command]
pub async fn pick_folder() -> Result<Option<String>> {
    let out = tokio::process::Command::new("osascript")
        .args([
            "-e",
            "POSIX path of (choose folder with prompt \"Choose a folder with your git repos\")",
        ])
        .output()
        .await?;
    if !out.status.success() {
        return Ok(None); // cancelled
    }
    let p = String::from_utf8_lossy(&out.stdout)
        .trim()
        .trim_end_matches('/')
        .to_string();
    Ok((!p.is_empty()).then_some(p))
}

/// Pasted MR/PR link: adds the project if needed (local clone if one is found, else a managed
/// clone), stores the MR and queues a review of it.
#[tauri::command]
pub async fn open_mr_link(state: St<'_>, url: String) -> Result<OpenMrResult> {
    let known: Vec<(ForgeKind, String)> = state
        .known_forges()
        .await?
        .into_iter()
        .map(|h| (h.forge, h.base_url))
        .collect();
    let link = discovery::parse_mr_link(&url, &known)
        .ok_or_else(|| WardenError::Config("not a merge request or pull request link".into()))?;
    let base = norm_base(&link.base_url);
    let token = forge_token(link.forge, &base)?;
    let provider = provider_for_host(link.forge, &base, token);

    let existing = state.db.list_projects().await?.into_iter().find(|p| {
        p.forge == link.forge
            && norm_base(&p.base_url) == base
            && p.remote_id.eq_ignore_ascii_case(&link.remote_id)
    });
    let (project, added) = match existing {
        Some(p) => (p, false),
        None => {
            let remote: HashMap<String, RemoteProjectInfo> = provider
                .list_project_details(&link.remote_id)
                .await?
                .into_iter()
                .map(|r| (r.remote_id.to_ascii_lowercase(), r))
                .collect();
            let roots = discovery::default_roots();
            let (scanned, _) =
                tokio::task::spawn_blocking(move || discovery::scan_repos(&roots)).await?;
            let local = scanned.iter().find(|r| {
                r.remotes.iter().any(|(_, u)| {
                    discovery::remote_id_for(u, &base)
                        .is_some_and(|id| id.eq_ignore_ascii_case(&link.remote_id))
                })
            });
            let item = AddProjectItem {
                remote_id: remote
                    .get(&link.remote_id.to_ascii_lowercase())
                    .map(|r| r.remote_id.clone())
                    .unwrap_or_else(|| link.remote_id.clone()),
                name: String::new(),
                local_path: local.map(|r| r.path.to_string_lossy().to_string()),
                target_branches: None,
            };
            let id = add_one(
                &state,
                link.forge,
                &base,
                &provider,
                &remote,
                &item,
                PostingMode::Draft,
                300,
            )
            .await?;
            (state.db.get_project(id).await?, true)
        }
    };
    let mut mr = provider.get_mr(&project, link.iid).await?;
    mr.project_id = project.id;
    let mr_id = state.db.upsert_mr(&mr).await?;
    let run = state.pipeline.queue_mr_review(mr_id, None).await?;
    state.enqueue(run.id);
    state.events.emit(AppEvent::MrsUpdated {
        project_id: project.id,
    });
    Ok(OpenMrResult {
        project_id: project.id,
        mr_id,
        run_id: run.id,
        added_project: added,
    })
}

// ---------------------------------------------------------------------------------------------
// projects
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn list_projects(state: St<'_>) -> Result<Vec<Project>> {
    Ok(state
        .db
        .list_projects()
        .await?
        .into_iter()
        .filter(|p| !is_local_project(p))
        .collect())
}

#[tauri::command]
pub async fn save_project(state: St<'_>, project: Project) -> Result<i64> {
    let mut p = project;
    p.base_url = norm_base(&p.base_url);
    p.name = p.name.trim().to_string();
    p.remote_id = p.remote_id.trim().to_string();
    if p.base_url.is_empty() || p.remote_id.is_empty() {
        return Err(WardenError::Config(
            "base URL and remote project are required".into(),
        ));
    }
    if p.base_url == LOCAL_BASE_URL.trim_end_matches('/') || p.base_url == LOCAL_BASE_URL {
        return Err(WardenError::Config("reserved base URL".into()));
    }
    if p.name.is_empty() {
        p.name = p
            .remote_id
            .rsplit('/')
            .next()
            .unwrap_or(&p.remote_id)
            .to_string();
    }
    let id = state.db.upsert_project(&p).await?;
    state.events.emit(AppEvent::MrsUpdated { project_id: id });
    if p.enabled {
        state.request_poll(Some(id));
    }
    Ok(id)
}

#[tauri::command]
pub async fn delete_project(state: St<'_>, id: i64) -> Result<()> {
    let project = state.db.get_project(id).await?;
    let wt = state.worktrees();
    for mr in state.db.list_mrs(Some(id)).await? {
        let _ = wt.remove(&project, mr.iid).await;
    }
    state.db.delete_project(id).await?;
    state.events.emit(AppEvent::MrsUpdated { project_id: id });
    Ok(())
}

#[tauri::command]
pub async fn get_project_config(state: St<'_>, id: i64) -> Result<Value> {
    let project = state.db.get_project(id).await?;
    let (cfg, disabled) = state
        .pipeline
        .effective_config(&project, None, false)
        .await?;
    let mut v = serde_json::to_value(&cfg)?;
    if let Some(o) = v.as_object_mut() {
        o.insert(DISABLED_RULES_KEY.into(), serde_json::to_value(disabled)?);
    }
    Ok(v)
}

/// The project's stored local overrides (not merged), so the UI can change single fields.
#[tauri::command]
pub async fn get_project_overrides(state: St<'_>, id: i64) -> Result<Value> {
    state.db.get_project(id).await?;
    state.db.get_project_overrides(id).await
}

#[tauri::command]
pub async fn save_project_overrides(state: St<'_>, id: i64, overrides: Value) -> Result<()> {
    state.db.get_project(id).await?;
    if !overrides.is_object() && !overrides.is_null() {
        return Err(WardenError::Config(
            "overrides must be a JSON object".into(),
        ));
    }
    // Validate: the overrides must merge cleanly over the defaults.
    let mut probe = overrides.clone();
    if let Some(o) = probe.as_object_mut() {
        if let Some(d) = o.remove(DISABLED_RULES_KEY) {
            serde_json::from_value::<Vec<String>>(d)
                .map_err(|e| WardenError::Config(format!("disabled_rules: {e}")))?;
        }
    }
    merge_overrides(WardenConfig::default(), &probe)?;
    let stored = if overrides.is_null() {
        serde_json::json!({})
    } else {
        overrides
    };
    state.db.save_project_overrides(id, &stored).await?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// MRs and runs
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn poll_now(state: St<'_>) -> Result<()> {
    state.request_poll(None);
    Ok(())
}

#[tauri::command]
pub async fn list_mrs(state: St<'_>, project_id: Option<i64>) -> Result<Vec<MrRow>> {
    let projects: HashMap<i64, Project> = state
        .db
        .list_projects()
        .await?
        .into_iter()
        .map(|p| (p.id, p))
        .collect();
    let mut rows = Vec::new();
    for mr in state.db.list_mrs(project_id).await? {
        let Some(project) = projects.get(&mr.project_id) else {
            continue;
        };
        let local = is_local_project(project);
        if !local && mr.state != "opened" {
            continue;
        }
        let latest_run = state.db.latest_run_for_mr(mr.id).await?;
        let mut counts = FindingCounts::default();
        let mut ci = CiSummary::default();
        if let Some(run) = &latest_run {
            for f in state.db.list_findings(run.id).await? {
                match f.status {
                    FindingStatus::Draft | FindingStatus::Approved => counts.draft += 1,
                    FindingStatus::Posted | FindingStatus::Resolved => counts.posted += 1,
                    FindingStatus::Rejected => counts.rejected += 1,
                    FindingStatus::Dismissed => {}
                }
            }
            for j in state.db.list_ci_jobs(run.id).await? {
                ci.total += 1;
                match j.status {
                    CiStatus::Passed => ci.passed += 1,
                    CiStatus::Failed | CiStatus::TimedOut => ci.failed += 1,
                    CiStatus::Running | CiStatus::Pending => ci.running += 1,
                    _ => {}
                }
            }
        }
        rows.push(MrRow {
            mr: ui_mr(mr, local),
            project_name: project.name.clone(),
            latest_run,
            finding_counts: counts,
            ci_summary: ci,
        });
    }
    Ok(rows)
}

/// Local MRs are shown with `project_id = 0` (the UI's "local" marker).
fn ui_mr(mut mr: MergeRequest, local: bool) -> MergeRequest {
    if local {
        mr.project_id = 0;
    }
    mr
}

#[tauri::command]
pub async fn start_review(state: St<'_>, mr_id: i64) -> Result<i64> {
    let mr = state.db.get_mr(mr_id).await?;
    let project = state.db.get_project(mr.project_id).await?;
    let run = if is_local_project(&project) {
        state
            .pipeline
            .queue_local_review(&project.local_path, &mr.target_branch, &mr.source_branch)
            .await?
    } else {
        state.pipeline.queue_mr_review(mr_id, None).await?
    };
    state.enqueue(run.id);
    Ok(run.id)
}

#[tauri::command]
pub async fn review_local(
    state: St<'_>,
    repo_path: String,
    base_ref: String,
    head_ref: String,
) -> Result<i64> {
    let run = state
        .pipeline
        .queue_local_review(&repo_path, &base_ref, &head_ref)
        .await?;
    state.enqueue(run.id);
    Ok(run.id)
}

#[tauri::command]
pub async fn cancel_run(state: St<'_>, run_id: i64) -> Result<()> {
    state.pipeline.cancel(run_id).await
}

/// Pasted code matched by a pattern rule (rule editor test box). Nothing is stored.
#[tauri::command]
pub async fn test_rule_pattern(pattern: String, code: String) -> Result<RuleTestResult> {
    let matches = checks::rules::test_pattern_on_code(&pattern, &code)?;
    Ok(RuleTestResult {
        matches,
        files_checked: 0,
        note: None,
    })
}

/// What a (not yet saved) rule would flag in a past run's diff. Read-only.
#[tauri::command]
pub async fn test_rule_on_run(
    state: St<'_>,
    rule: RuleConfig,
    run_id: i64,
) -> Result<RuleTestResult> {
    let run = state.db.get_run(run_id).await?;
    let cached = state
        .files_cache
        .lock()
        .ok()
        .and_then(|c| c.get(&run_id).cloned());
    let files = match cached {
        Some(f) => f,
        None => state.pipeline.run_files(&run).await?,
    };
    checks::rules::test_rule_on_files(&rule, &files)
}

#[tauri::command]
pub async fn get_run(state: St<'_>, run_id: i64) -> Result<RunDetail> {
    let run = state.db.get_run(run_id).await?;
    let mr = state.db.get_mr(run.mr_id).await?;
    let project = state.db.get_project(mr.project_id).await?;
    let findings = state.db.list_findings(run_id).await?;
    let ci_jobs = state.db.list_ci_jobs(run_id).await?;
    let cached = state
        .files_cache
        .lock()
        .ok()
        .and_then(|c| c.get(&run_id).cloned());
    let files = match cached {
        Some(f) => f,
        None => {
            let files = state.pipeline.run_files(&run).await.unwrap_or_default();
            if run.status.is_terminal() && !files.is_empty() {
                if let Ok(mut c) = state.files_cache.lock() {
                    if c.len() > 64 {
                        c.clear();
                    }
                    c.insert(run_id, files.clone());
                }
            }
            files
        }
    };
    Ok(RunDetail {
        run,
        mr: ui_mr(mr, is_local_project(&project)),
        findings,
        ci_jobs,
        files,
    })
}

#[tauri::command]
pub async fn list_runs(state: St<'_>, mr_id: Option<i64>, limit: u32) -> Result<Vec<ReviewRun>> {
    state.db.list_runs(mr_id, limit.clamp(1, 1000)).await
}

// ---------------------------------------------------------------------------------------------
// findings
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn update_finding(
    state: St<'_>,
    id: i64,
    title: String,
    body: String,
    suggestion: Option<String>,
) -> Result<()> {
    let f = state.db.get_finding(id).await?;
    if matches!(f.status, FindingStatus::Posted | FindingStatus::Resolved) {
        return Err(WardenError::Other(
            "this finding is already posted; edit it on the forge".into(),
        ));
    }
    let suggestion = suggestion.filter(|s| !s.trim().is_empty());
    state
        .db
        .update_finding_text(id, title.trim(), body.trim(), suggestion.as_deref())
        .await?;
    let _ = state
        .db
        .audit(
            Some(f.run_id),
            "guardrail",
            serde_json::json!({"event": "user_edit", "finding_id": id}),
        )
        .await;
    state
        .events
        .emit(AppEvent::FindingsUpdated { run_id: f.run_id });
    Ok(())
}

#[tauri::command]
pub async fn set_finding_status(state: St<'_>, id: i64, status: FindingStatus) -> Result<()> {
    let f = state.db.get_finding(id).await?;
    if matches!(status, FindingStatus::Posted | FindingStatus::Resolved) {
        return Err(WardenError::Other(format!(
            "status {status} is set by posting, not by hand"
        )));
    }
    if matches!(f.status, FindingStatus::Posted | FindingStatus::Resolved) {
        return Err(WardenError::Other("this finding is already posted".into()));
    }
    state.db.update_finding_status(id, status, None).await?;
    let _ = state
        .db
        .audit(
            Some(f.run_id),
            "guardrail",
            serde_json::json!({"event": "user_status", "finding_id": id, "from": f.status.as_str(), "to": status.as_str()}),
        )
        .await;
    state.pipeline.refresh_run_status(f.run_id).await?;
    Ok(())
}

#[tauri::command]
pub async fn post_findings(
    state: St<'_>,
    run_id: i64,
    finding_ids: Vec<i64>,
) -> Result<PostReport> {
    post_run_findings(&state, run_id, &finding_ids).await
}

/// Shared by `post_findings` and `post_findings_bulk`.
async fn post_run_findings(state: &Shared, run_id: i64, finding_ids: &[i64]) -> Result<PostReport> {
    let run = state.db.get_run(run_id).await?;
    let mr = state.db.get_mr(run.mr_id).await?;
    let project = state.db.get_project(mr.project_id).await?;
    let provider = state.require_provider(&project)?;
    state
        .pipeline
        .post_findings(run_id, finding_ids, provider)
        .await
}

/// Posts findings from several runs (approval queue): grouped by run, each group through the same
/// path as `post_findings`. One report per run (ascending run id). If every run fails the first
/// error is returned; otherwise a failed run counts its findings as `failed` and raises a notice.
#[tauri::command]
pub async fn post_findings_bulk(state: St<'_>, finding_ids: Vec<i64>) -> Result<Vec<PostReport>> {
    let groups = stats::group_by_run(&state.db, &finding_ids).await?;
    let mut reports = Vec::with_capacity(groups.len());
    let mut first_err = None;
    for (run_id, ids) in &groups {
        match post_run_findings(&state, *run_id, ids).await {
            Ok(r) => reports.push(r),
            Err(e) => {
                state.notice("error", format!("Posting run {run_id} failed: {e}"));
                reports.push(PostReport {
                    failed: u32::try_from(ids.len()).unwrap_or(u32::MAX),
                    ..Default::default()
                });
                first_err.get_or_insert(e);
            }
        }
    }
    match first_err {
        Some(e) if reports.iter().all(|r| r.posted == 0 && r.skipped == 0) => Err(e),
        _ => Ok(reports),
    }
}

// ---------------------------------------------------------------------------------------------
// stats / rule catalogue / guardrail preview / approval queue
// ---------------------------------------------------------------------------------------------

/// History rows (runs of every MR incl. merged/closed and local reviews), newest first. SQL only.
#[tauri::command]
pub async fn list_run_rows(state: St<'_>, limit: u32) -> Result<Vec<RunRow>> {
    state
        .db
        .list_run_rows(limit.clamp(1, 5000), LOCAL_BASE_URL)
        .await
}

#[tauri::command]
pub async fn get_stats(state: St<'_>, days: u32) -> Result<Stats> {
    stats::get_stats(&state.db, days).await
}

#[tauri::command]
pub async fn rule_stats(
    state: St<'_>,
    project_id: Option<i64>,
    days: u32,
) -> Result<Vec<RuleStat>> {
    // Configured rule titles: the project's merged config, or every project's plus the defaults.
    let projects = match project_id {
        Some(id) => vec![state.db.get_project(id).await?],
        None => state.db.list_projects().await?,
    };
    let mut rules = Vec::new();
    for p in &projects {
        if let Ok((cfg, _)) = state.pipeline.effective_config(p, None, false).await {
            rules.extend(cfg.rules);
        }
    }
    if project_id.is_none() {
        rules.extend(default_rules());
    }
    stats::rule_stats(&state.db, project_id, days, &rules).await
}

#[tauri::command]
pub async fn simulate_guardrails(
    state: St<'_>,
    project_id: i64,
    overrides: Value,
    limit_runs: u32,
) -> Result<Vec<SimulatedFinding>> {
    let project = state.db.get_project(project_id).await?;
    let (cfg, disabled) = state
        .pipeline
        .config_with_candidate(&project, &overrides)
        .await?;
    stats::simulate_guardrails(&state.db, project_id, &cfg, &disabled, limit_runs).await
}

#[tauri::command]
pub async fn preview_note(state: St<'_>, finding_id: i64) -> Result<String> {
    let f = state.db.get_finding(finding_id).await?;
    Ok(Poster::preview_note(&f))
}

#[tauri::command]
pub async fn list_pending_findings(state: St<'_>) -> Result<Vec<PendingFinding>> {
    stats::list_pending_findings(&state.db).await
}

// ---------------------------------------------------------------------------------------------
// windows
// ---------------------------------------------------------------------------------------------

const SETTINGS_WINDOW: &str = "settings";

/// `pane` limited to `[a-z0-9_-]` (it goes into a URL and a JS string); default `general`.
fn settings_pane(pane: Option<&str>) -> String {
    let p: String = pane
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    if p.is_empty() {
        "general".into()
    } else {
        p
    }
}

/// Opens the Settings window (`index.html?view=settings&pane=<pane>`), or focuses it and switches
/// to `pane` when it is already open.
#[tauri::command]
pub async fn open_settings_window(app: AppHandle, pane: Option<String>) -> Result<()> {
    let pane = settings_pane(pane.as_deref());
    let ui_err = |e: tauri::Error| WardenError::Other(format!("settings window: {e}"));
    if let Some(w) = app.get_webview_window(SETTINGS_WINDOW) {
        let current = w.url().ok().and_then(|u| {
            u.query_pairs()
                .find(|(k, _)| k == "pane")
                .map(|(_, v)| v.into_owned())
        });
        if current.as_deref() != Some(pane.as_str()) {
            w.eval(format!(
                "window.location.search = '?view=settings&pane={pane}'"
            ))
            .map_err(ui_err)?;
        }
        let _ = w.unminimize();
        w.show().map_err(ui_err)?;
        w.set_focus().map_err(ui_err)?;
        return Ok(());
    }
    let url = WebviewUrl::App(format!("index.html?view=settings&pane={pane}").into());
    let builder = WebviewWindowBuilder::new(&app, SETTINGS_WINDOW, url)
        .title("Warden Settings")
        .inner_size(900.0, 640.0)
        .min_inner_size(760.0, 520.0)
        .resizable(true);
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    let w = builder.build().map_err(ui_err)?;
    let _ = w.set_focus();
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// CI
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn rerun_ci_job(state: St<'_>, run_id: i64, name: String) -> Result<CiJob> {
    state.pipeline.rerun_ci_job(run_id, &name).await
}

#[tauri::command]
pub async fn read_ci_log(state: St<'_>, job_id: i64) -> Result<String> {
    let job = state.db.get_ci_job(job_id).await?;
    if job.log_path.is_empty() {
        return Ok(String::new());
    }
    let path = std::path::PathBuf::from(&job.log_path);
    let text = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
        let mut f = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
            Err(e) => return Err(e),
        };
        let len = f.metadata()?.len();
        let mut prefix = String::new();
        if len > MAX_LOG_BYTES {
            f.seek(SeekFrom::Start(len - MAX_LOG_BYTES))?;
            prefix = format!(
                "[warden] showing the last {} KB of the log\n",
                MAX_LOG_BYTES / 1024
            );
        }
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        Ok(prefix + &String::from_utf8_lossy(&buf))
    })
    .await??;
    Ok(redact_secrets(&text).0)
}

// ---------------------------------------------------------------------------------------------
// audit / posting
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn list_audit(state: St<'_>, run_id: Option<i64>, limit: u32) -> Result<Vec<AuditEvent>> {
    state.db.list_audit(run_id, limit.clamp(1, 5000)).await
}

#[tauri::command]
pub async fn set_posting_paused(app: AppHandle, state: St<'_>, paused: bool) -> Result<()> {
    crate::set_paused(&app, &state, paused).await
}
