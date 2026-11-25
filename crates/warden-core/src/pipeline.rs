// OWNER: integration
//! Orchestrates one review run end to end (SPEC "Pipeline"): usage gate -> Preparing (worktree, config,
//! diff, redact) -> Checking (checks + CI concurrently) -> Reviewing (RunRegistry + ReviewEngine) ->
//! Filtering (budget, auto_post per mode) -> AwaitingApproval / Posting -> incremental auto-resolve.
//! Also `review_local(repo_path, base_ref, head_ref)` for forge-less runs.
//!
//! Local runs are stored against a hidden project (`base_url = "local://"`, disabled, never polled)
//! and an MR row with `iid = 0`, because `review_runs.mr_id` must reference an MR. The Tauri layer
//! reports such MRs with `project_id = 0` so the UI treats them as local.
//!
//! The forge provider is passed in by the caller (the Tauri layer reads the token from the keychain);
//! the pipeline never sees or stores tokens, and nothing here hands a forge-writing tool to Claude.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::checks::secrets_scan::SECRET_RULE_ID;
use crate::checks::{redact_secrets, run_checks, CheckReport};
use crate::ci::{CiRunner, RecipeContext};
use crate::config::{
    load_repo_config, merge_overrides, parse_repo_config, RecipeConfig, RuleKind, WardenConfig,
    REPO_CONFIG_FILES,
};
use crate::db::{format_ts, now_ts, Db};
use crate::diff::{filter_ignored, parse_unified, ChangedIndex, FileDiff, Hunk, PathMatcher};
use crate::engine::prompt::{build_system_prompt, build_user_prompt};
use crate::engine::{ReviewEngine, ReviewRequest};
use crate::error::{Result, WardenError};
use crate::events::EventBus;
use crate::forge::ForgeProvider;
use crate::guardrails::{
    apply_budget, auto_post_for, fingerprint_for, validate_new_finding, GuardrailInput, Verdict,
    OVER_BUDGET,
};
use crate::mcp::{CiRerun, RunContext, RunRegistry};
use crate::poller::{MrChange, MrChangeReason};
use crate::poster::{PostReport, Poster};
use crate::types::{
    AppEvent, CiJob, CiStatus, Finding, FindingStatus, ForgeKind, MergeRequest, NewFinding,
    PostingMode, Project, ReviewRun, RunStatus, UsageStats,
};
use crate::worktree::{expand_tilde, is_hex_sha, sanitize_name, WorktreeManager};

/// `base_url` of the hidden projects that hold local (forge-less) runs.
pub const LOCAL_BASE_URL: &str = "local://";
/// Key in the overrides JSON listing rule ids the user switched off (Guardrails screen).
pub const DISABLED_RULES_KEY: &str = "disabled_rules";

const GIT_TIMEOUT: Duration = Duration::from_secs(120);
const CANCELLED_MSG: &str = "cancelled by user";

pub fn is_local_project(project: &Project) -> bool {
    project.base_url == LOCAL_BASE_URL
}

/// Result of trying to execute a queued run.
#[derive(Debug, Clone, PartialEq)]
pub enum ExecOutcome {
    /// Usage gate: the hourly review limit is reached; the run stays `Queued`.
    Deferred,
    /// The run finished (any terminal status).
    Finished(Box<ReviewRun>),
}

/// Everything a review run needs. Cheap to share behind an `Arc`.
pub struct Pipeline {
    pub db: Db,
    pub events: EventBus,
    pub registry: Arc<RunRegistry>,
    pub ci: Arc<CiRunner>,
    mcp_url: Mutex<String>,
    /// Shared with every per-run [`ReviewEngine`] so `cancel` reaches the claude process group.
    engine_cancels: Arc<Mutex<HashMap<i64, watch::Sender<bool>>>>,
    cancelled: Mutex<HashSet<i64>>,
}

impl Pipeline {
    pub fn new(
        db: Db,
        events: EventBus,
        registry: Arc<RunRegistry>,
        ci: Arc<CiRunner>,
        mcp_url: impl Into<String>,
    ) -> Self {
        Self {
            db,
            events,
            registry,
            ci,
            mcp_url: Mutex::new(mcp_url.into()),
            engine_cancels: Arc::new(Mutex::new(HashMap::new())),
            cancelled: Mutex::new(HashSet::new()),
        }
    }

    /// A `CiRunner` whose jobs get DB ids before they start (so live `CiLog` / `CiJobUpdated`
    /// events carry the real job id). Callers still `update_ci_job` the returned jobs.
    pub fn ci_runner(db: &Db, events: EventBus, log_dir: impl Into<PathBuf>) -> CiRunner {
        let db = db.clone();
        CiRunner::new(events, log_dir).with_job_id_allocator(move |job: CiJob| {
            let db = db.clone();
            async move { db.insert_ci_job(&job).await }
        })
    }

    pub fn mcp_url(&self) -> String {
        self.mcp_url.lock().map(|u| u.clone()).unwrap_or_default()
    }

    pub fn set_mcp_url(&self, url: impl Into<String>) {
        if let Ok(mut u) = self.mcp_url.lock() {
            *u = url.into();
        }
    }

    /// A review engine for the current settings that shares this pipeline's cancel map.
    pub fn engine(&self, settings: &crate::config::AppSettings) -> ReviewEngine {
        let mut engine = ReviewEngine::new(settings.clone(), self.events.clone());
        engine.cancels = self.engine_cancels.clone();
        engine
    }

    // -----------------------------------------------------------------------------------------
    // Usage gate
    // -----------------------------------------------------------------------------------------

    /// Hourly / daily review counts. `max_reviews_per_hour = 0` means unlimited.
    pub async fn usage(&self) -> Result<UsageStats> {
        let settings = self.db.get_settings().await?;
        let hour_ago = format_ts(Utc::now() - chrono::Duration::hours(1));
        let today = chrono::Local::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|t| t.and_local_timezone(chrono::Local).single())
            .map(|t| format_ts(t.with_timezone(&Utc)))
            .unwrap_or_else(|| hour_ago.clone());
        Ok(UsageStats {
            reviews_last_hour: self.db.reviews_started_since(&hour_ago).await?,
            reviews_today: self.db.reviews_started_since(&today).await?,
            max_reviews_per_hour: settings.max_reviews_per_hour,
            paused: settings.posting_paused,
        })
    }

    /// SPEC step 1. `posting_paused` only blocks posting, never reviewing.
    pub async fn usage_allows(&self) -> Result<bool> {
        let u = self.usage().await?;
        Ok(u.max_reviews_per_hour == 0 || u.reviews_last_hour < u.max_reviews_per_hour)
    }

    // -----------------------------------------------------------------------------------------
    // Queueing
    // -----------------------------------------------------------------------------------------

    /// Creates a `Queued` run for an MR (or returns the run already queued / in progress for the
    /// same head). `since` = last reviewed sha for an incremental re-review.
    ///
    /// At most one `Queued` run exists per MR: an existing queued run (whatever head it was queued
    /// for) is moved to the MR's current head and reused, and any older queued duplicates are
    /// cancelled, so a run held by the usage gate is not reviewed twice once the MR moves on.
    pub async fn queue_mr_review(&self, mr_id: i64, since: Option<String>) -> Result<ReviewRun> {
        let mr = self.db.get_mr(mr_id).await?;
        if let Some(latest) = self.db.latest_run_for_mr(mr_id).await? {
            if !latest.status.is_terminal() && latest.head_sha == mr.head_sha {
                return Ok(latest);
            }
        }
        let since = since.filter(|s| !s.is_empty() && *s != mr.head_sha);
        let mut queued: Vec<ReviewRun> = self
            .db
            .list_runs(Some(mr_id), 50)
            .await?
            .into_iter()
            .filter(|r| r.status == RunStatus::Queued)
            .collect(); // newest first
        if !queued.is_empty() {
            let mut keep = queued.remove(0);
            for stale in queued {
                self.cancel_superseded(stale, keep.id).await?;
            }
            if keep.head_sha != mr.head_sha || keep.base_sha != mr.base_sha {
                let old_head = std::mem::replace(&mut keep.head_sha, mr.head_sha.clone());
                keep.base_sha = mr.base_sha.clone();
                // The queued run never ran, so its own `since` (the last head actually reviewed) is
                // still right; only drop it if it now equals the head.
                if keep.since_sha.as_deref() == Some(mr.head_sha.as_str()) {
                    keep.since_sha = None;
                }
                self.db.update_run(&keep).await?;
                self.audit(
                    Some(keep.id),
                    "engine",
                    json!({"event": "requeued", "mr_id": mr.id, "old_head": old_head,
                           "head": mr.head_sha, "since": keep.since_sha}),
                )
                .await;
                self.emit_run(&keep);
            }
            return Ok(keep);
        }
        let settings = self.db.get_settings().await?;
        let run = self
            .db
            .create_run(
                mr.id,
                &mr.base_sha,
                &mr.head_sha,
                since.as_deref(),
                &settings.model,
            )
            .await?;
        self.audit(
            Some(run.id),
            "engine",
            json!({"event": "queued", "mr_id": mr.id, "iid": mr.iid, "head": mr.head_sha, "since": since}),
        )
        .await;
        self.emit_run(&run);
        Ok(run)
    }

    async fn cancel_superseded(&self, mut run: ReviewRun, by: i64) -> Result<()> {
        run.status = RunStatus::Cancelled;
        run.error = Some(format!("superseded by queued run {by}"));
        run.ended_at = Some(now_ts());
        self.db.update_run(&run).await?;
        self.audit(
            Some(run.id),
            "engine",
            json!({"event": "cancelled", "reason": "superseded", "by": by}),
        )
        .await;
        self.emit_run(&run);
        Ok(())
    }

    /// Records a `Skipped` run (draft MR, `no-ai-review` label).
    pub async fn record_skip(&self, mr: &MergeRequest, reason: &str) -> Result<ReviewRun> {
        let settings = self.db.get_settings().await?;
        let mut run = self
            .db
            .create_run(mr.id, &mr.base_sha, &mr.head_sha, None, &settings.model)
            .await?;
        run.status = RunStatus::Skipped;
        run.summary = Some(format!("Skipped: {reason}"));
        run.ended_at = Some(now_ts());
        self.db.update_run(&run).await?;
        self.audit(
            Some(run.id),
            "engine",
            json!({"event": "skipped", "mr_id": mr.id, "iid": mr.iid, "reason": reason}),
        )
        .await;
        self.emit_run(&run);
        Ok(run)
    }

    /// Applies poller changes: Closed -> remove worktree, skip reasons -> Skipped run, New /
    /// NewCommits -> queued run (incremental for NewCommits). Returns the queued run ids.
    pub async fn handle_poll_changes(
        &self,
        project: &Project,
        changes: &[MrChange],
    ) -> Result<Vec<i64>> {
        let mut queued = Vec::new();
        for change in changes {
            match &change.reason {
                MrChangeReason::Closed => {
                    let settings = self.db.get_settings().await?;
                    let wt = WorktreeManager::new(&settings.worktree_root);
                    if let Err(e) = wt.remove(project, change.mr.iid).await {
                        tracing::warn!("removing worktree for !{}: {e}", change.mr.iid);
                    }
                }
                _ if change.skip_reason().is_some() => {
                    let reason = change.skip_reason().unwrap_or("skipped");
                    // Do not record the same skip again for an unchanged head.
                    let latest = self.db.latest_run_for_mr(change.mr.id).await?;
                    let dup = latest.is_some_and(|r| {
                        r.status == RunStatus::Skipped && r.head_sha == change.mr.head_sha
                    });
                    if !dup {
                        self.record_skip(&change.mr, reason).await?;
                    }
                }
                MrChangeReason::New => {
                    queued.push(self.queue_mr_review(change.mr.id, None).await?.id);
                }
                MrChangeReason::NewCommits { since } => {
                    queued.push(
                        self.queue_mr_review(change.mr.id, Some(since.clone()))
                            .await?
                            .id,
                    );
                }
            }
        }
        if !changes.is_empty() {
            self.events.emit(AppEvent::MrsUpdated {
                project_id: project.id,
            });
        }
        Ok(queued)
    }

    /// Creates the hidden local project + MR row for `repo_path` and a `Queued` run for
    /// `base_ref...head_ref` (refs resolved to commit shas now).
    pub async fn queue_local_review(
        &self,
        repo_path: &str,
        base_ref: &str,
        head_ref: &str,
    ) -> Result<ReviewRun> {
        for r in [base_ref, head_ref] {
            if r.trim().is_empty() || r.trim_start().starts_with('-') {
                return Err(WardenError::Config(format!("invalid git ref {r:?}")));
            }
        }
        let repo = expand_tilde(Path::new(repo_path.trim()));
        let repo = repo
            .canonicalize()
            .map_err(|e| WardenError::Config(format!("repository {}: {e}", repo.display())))?;
        let top = git(&repo, &["rev-parse", "--show-toplevel"]).await?;
        let repo = PathBuf::from(top.trim());
        let base_sha = rev_parse(&repo, base_ref.trim()).await?;
        let head_sha = rev_parse(&repo, head_ref.trim()).await?;
        if base_sha == head_sha {
            return Err(WardenError::Config(format!(
                "{base_ref} and {head_ref} point at the same commit; nothing to review"
            )));
        }
        let repo_str = repo.to_string_lossy().to_string();
        let name = repo
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "local".into());
        let project_id = self
            .db
            .upsert_project(&Project {
                id: 0,
                forge: ForgeKind::Gitlab,
                base_url: LOCAL_BASE_URL.into(),
                remote_id: repo_str.clone(),
                name: name.clone(),
                local_path: repo_str.clone(),
                target_branches: vec![],
                mode: PostingMode::Draft,
                poll_interval_secs: 0,
                enabled: false,
            })
            .await?;
        let subject = git(&repo, &["log", "-1", "--format=%s", &head_sha])
            .await
            .unwrap_or_default();
        let author = git(&repo, &["log", "-1", "--format=%an", &head_sha])
            .await
            .unwrap_or_default();
        let mr = MergeRequest {
            id: 0,
            project_id,
            iid: 0,
            title: format!(
                "Local: {} vs {}{}",
                head_ref.trim(),
                base_ref.trim(),
                if subject.trim().is_empty() {
                    String::new()
                } else {
                    format!(" ({})", subject.trim())
                }
            ),
            description: String::new(),
            author: author.trim().to_string(),
            source_branch: head_ref.trim().into(),
            target_branch: base_ref.trim().into(),
            web_url: String::new(),
            draft: false,
            labels: vec![],
            head_sha: head_sha.clone(),
            base_sha: base_sha.clone(),
            start_sha: base_sha.clone(),
            last_reviewed_sha: None,
            additions: 0,
            deletions: 0,
            state: "opened".into(),
            updated_at: now_ts(),
        };
        let mr_id = self.db.upsert_mr(&mr).await?;
        let settings = self.db.get_settings().await?;
        let run = self
            .db
            .create_run(mr_id, &base_sha, &head_sha, None, &settings.model)
            .await?;
        self.audit(
            Some(run.id),
            "engine",
            json!({"event": "queued_local", "repo": repo_str, "base": base_ref, "head": head_ref}),
        )
        .await;
        self.emit_run(&run);
        self.events.emit(AppEvent::MrsUpdated { project_id });
        Ok(run)
    }

    /// SPEC `review_local`: queue + execute immediately (steps 2–5; no forge, nothing is posted).
    pub async fn review_local(
        &self,
        repo_path: &str,
        base_ref: &str,
        head_ref: &str,
    ) -> Result<ReviewRun> {
        let run = self
            .queue_local_review(repo_path, base_ref, head_ref)
            .await?;
        self.execute_run(run.id, None).await
    }

    // -----------------------------------------------------------------------------------------
    // Execution
    // -----------------------------------------------------------------------------------------

    /// Usage gate, then [`Pipeline::execute_run`].
    pub async fn execute_queued(
        &self,
        run_id: i64,
        provider: Option<Arc<dyn ForgeProvider>>,
    ) -> Result<ExecOutcome> {
        let run = self.db.get_run(run_id).await?;
        if run.status != RunStatus::Queued {
            return Ok(ExecOutcome::Finished(Box::new(run)));
        }
        if !self.usage_allows().await? {
            return Ok(ExecOutcome::Deferred);
        }
        Ok(ExecOutcome::Finished(Box::new(
            self.execute_run(run_id, provider).await?,
        )))
    }

    /// Runs a queued run to a terminal status. Step errors become `Failed` (or `Cancelled`), never
    /// `Err`; `Err` only when the run/MR/project rows cannot be read or written.
    pub async fn execute_run(
        &self,
        run_id: i64,
        provider: Option<Arc<dyn ForgeProvider>>,
    ) -> Result<ReviewRun> {
        let mut run = self.db.get_run(run_id).await?;
        if run.status != RunStatus::Queued {
            return Ok(run);
        }
        let mut mr = self.db.get_mr(run.mr_id).await?;
        let project = self.db.get_project(mr.project_id).await?;
        // A queued run reviews the MR's current head.
        if !mr.head_sha.is_empty() && mr.head_sha != run.head_sha {
            run.head_sha = mr.head_sha.clone();
            run.base_sha = mr.base_sha.clone();
        }
        mr.head_sha = run.head_sha.clone();
        if mr.base_sha.is_empty() {
            mr.base_sha = run.base_sha.clone();
        }
        if mr.start_sha.is_empty() {
            mr.start_sha = mr.base_sha.clone();
        }
        let provider = if is_local_project(&project) {
            None
        } else {
            provider
        };

        let result = self.run_steps(&mut run, &project, &mr, provider).await;
        let was_cancelled = self.take_cancelled(run.id);
        if let Err(e) = result {
            let msg = if was_cancelled {
                CANCELLED_MSG.to_string()
            } else {
                e.to_string()
            };
            run.status = if was_cancelled {
                RunStatus::Cancelled
            } else {
                RunStatus::Failed
            };
            run.error = Some(msg.clone());
            run.ended_at = Some(now_ts());
            self.db.update_run(&run).await?;
            self.emit_run(&run);
            self.audit(
                Some(run.id),
                if was_cancelled { "engine" } else { "error" },
                json!({"event": run.status.as_str(), "error": msg}),
            )
            .await;
            if !was_cancelled {
                self.events.emit(AppEvent::Notice {
                    level: "error".into(),
                    message: format!("Review of {} failed: {msg}", mr_label(&mr)),
                });
            }
        }
        Ok(run)
    }

    async fn run_steps(
        &self,
        run: &mut ReviewRun,
        project: &Project,
        mr: &MergeRequest,
        provider: Option<Arc<dyn ForgeProvider>>,
    ) -> Result<()> {
        let settings = self.db.get_settings().await?;
        let local = is_local_project(project);
        let mut worktrees = WorktreeManager::new(&settings.worktree_root);
        // Warden-managed clones fetch over HTTPS with the forge token (scoped to the forge host).
        if !local
            && crate::discovery::is_managed_clone(
                &crate::worktree::expand_tilde(std::path::Path::new(&settings.data_dir)),
                &project.local_path,
            )
        {
            if let Some(h) = provider.as_ref().and_then(|p| p.git_auth_header()) {
                worktrees =
                    worktrees.with_fetch_env(crate::discovery::auth_env(&project.base_url, &h));
            }
        }

        // ---- 2. Preparing ------------------------------------------------------------------
        run.started_at = now_ts();
        run.error = None;
        self.set_status(run, RunStatus::Preparing).await?;
        self.progress(run.id, "Preparing worktree");
        let worktree = if local {
            prepare_local_worktree(&worktrees, project, &run.head_sha).await?
        } else {
            worktrees.prepare(project, mr).await?
        };
        self.check_cancel(run.id)?;
        // The repo config is read from the BASE commit, never from the MR head: whoever opens the
        // MR must not be able to add CI recipes, auto-post rules, disabled rules or prompt text to
        // the review of their own MR.
        let (cfg, _) = self
            .effective_config(project, Some((&worktree, &run.base_sha)), true)
            .await?;
        let raw_diff = worktrees
            .diff(&worktree, &run.base_sha, &run.head_sha)
            .await?;
        let all_files = parse_unified(&raw_diff);
        let config_changed: Vec<String> = all_files
            .iter()
            .flat_map(|f| [f.old_path.clone(), f.new_path.clone()])
            .filter(|p| REPO_CONFIG_FILES.contains(&p.as_str()))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let files = filter_ignored(all_files, &cfg.review.ignore_paths);
        let keep_paths: HashSet<String> = files
            .iter()
            .flat_map(|f| [f.old_path.clone(), f.new_path.clone()])
            .collect();
        let index = ChangedIndex::from_files(&files);
        let (adds, dels) = files.iter().fold((0, 0), |(a, d), f| {
            let (fa, fd) = f.stats();
            (a + fa, d + fd)
        });
        // Incremental re-review: Claude sees only `since..head` (restricted to the MR's files);
        // the guardrails keep anchoring against the full `base...head` index. If `since` is not an
        // ancestor of head (force push / rebase) the whole MR is reviewed again.
        let increment = match run.since_sha.as_deref() {
            Some(since) => {
                incremental_diff(&worktrees, &worktree, since, &run.head_sha, &keep_paths).await
            }
            None => None,
        };
        if run.since_sha.is_some() && increment.is_none() {
            self.progress(
                run.id,
                "Incremental diff unavailable (history rewritten or no new reviewable changes); reviewing the whole MR",
            );
        }
        let (claude_files, diff_text, incremental_since) = match &increment {
            Some((inc_files, inc_raw)) => (
                inc_files.clone(),
                redact_secrets(&filter_diff_text(inc_raw, &keep_paths)).0,
                run.since_sha.clone(),
            ),
            None => (
                files.clone(),
                redact_secrets(&filter_diff_text(&raw_diff, &keep_paths)).0,
                None,
            ),
        };
        self.progress(
            run.id,
            format!(
                "Diff: {} files (+{adds} -{dels}){}",
                files.len(),
                incremental_since
                    .as_deref()
                    .map(|s| format!(
                        ", incremental since {} ({} file(s) changed since)",
                        short(s),
                        claude_files.len()
                    ))
                    .unwrap_or_default()
            ),
        );
        if !config_changed.is_empty() {
            let msg = format!(
                "{} changes {}; the change is NOT applied to this review (config read from base {})",
                mr_label(mr),
                config_changed.join(", "),
                short(&run.base_sha)
            );
            self.notice("warn", msg.clone());
            self.progress(run.id, msg);
            self.audit(
                Some(run.id),
                "guardrail",
                json!({"event": "config_changed_by_mr", "files": config_changed,
                       "config_from": run.base_sha}),
            )
            .await;
        }
        self.audit(
            Some(run.id),
            "engine",
            json!({
                "event": "prepared", "worktree": worktree.to_string_lossy(), "files": files.len(),
                "additions": adds, "deletions": dels, "since": run.since_sha,
                "mode": cfg.posting.mode.as_str(),
            }),
        )
        .await;

        let shas = (
            if mr.base_sha.is_empty() {
                run.base_sha.clone()
            } else {
                mr.base_sha.clone()
            },
            if mr.start_sha.is_empty() {
                run.base_sha.clone()
            } else {
                mr.start_sha.clone()
            },
            run.head_sha.clone(),
        );

        // Existing discussions (dedupe context for Claude and the guardrails).
        let (note_bodies, discussions) = match &provider {
            Some(p) => match p.list_discussions(project, mr.iid).await {
                Ok(ds) => {
                    let mut bodies = Vec::new();
                    let mut rendered = Vec::new();
                    for d in &ds {
                        for n in &d.notes {
                            bodies.push(n.body.clone());
                            let at = match (&n.path, n.line) {
                                (Some(p), Some(l)) => format!(" on {p}:{l}"),
                                (Some(p), None) => format!(" on {p}"),
                                _ => String::new(),
                            };
                            rendered.push(format!(
                                "[{}] {}{}: {}",
                                if d.resolved { "resolved" } else { "open" },
                                n.author,
                                at,
                                n.body
                            ));
                        }
                    }
                    (bodies, rendered)
                }
                Err(e) => {
                    self.notice("warn", format!("Could not load MR discussions: {e}"));
                    (Vec::new(), Vec::new())
                }
            },
            None => (Vec::new(), Vec::new()),
        };
        // Local runs do not dedupe against earlier local runs of the same repo.
        let existing_fps = if local {
            HashSet::new()
        } else {
            self.db.fingerprints_for_mr(mr.id).await?
        };
        let guard = Arc::new(GuardCtx {
            cfg: cfg.clone(),
            index: index.clone(),
            fingerprints: Mutex::new(existing_fps),
            note_bodies,
            shas: shas.clone(),
        });
        self.check_cancel(run.id)?;

        // ---- 3. Checking -------------------------------------------------------------------
        self.set_status(run, RunStatus::Checking).await?;
        self.progress(
            run.id,
            format!("Running checks and {} CI recipe(s)", cfg.ci.recipes.len()),
        );
        let rctx = RecipeContext {
            changed_files: index.changed_files(),
            base_sha: shas.0.clone(),
            head_sha: run.head_sha.clone(),
        };
        let (report, ci_jobs) = tokio::join!(
            run_checks(&worktree, &files, &cfg),
            self.run_ci(run.id, &worktree, &cfg, &rctx)
        );
        // Auto-resolve of rule findings relies on "the rule ran and did not re-find it".
        let checks_ok = report.is_ok();
        let report = report.unwrap_or_else(|e| CheckReport {
            notices: vec![format!("checks failed: {e}")],
            ..Default::default()
        });
        for n in &report.notices {
            self.progress(run.id, n.clone());
        }
        if !report.notices.is_empty() {
            self.audit(
                Some(run.id),
                "engine",
                json!({"event": "check_notices", "notices": report.notices}),
            )
            .await;
        }
        for f in &report.findings {
            let v = guard.validate(f);
            self.store_finding(run.id, f, &v, &index).await?;
        }
        self.events
            .emit(AppEvent::FindingsUpdated { run_id: run.id });
        let ci_failed = ci_jobs
            .iter()
            .any(|j| matches!(j.status, CiStatus::Failed | CiStatus::TimedOut));
        self.progress(
            run.id,
            format!(
                "Checks: {} rule finding(s); CI: {}",
                report.findings.len(),
                ci_line(&ci_jobs)
            ),
        );
        self.check_cancel(run.id)?;

        // ---- 4. Reviewing ------------------------------------------------------------------
        self.set_status(run, RunStatus::Reviewing).await?;
        let (ftx, mut frx) = mpsc::channel::<NewFinding>(64);
        let (rtx, mut rrx) = mpsc::unbounded_channel::<(NewFinding, String)>();
        let mut ctx = RunContext::new(run.id, &worktree, mr.clone(), cfg.clone(), ftx);
        ctx.diff_text = diff_text;
        ctx.files = files.clone();
        ctx.existing_discussions = discussions;
        ctx.ci_jobs = ci_jobs.clone();
        ctx.check_findings = report.findings.clone();
        ctx.ci_runner = Some(self.ci.clone());
        ctx.ci_rerun = Some(Arc::new(DbCiRerun {
            runner: self.ci.clone(),
            db: self.db.clone(),
            events: self.events.clone(),
        }));
        ctx.existing_fingerprints = guard.snapshot();
        let g = guard.clone();
        ctx.validator = Some(Arc::new(move |f: &NewFinding| {
            let v = g.validate(f);
            if v.keep {
                Ok(())
            } else {
                let reason = v
                    .reason
                    .unwrap_or_else(|| "rejected by guardrails".to_string());
                let _ = rtx.send((f.clone(), reason.clone()));
                Err(reason)
            }
        }));
        ctx.redactor = Some(Arc::new(|s: &str| redact_secrets(s).0));
        let token = self.registry.register(ctx);

        let engine = self.engine(&settings);
        let mut user_prompt = build_user_prompt(
            mr,
            &claude_files,
            &report,
            &ci_jobs,
            incremental_since.as_deref(),
        );
        if !config_changed.is_empty() {
            user_prompt.push_str(&format!(
                "\nNOTE: this MR modifies Warden's own review config ({}). That change is NOT applied to \
this review (the config comes from the target branch). Review it like any other change: flag it if it \
disables rules, widens ignore_paths, enables auto-posting, adds CI commands or adds review instructions.\n",
                config_changed.join(", ")
            ));
        }
        let req = ReviewRequest {
            run_id: run.id,
            worktree: worktree.clone(),
            mcp_url: self.mcp_url(),
            run_token: token.clone(),
            system_prompt: build_system_prompt(&cfg),
            user_prompt,
            max_turns: settings.max_turns,
            timeout: Duration::from_secs(settings.review_timeout_secs.max(30)),
            model: settings.model.clone(),
        };
        let review = engine.review(req);
        tokio::pin!(review);
        let outcome = loop {
            tokio::select! {
                res = &mut review => break res,
                Some(f) = frx.recv() => {
                    self.store_claude_finding(run.id, &f, &guard, &index).await;
                }
                Some((f, reason)) = rrx.recv() => {
                    self.store_rejected(run.id, &f, &reason, &index).await;
                }
            }
        };
        let submission = self.registry.submission(&token);
        let state = self.registry.get(&token).map(|c| c.state_snapshot());
        self.registry.remove(&token);
        frx.close();
        while let Some(f) = frx.recv().await {
            self.store_claude_finding(run.id, &f, &guard, &index).await;
        }
        rrx.close();
        while let Some((f, reason)) = rrx.recv().await {
            self.store_rejected(run.id, &f, &reason, &index).await;
        }
        self.events
            .emit(AppEvent::FindingsUpdated { run_id: run.id });

        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                self.audit(
                    Some(run.id),
                    "engine",
                    json!({"event": "engine_error", "error": e.to_string()}),
                )
                .await;
                return Err(e);
            }
        };
        run.turns = outcome.turns;
        run.duration_ms = outcome.duration_ms;
        run.cli_session_id = outcome.session_id.clone();
        run.summary = submission
            .as_ref()
            .map(|s| s.summary.clone())
            .or(outcome.summary.clone());
        run.verdict = submission
            .as_ref()
            .map(|s| s.verdict.clone())
            .or(outcome.verdict.clone());
        let submitted = submission.is_some() || outcome.submitted;
        self.audit(
            Some(run.id),
            "engine",
            json!({
                "event": "review_finished", "turns": outcome.turns, "duration_ms": outcome.duration_ms,
                "session_id": outcome.session_id, "submitted": submitted, "is_error": outcome.is_error,
                "error": outcome.error, "verdict": run.verdict,
            }),
        )
        .await;
        if let Some(st) = &state {
            self.audit(
                Some(run.id),
                "tool_call",
                json!({
                    "tool_calls": st.tool_calls, "accepted": st.accepted, "rejected": st.rejected,
                    "ci_reruns": st.ci_reruns,
                }),
            )
            .await;
        }
        self.check_cancel(run.id)?;
        if outcome.is_error && !submitted {
            return Err(WardenError::Engine(outcome.error.clone().unwrap_or_else(
                || "Claude Code ended without review_submit".into(),
            )));
        }
        if outcome.is_error {
            self.notice(
                "warn",
                format!(
                    "Claude finished with an error after submitting: {}",
                    outcome.error.clone().unwrap_or_default()
                ),
            );
        }
        if !submitted {
            self.progress(
                run.id,
                "Claude did not call review_submit; keeping its findings",
            );
        }

        // ---- 5. Filtering ------------------------------------------------------------------
        self.set_status(run, RunStatus::Filtering).await?;
        let mut findings = self.db.list_findings(run.id).await?;
        let before: HashMap<i64, FindingStatus> =
            findings.iter().map(|f| (f.id, f.status)).collect();
        apply_budget(&mut findings, cfg.review.max_notes);
        for f in &findings {
            if before.get(&f.id) != Some(&f.status) && f.status == FindingStatus::Rejected {
                self.db
                    .set_finding_guardrail(f.id, FindingStatus::Rejected, Some(OVER_BUDGET))
                    .await?;
                self.audit(
                    Some(run.id),
                    "guardrail",
                    json!({"finding_id": f.id, "title": f.title, "keep": false, "reason": OVER_BUDGET}),
                )
                .await;
            }
        }
        // A re-found finding that is still a pending Draft in an earlier run moves to this run
        // (fresh position); the old copy is marked superseded.
        if !local {
            let mut touched = HashSet::new();
            for f in findings.iter().filter(|f| f.status == FindingStatus::Draft) {
                let reason = format!("superseded by run {}", run.id);
                for rid in self
                    .db
                    .supersede_drafts(mr.id, &f.fingerprint, run.id, &reason)
                    .await?
                {
                    touched.insert(rid);
                }
            }
            for rid in touched {
                self.refresh_run_status(rid).await?;
            }
        }
        let mut auto_ids = Vec::new();
        if !local && provider.is_some() {
            for f in findings.iter().filter(|f| f.status == FindingStatus::Draft) {
                if auto_post_for(&cfg, f.severity, f.confidence) {
                    self.db
                        .update_finding_status(f.id, FindingStatus::Approved, None)
                        .await?;
                    auto_ids.push(f.id);
                }
            }
        }
        if !auto_ids.is_empty() {
            self.audit(
                Some(run.id),
                "guardrail",
                json!({"event": "auto_post", "mode": cfg.posting.mode.as_str(), "finding_ids": auto_ids}),
            )
            .await;
        }
        self.events
            .emit(AppEvent::FindingsUpdated { run_id: run.id });

        // ---- 6. Posting --------------------------------------------------------------------
        let poster = Poster::new(self.events.clone()).with_posting(cfg.posting.clone());
        if let Some(p) = &provider {
            // A clean review in Assist/Auto posts nothing, but can still approve the MR when
            // `posting.mr_status` is on. Never "request changes" without a note to point at, and
            // never act on its own in Draft mode.
            if cfg.posting.mode != PostingMode::Draft
                && auto_ids.is_empty()
                && !ci_failed
                && cfg.posting.mr_status.mode != crate::config::MrStatusMode::Off
            {
                let findings = self.db.list_findings(run.id).await?;
                if crate::poster::decide_mr_status(
                    run,
                    &findings,
                    &cfg.posting.mr_status.request_changes_on,
                ) == crate::types::MrReviewStatus::Approved
                {
                    poster
                        .apply_mr_status(&self.db, p.as_ref(), project, mr, run, &cfg.posting)
                        .await;
                }
            }
            if cfg.posting.mode != PostingMode::Draft && (!auto_ids.is_empty() || ci_failed) {
                self.set_status(run, RunStatus::Posting).await?;
                match poster
                    .post_run(&self.db, p.as_ref(), project, mr, run, None)
                    .await
                {
                    Ok(rep) => self.progress(run.id, post_line(&rep)),
                    Err(e) => {
                        self.notice("error", format!("Posting failed: {e}"));
                        self.audit(
                            Some(run.id),
                            "error",
                            json!({"event": "post_failed", "error": e.to_string()}),
                        )
                        .await;
                    }
                }
            }

            // ---- 7. Incremental re-review: resolve threads whose issue is gone ------------
            let refound: HashSet<String> = self
                .db
                .list_findings(run.id)
                .await?
                .into_iter()
                .map(|f| f.fingerprint)
                .collect();
            let rules_ran = RulesRan::new(&cfg, checks_ok, &report);
            let mut fixed: Vec<String> = Vec::new();
            for f in self
                .db
                .posted_findings_for_mr(mr.id)
                .await?
                .into_iter()
                .filter(|f| f.run_id != run.id && !refound.contains(&f.fingerprint))
            {
                let gone = if f.source == "rule" {
                    // Not re-found only means fixed if the rule actually scanned this file now.
                    rules_ran.covers(&f)
                } else {
                    self.anchor_gone(&worktree, &f, &run.head_sha, &shas.0, &index, submitted)
                        .await
                };
                if gone {
                    fixed.push(f.fingerprint);
                }
            }
            if !fixed.is_empty() {
                match poster
                    .auto_resolve(&self.db, p.as_ref(), project, mr, &fixed)
                    .await
                {
                    Ok(n) => {
                        if n > 0 {
                            self.progress(run.id, format!("Resolved {n} fixed thread(s)"));
                        }
                    }
                    Err(e) => self.notice("warn", format!("Auto-resolve failed: {e}")),
                }
            }
        }

        // ---- final status ------------------------------------------------------------------
        let findings = self.db.list_findings(run.id).await?;
        let pending = findings
            .iter()
            .any(|f| matches!(f.status, FindingStatus::Draft | FindingStatus::Approved));
        run.ended_at = Some(now_ts());
        self.set_status(
            run,
            if pending {
                RunStatus::AwaitingApproval
            } else {
                RunStatus::Posted
            },
        )
        .await?;
        self.db.set_last_reviewed(mr.id, &run.head_sha).await?;
        self.events.emit(AppEvent::MrsUpdated {
            project_id: project.id,
        });
        Ok(())
    }

    async fn run_ci(
        &self,
        run_id: i64,
        worktree: &Path,
        cfg: &WardenConfig,
        rctx: &RecipeContext,
    ) -> Vec<CiJob> {
        let jobs = match self.ci.run_all(run_id, worktree, &cfg.ci, rctx).await {
            Ok(j) => j,
            Err(e) => {
                self.notice("warn", format!("CI could not run: {e}"));
                return Vec::new();
            }
        };
        let mut out = Vec::with_capacity(jobs.len());
        for job in jobs {
            out.push(persist_job(&self.db, job).await);
        }
        out
    }

    // -----------------------------------------------------------------------------------------
    // Findings
    // -----------------------------------------------------------------------------------------

    async fn store_finding(
        &self,
        run_id: i64,
        f: &NewFinding,
        v: &Verdict,
        index: &ChangedIndex,
    ) -> Result<Finding> {
        let fp = fingerprint_for(f, index);
        let status = if v.keep {
            FindingStatus::Draft
        } else {
            FindingStatus::Rejected
        };
        let reason = if v.keep { None } else { v.reason.as_deref() };
        let stored = self
            .db
            .insert_finding(run_id, f, status, reason, &fp, v.position.as_ref())
            .await?;
        self.audit(
            Some(run_id),
            "guardrail",
            json!({
                "finding_id": stored.id, "source": f.source, "file": f.file, "line": f.line,
                "rule_id": f.rule_id, "title": f.title, "keep": v.keep, "reason": v.reason,
            }),
        )
        .await;
        Ok(stored)
    }

    /// Accepted by `review_add_finding` (built-in checks + our validator already ran).
    async fn store_claude_finding(
        &self,
        run_id: i64,
        f: &NewFinding,
        guard: &GuardCtx,
        index: &ChangedIndex,
    ) {
        let (b, s, h) = (&guard.shas.0, &guard.shas.1, &guard.shas.2);
        let v = Verdict {
            keep: true,
            reason: None,
            auto_post: false,
            position: index.position(&f.file, f.line, &f.side, (b, s, h)),
        };
        match self.store_finding(run_id, f, &v, index).await {
            Ok(stored) => self.progress(
                run_id,
                format!(
                    "Finding: {} · {}:{} · {}",
                    stored.severity.as_str(),
                    stored.file,
                    stored.line,
                    stored.title
                ),
            ),
            Err(e) => tracing::error!("storing finding failed: {e}"),
        }
        self.events.emit(AppEvent::FindingsUpdated { run_id });
    }

    async fn store_rejected(
        &self,
        run_id: i64,
        f: &NewFinding,
        reason: &str,
        index: &ChangedIndex,
    ) {
        let v = Verdict {
            keep: false,
            reason: Some(reason.to_string()),
            auto_post: false,
            position: None,
        };
        if let Err(e) = self.store_finding(run_id, f, &v, index).await {
            tracing::error!("storing rejected finding failed: {e}");
        }
    }

    /// SPEC step 7 for a posted Claude/CI finding that was not re-found: true when its anchor is
    /// gone from the change. The old anchor (a line number at the finding's head) is first moved
    /// to the new head through `git diff -U0 old_head new_head`; then:
    /// - the anchor line itself was edited or deleted since (only trusted when Claude finished
    ///   with `review_submit`, so "not re-found" means something), or
    /// - the moved line is no longer inside the MR's diff hunks.
    ///
    /// Anything uncertain (git errors, rebased base for old-side anchors) keeps the thread open.
    async fn anchor_gone(
        &self,
        worktree: &Path,
        f: &Finding,
        new_head: &str,
        new_base: &str,
        index: &ChangedIndex,
        submitted: bool,
    ) -> bool {
        if f.side == "old" {
            let same_base = f.position.as_ref().is_some_and(|p| p.base_sha == new_base);
            return same_base && !index.is_deleted_line(&f.file, f.line);
        }
        let old_head = match f.position.as_ref().map(|p| p.head_sha.clone()) {
            Some(h) if !h.is_empty() => h,
            _ => match self.db.get_run(f.run_id).await {
                Ok(r) => r.head_sha,
                Err(_) => return false,
            },
        };
        let moved = if old_head == new_head {
            Some(f.line)
        } else {
            if !is_hex_sha(&old_head) || !is_hex_sha(new_head) {
                return false;
            }
            let out = git(
                worktree,
                &[
                    "diff",
                    "--no-color",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--src-prefix=a/",
                    "--dst-prefix=b/",
                    "-U0",
                    &old_head,
                    new_head,
                    "--",
                    &f.file,
                ],
            )
            .await;
            let Ok(out) = out else {
                return false;
            };
            let hunks: Vec<Hunk> = parse_unified(&out)
                .into_iter()
                .flat_map(|fd| fd.hunks)
                .collect();
            map_line_through(&hunks, f.line)
        };
        match moved {
            None => submitted,
            Some(n) => !index.in_hunk(&f.file, n),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Config
    // -----------------------------------------------------------------------------------------

    /// Repo `.warden.yml` merged with the project's overrides. With `at = Some((repo, sha))` the
    /// file is read from commit `sha` (a run's BASE commit; never the MR head, which the MR author
    /// controls) in `repo` (the run's worktree, else the project's local clone, which shares its
    /// objects). With `None` it is read from the project's local clone working tree (display
    /// only), else defaults. `disabled_rules` in the overrides is removed from the rule list when
    /// `drop_disabled`; `posting.mode` always comes from `Project.mode`.
    /// Returns the config and the disabled rule ids.
    pub async fn effective_config(
        &self,
        project: &Project,
        at: Option<(&Path, &str)>,
        drop_disabled: bool,
    ) -> Result<(WardenConfig, Vec<String>)> {
        let base = match at {
            Some((repo, sha)) => {
                let lp = expand_tilde(Path::new(&project.local_path));
                let repo = if repo.is_dir() {
                    repo.to_path_buf()
                } else if !project.local_path.is_empty() && lp.is_dir() {
                    lp
                } else {
                    return Err(WardenError::NotFound(format!(
                        "repository for {} (no worktree or local clone)",
                        project.name
                    )));
                };
                config_at_commit(&repo, sha).await?
            }
            None => {
                let lp = expand_tilde(Path::new(&project.local_path));
                if !project.local_path.is_empty() && lp.is_dir() {
                    load_repo_config(&lp)?
                } else {
                    WardenConfig::default()
                }
            }
        };
        let mut overrides = if project.id > 0 {
            self.db.get_project_overrides(project.id).await?
        } else {
            json!({})
        };
        let disabled: Vec<String> = match overrides.as_object_mut() {
            Some(o) => o
                .remove(DISABLED_RULES_KEY)
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let mut cfg = merge_overrides(base, &overrides)?;
        if drop_disabled && !disabled.is_empty() {
            cfg.rules.retain(|r| !disabled.contains(&r.id));
        }
        cfg.posting.mode = project.mode;
        Ok((cfg, disabled))
    }

    /// The config a project would have with `candidate` overrides merged over its stored ones
    /// (Guardrails live preview; nothing is saved). Display base as in `effective_config(None)`.
    /// `candidate.disabled_rules`, when present, replaces the stored list; `candidate.posting.mode`,
    /// when present, replaces `Project.mode`. Rules stay in the list (disabled ids are returned).
    pub async fn config_with_candidate(
        &self,
        project: &Project,
        candidate: &Value,
    ) -> Result<(WardenConfig, Vec<String>)> {
        let (cfg, stored_disabled) = self.effective_config(project, None, false).await?;
        let mut candidate = match candidate {
            Value::Null => json!({}),
            Value::Object(_) => candidate.clone(),
            _ => {
                return Err(WardenError::Config(
                    "overrides must be a JSON object".into(),
                ))
            }
        };
        let disabled = match candidate
            .as_object_mut()
            .and_then(|o| o.remove(DISABLED_RULES_KEY))
        {
            Some(v) => serde_json::from_value::<Vec<String>>(v)
                .map_err(|e| WardenError::Config(format!("disabled_rules: {e}")))?,
            None => stored_disabled,
        };
        let cfg = merge_overrides(cfg, &candidate)?;
        Ok((cfg, disabled))
    }

    /// Worktree directory of a run's MR (may not exist any more).
    pub async fn worktree_for(&self, project: &Project, mr: &MergeRequest) -> Result<PathBuf> {
        let settings = self.db.get_settings().await?;
        let wt = WorktreeManager::new(&settings.worktree_root);
        Ok(if is_local_project(project) {
            local_worktree_path(&wt, project)
        } else {
            wt.worktree_path(project, mr.iid)
        })
    }

    /// Diff files of a run (worktree first, then the project's local clone), ignore globs applied.
    pub async fn run_files(&self, run: &ReviewRun) -> Result<Vec<FileDiff>> {
        let mr = self.db.get_mr(run.mr_id).await?;
        let project = self.db.get_project(mr.project_id).await?;
        let wt = self.worktree_for(&project, &mr).await?;
        let local = expand_tilde(Path::new(&project.local_path));
        let mut raw = None;
        for dir in [wt.clone(), local] {
            if dir.is_dir() {
                if let Ok(d) = git(
                    &dir,
                    &[
                        "-c",
                        "core.quotePath=false",
                        "diff",
                        "--no-color",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--src-prefix=a/",
                        "--dst-prefix=b/",
                        "-U3",
                        &format!("{}...{}", run.base_sha, run.head_sha),
                    ],
                )
                .await
                {
                    raw = Some(d);
                    break;
                }
            }
        }
        let Some(raw) = raw else {
            return Ok(Vec::new());
        };
        let cfg = self
            .effective_config(&project, Some((&wt, &run.base_sha)), true)
            .await
            .map(|c| c.0)
            .unwrap_or_default();
        Ok(filter_ignored(
            parse_unified(&raw),
            &cfg.review.ignore_paths,
        ))
    }

    // -----------------------------------------------------------------------------------------
    // User actions
    // -----------------------------------------------------------------------------------------

    /// Cancels a queued or running run (claude process group and CI jobs are killed).
    pub async fn cancel(&self, run_id: i64) -> Result<()> {
        let mut run = self.db.get_run(run_id).await?;
        if run.status.is_terminal() {
            return Ok(());
        }
        if run.status == RunStatus::Queued {
            run.status = RunStatus::Cancelled;
            run.error = Some(CANCELLED_MSG.into());
            run.ended_at = Some(now_ts());
            self.db.update_run(&run).await?;
            self.emit_run(&run);
            self.audit(Some(run_id), "engine", json!({"event": "cancelled"}))
                .await;
            return Ok(());
        }
        if let Ok(mut c) = self.cancelled.lock() {
            c.insert(run_id);
        }
        if let Ok(map) = self.engine_cancels.lock() {
            if let Some(tx) = map.get(&run_id) {
                let _ = tx.send(true);
            }
        }
        self.ci.cancel(run_id);
        Ok(())
    }

    /// Posts the selected findings of a run (user approval). Guardrail-rejected findings the user
    /// ticked by hand are approved first. Updates the run status afterwards.
    pub async fn post_findings(
        &self,
        run_id: i64,
        finding_ids: &[i64],
        provider: Arc<dyn ForgeProvider>,
    ) -> Result<PostReport> {
        let run = self.db.get_run(run_id).await?;
        let mr = self.db.get_mr(run.mr_id).await?;
        let project = self.db.get_project(mr.project_id).await?;
        if is_local_project(&project) {
            return Err(WardenError::Other(
                "local reviews have no merge request to post to".into(),
            ));
        }
        let findings = self.db.list_findings(run_id).await?;
        for f in findings.iter().filter(|f| finding_ids.contains(&f.id)) {
            if f.status == FindingStatus::Rejected {
                self.db
                    .update_finding_status(f.id, FindingStatus::Approved, None)
                    .await?;
                self.audit(
                    Some(run_id),
                    "guardrail",
                    json!({"finding_id": f.id, "event": "manual_override", "reason": f.guardrail_reason}),
                )
                .await;
            }
        }
        let wt = self.worktree_for(&project, &mr).await?;
        let (cfg, _) = self
            .effective_config(&project, Some((&wt, &run.base_sha)), true)
            .await?;
        let poster = Poster::new(self.events.clone()).with_posting(cfg.posting.clone());
        let mut run_now = run.clone();
        if run_now.status == RunStatus::AwaitingApproval {
            run_now.status = RunStatus::Posting;
            self.db.update_run(&run_now).await?;
            self.emit_run(&run_now);
        }
        let report = poster
            .post_run(
                &self.db,
                provider.as_ref(),
                &project,
                &mr,
                &run_now,
                Some(finding_ids),
            )
            .await;
        self.refresh_run_status(run_id).await?;
        report
    }

    /// AwaitingApproval / Posting runs become Posted once no Draft/Approved finding is left.
    pub async fn refresh_run_status(&self, run_id: i64) -> Result<ReviewRun> {
        let mut run = self.db.get_run(run_id).await?;
        if !matches!(
            run.status,
            RunStatus::AwaitingApproval | RunStatus::Posting | RunStatus::Posted
        ) {
            return Ok(run);
        }
        let pending = self
            .db
            .list_findings(run_id)
            .await?
            .iter()
            .any(|f| matches!(f.status, FindingStatus::Draft | FindingStatus::Approved));
        let next = if pending {
            RunStatus::AwaitingApproval
        } else {
            RunStatus::Posted
        };
        if next != run.status {
            run.status = next;
            run.ended_at = Some(now_ts());
            self.db.update_run(&run).await?;
            self.emit_run(&run);
        }
        self.events.emit(AppEvent::FindingsUpdated { run_id });
        Ok(run)
    }

    /// Re-runs one CI recipe of a run in its worktree (the job is stored and streamed).
    pub async fn rerun_ci_job(&self, run_id: i64, name: &str) -> Result<CiJob> {
        let run = self.db.get_run(run_id).await?;
        let mr = self.db.get_mr(run.mr_id).await?;
        let project = self.db.get_project(mr.project_id).await?;
        let wt = self.worktree_for(&project, &mr).await?;
        if !wt.is_dir() {
            return Err(WardenError::NotFound(format!(
                "worktree {} (review the MR again first)",
                wt.display()
            )));
        }
        let (cfg, _) = self
            .effective_config(&project, Some((&wt, &run.base_sha)), true)
            .await?;
        let recipe: RecipeConfig = cfg
            .ci
            .recipes
            .iter()
            .find(|r| r.name == name)
            .cloned()
            .ok_or_else(|| WardenError::NotFound(format!("CI recipe {name:?}")))?;
        let files = self.run_files(&run).await.unwrap_or_default();
        let rctx = RecipeContext {
            changed_files: ChangedIndex::from_files(&files).changed_files(),
            base_sha: run.base_sha.clone(),
            head_sha: run.head_sha.clone(),
        };
        let rerun = DbCiRerun {
            runner: self.ci.clone(),
            db: self.db.clone(),
            events: self.events.clone(),
        };
        let job = rerun.rerun(run_id, &wt, &recipe, &rctx).await?;
        self.audit(
            Some(run_id),
            "engine",
            json!({"event": "ci_rerun", "job": job.name, "status": job.status.as_str()}),
        )
        .await;
        Ok(job)
    }

    // -----------------------------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------------------------

    async fn set_status(&self, run: &mut ReviewRun, status: RunStatus) -> Result<()> {
        run.status = status;
        self.db.update_run(run).await?;
        self.emit_run(run);
        Ok(())
    }

    fn emit_run(&self, run: &ReviewRun) {
        self.events.emit(AppEvent::RunUpdated { run: run.clone() });
    }

    fn progress(&self, run_id: i64, message: impl Into<String>) {
        self.events.emit(AppEvent::RunProgress {
            run_id,
            message: message.into(),
            turn: 0,
        });
    }

    fn notice(&self, level: &str, message: impl Into<String>) {
        self.events.emit(AppEvent::Notice {
            level: level.into(),
            message: message.into(),
        });
    }

    async fn audit(&self, run_id: Option<i64>, kind: &str, payload: Value) {
        if let Err(e) = self.db.audit(run_id, kind, payload).await {
            tracing::warn!("audit write failed: {e}");
        }
    }

    fn check_cancel(&self, run_id: i64) -> Result<()> {
        let c = self
            .cancelled
            .lock()
            .map(|c| c.contains(&run_id))
            .unwrap_or(false);
        if c {
            Err(WardenError::Other(CANCELLED_MSG.into()))
        } else {
            Ok(())
        }
    }

    fn take_cancelled(&self, run_id: i64) -> bool {
        self.cancelled
            .lock()
            .map(|mut c| c.remove(&run_id))
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------------------------
// Guardrail context shared with the MCP validator hook
// ---------------------------------------------------------------------------------------------

struct GuardCtx {
    cfg: WardenConfig,
    index: ChangedIndex,
    /// Existing fingerprints for the MR plus every finding kept in this run so far.
    fingerprints: Mutex<HashSet<String>>,
    note_bodies: Vec<String>,
    shas: (String, String, String),
}

impl GuardCtx {
    fn validate(&self, f: &NewFinding) -> Verdict {
        let mut fps = self.fingerprints.lock().unwrap_or_else(|e| e.into_inner());
        let input = GuardrailInput {
            cfg: &self.cfg,
            index: &self.index,
            existing_fingerprints: &fps,
            existing_note_bodies: &self.note_bodies,
            shas: (&self.shas.0, &self.shas.1, &self.shas.2),
        };
        let v = validate_new_finding(f, &input);
        if v.keep {
            fps.insert(fingerprint_for(f, &self.index));
        }
        v
    }

    fn snapshot(&self) -> HashSet<String> {
        self.fingerprints
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------------------------
// CI rerun that writes to the DB
// ---------------------------------------------------------------------------------------------

/// `ci_run_recipe` / `rerun_ci_job`: runs the recipe, redacts the excerpt, stores the job.
pub struct DbCiRerun {
    pub runner: Arc<CiRunner>,
    pub db: Db,
    pub events: EventBus,
}

#[async_trait]
impl CiRerun for DbCiRerun {
    async fn rerun(
        &self,
        run_id: i64,
        worktree: &Path,
        recipe: &RecipeConfig,
        ctx: &RecipeContext,
    ) -> Result<CiJob> {
        let job = self.runner.run_one(run_id, worktree, recipe, ctx).await?;
        let job = persist_job(&self.db, job).await;
        self.events
            .emit(AppEvent::CiJobUpdated { job: job.clone() });
        Ok(job)
    }
}

/// Redacts the failure excerpt and inserts (id 0) or updates the job.
async fn persist_job(db: &Db, mut job: CiJob) -> CiJob {
    job.failure_excerpt = job.failure_excerpt.map(|e| redact_secrets(&e).0);
    let res = if job.id == 0 {
        db.insert_ci_job(&job).await.map(|id| job.id = id)
    } else {
        db.update_ci_job(&job).await
    };
    if let Err(e) = res {
        tracing::warn!("storing CI job {} failed: {e}", job.name);
    }
    job
}

// ---------------------------------------------------------------------------------------------
// git helpers (local runs)
// ---------------------------------------------------------------------------------------------

/// `since..head` limited to the MR's (non-ignored) files, as (parsed files, raw diff). `None` when
/// `since` is not an ancestor of `head` (rewritten history), git fails, or nothing reviewable
/// changed since.
async fn incremental_diff(
    worktrees: &WorktreeManager,
    worktree: &Path,
    since: &str,
    head: &str,
    keep_paths: &HashSet<String>,
) -> Option<(Vec<FileDiff>, String)> {
    if !is_hex_sha(since) || !is_hex_sha(head) {
        return None;
    }
    git(worktree, &["merge-base", "--is-ancestor", since, head])
        .await
        .ok()?;
    let raw = match worktrees.diff(worktree, since, head).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("incremental diff {since}..{head} failed: {e}");
            return None;
        }
    };
    let files: Vec<FileDiff> = parse_unified(&raw)
        .into_iter()
        .filter(|f| keep_paths.contains(&f.new_path) || keep_paths.contains(&f.old_path))
        .collect();
    if files.is_empty() {
        return None;
    }
    Some((files, raw))
}

/// `.warden.yml` (or `.warden.yaml`) as of commit `sha` in `repo`; defaults when the commit has
/// none. The working tree is never read, so an MR cannot change the config of its own review.
pub async fn config_at_commit(repo: &Path, sha: &str) -> Result<WardenConfig> {
    if !is_hex_sha(sha) {
        return Err(WardenError::Config(format!(
            "cannot read the repo config at {sha:?}: not a commit sha"
        )));
    }
    for name in REPO_CONFIG_FILES {
        let spec = format!("{sha}:{name}");
        if git(repo, &["cat-file", "-e", &spec]).await.is_err() {
            continue;
        }
        let text = git(repo, &["show", &spec]).await?;
        return parse_repo_config(&text)
            .map_err(|e| WardenError::Config(format!("{name} at {}: {e}", short(sha))));
    }
    Ok(WardenConfig::default())
}

fn local_worktree_path(wt: &WorktreeManager, project: &Project) -> PathBuf {
    let tag = &crate::guardrails::fingerprint(&project.local_path, "", None)[..8];
    wt.root_dir()
        .join("_local")
        .join(format!("{}-{tag}", sanitize_name(&project.name)))
}

async fn prepare_local_worktree(
    wt: &WorktreeManager,
    project: &Project,
    head_sha: &str,
) -> Result<PathBuf> {
    let repo = expand_tilde(Path::new(&project.local_path));
    let path = local_worktree_path(wt, project);
    let path_s = path.to_string_lossy().to_string();
    if path.join(".git").exists()
        && git(&path, &["rev-parse", "--is-inside-work-tree"])
            .await
            .is_ok()
    {
        git(&path, &["checkout", "--force", "--detach", head_sha]).await?;
        git(&path, &["clean", "-fd"]).await?;
    } else {
        if path.exists() {
            let _ = std::fs::remove_dir_all(&path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = git(&repo, &["worktree", "prune"]).await;
        git(
            &repo,
            &["worktree", "add", "--force", "--detach", &path_s, head_sha],
        )
        .await?;
    }
    Ok(path)
}

async fn rev_parse(repo: &Path, r: &str) -> Result<String> {
    let out = git(repo, &["rev-parse", "--verify", &format!("{r}^{{commit}}")])
        .await
        .map_err(|_| WardenError::Git(format!("unknown revision {r:?} in {}", repo.display())))?;
    Ok(out.trim().to_string())
}

async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .kill_on_drop(true);
    let out = tokio::time::timeout(GIT_TIMEOUT, cmd.output())
        .await
        .map_err(|_| WardenError::Git(format!("git {} timed out", args.join(" "))))??;
    if !out.status.success() {
        return Err(WardenError::Git(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

// ---------------------------------------------------------------------------------------------
// small pure helpers
// ---------------------------------------------------------------------------------------------

/// Keeps only the `diff --git` sections whose old or new path is in `keep`.
pub fn filter_diff_text(raw: &str, keep: &HashSet<String>) -> String {
    if !raw.contains("diff --git ") {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let mut section = String::new();
    let flush = |section: &mut String, out: &mut String| {
        if section.is_empty() {
            return;
        }
        let parsed = parse_unified(section);
        let keep_it = parsed.is_empty()
            || parsed
                .iter()
                .any(|f| keep.contains(&f.new_path) || keep.contains(&f.old_path));
        if keep_it {
            out.push_str(section);
        }
        section.clear();
    };
    for line in raw.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            flush(&mut section, &mut out);
        }
        section.push_str(line);
    }
    flush(&mut section, &mut out);
    out
}

/// Maps a line number of the old side of a `-U0` diff to the new side. `None` when the line
/// itself was changed or deleted.
pub fn map_line_through(hunks: &[Hunk], line: u32) -> Option<u32> {
    let mut sorted: Vec<&Hunk> = hunks.iter().collect();
    sorted.sort_by_key(|h| h.old_start);
    let mut shift: i64 = 0;
    for h in sorted {
        if h.old_lines == 0 {
            // Pure insertion after old line `old_start`.
            if h.old_start < line {
                shift += i64::from(h.new_lines);
            } else {
                break;
            }
        } else {
            let end = h.old_start + h.old_lines; // exclusive
            if line < h.old_start {
                break;
            }
            if line < end {
                return None;
            }
            shift += i64::from(h.new_lines) - i64::from(h.old_lines);
        }
    }
    u32::try_from(i64::from(line) + shift)
        .ok()
        .filter(|n| *n > 0)
}

/// Which deterministic rules really scanned which files in this run.
pub struct RulesRan<'a> {
    cfg: &'a WardenConfig,
    checks_ok: bool,
    semgrep_installed: bool,
    notices: &'a [String],
    ignore: PathMatcher,
}

impl<'a> RulesRan<'a> {
    pub fn new(cfg: &'a WardenConfig, checks_ok: bool, report: &'a CheckReport) -> Self {
        Self {
            cfg,
            checks_ok,
            semgrep_installed: report
                .tools
                .iter()
                .any(|t| t.name == "semgrep" && t.installed),
            notices: &report.notices,
            ignore: PathMatcher::lenient(&cfg.review.ignore_paths),
        }
    }

    /// True only when a posted rule finding's rule ran over its file in this run (rule still
    /// enabled, file not ignored and inside `applies_to`, checks did not fail, semgrep present
    /// for semgrep rules), so "not re-found" really means "fixed".
    pub fn covers(&self, f: &Finding) -> bool {
        if !self.checks_ok || self.ignore.is_match(&f.file) {
            return false;
        }
        let Some(id) = f.rule_id.as_deref() else {
            return false;
        };
        if id == SECRET_RULE_ID {
            return true; // the secret scanner always runs
        }
        let Some(rule) = self.cfg.rules.iter().find(|r| r.id == id) else {
            return false; // disabled or removed
        };
        if let Some(glob) = rule
            .applies_to
            .as_deref()
            .map(str::trim)
            .filter(|g| !g.is_empty())
        {
            match PathMatcher::new(&[glob]) {
                Ok(m) if m.is_match(&f.file) => {}
                _ => return false,
            }
        }
        match rule.kind() {
            RuleKind::Regex => true,
            RuleKind::Semgrep => {
                self.semgrep_installed && !self.notices.iter().any(|n| n.contains(id))
            }
            RuleKind::Claude => false,
        }
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

fn mr_label(mr: &MergeRequest) -> String {
    if mr.iid == 0 {
        format!("local {}", mr.source_branch)
    } else {
        format!("!{} {}", mr.iid, mr.title)
    }
}

fn ci_line(jobs: &[CiJob]) -> String {
    if jobs.is_empty() {
        return "no recipes".into();
    }
    jobs.iter()
        .map(|j| format!("{} {}", j.name, j.status.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn post_line(r: &PostReport) -> String {
    format!(
        "Posted {} note(s) ({} file-level fallback), {} failed, {} skipped",
        r.posted, r.fallback, r.failed, r.skipped
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_diff_text_drops_ignored_sections() {
        let raw = "diff --git a/a.py b/a.py\n--- a/a.py\n+++ b/a.py\n@@ -1 +1 @@\n-x\n+y\n\
diff --git a/yarn.lock b/yarn.lock\n--- a/yarn.lock\n+++ b/yarn.lock\n@@ -1 +1 @@\n-1\n+2\n";
        let keep: HashSet<String> = ["a.py".to_string()].into_iter().collect();
        let out = filter_diff_text(raw, &keep);
        assert!(out.contains("a/a.py"));
        assert!(!out.contains("yarn.lock"));
    }

    #[test]
    fn local_project_detection() {
        let p = Project {
            id: 1,
            forge: ForgeKind::Gitlab,
            base_url: LOCAL_BASE_URL.into(),
            remote_id: "/x".into(),
            name: "x".into(),
            local_path: "/x".into(),
            target_branches: vec![],
            mode: PostingMode::Draft,
            poll_interval_secs: 0,
            enabled: false,
        };
        assert!(is_local_project(&p));
    }
}
