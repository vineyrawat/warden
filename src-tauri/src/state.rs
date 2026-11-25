// OWNER: integration
//! Application state shared by commands and background tasks, plus startup and the background loops
//! (event forwarding, review queue, poller scheduler).
//!
//! Forge tokens live only in the macOS keychain (`warden_core::secrets`); they are read right before
//! a forge call, handed to the provider, and never logged or returned to the UI.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{broadcast, mpsc, Semaphore};

use warden_core::ci::CiRunner;
use warden_core::config::AppSettings;
use warden_core::db::Db;
use warden_core::diff::FileDiff;
use warden_core::engine::ReviewEngine;
use warden_core::forge::{provider_for, ForgeProvider};
use warden_core::mcp::{McpServer, RunRegistry};
use warden_core::pipeline::{is_local_project, ExecOutcome, Pipeline};
use warden_core::poller::Poller;
use warden_core::poster::Poster;
use warden_core::secrets;
use warden_core::worktree::WorktreeManager;
use warden_core::{AppEvent, EventBus, ForgeKind, Project, Result, RunStatus, WardenError};

/// Tauri event channel every `AppEvent` is forwarded on.
pub const EVENT_CHANNEL: &str = "warden://event";
/// `settings` table key listing forge hosts a token was saved for.
pub const FORGES_KV: &str = "forges";
/// How long a deferred (rate-limited) run waits before it is tried again.
const DEFER_RETRY: Duration = Duration::from_secs(60);
const MIN_POLL_SECS: u64 = 30;
pub const FORGE_STATUS_TTL: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ForgeHost {
    pub forge: ForgeKind,
    pub base_url: String,
}

/// Cached whoami per forge host (never holds the token).
#[derive(Debug, Clone)]
pub struct ForgeCheck {
    pub at: Instant,
    pub user: Option<String>,
    pub ok: bool,
}

pub struct AppState {
    pub db: Db,
    pub events: EventBus,
    pub registry: Arc<RunRegistry>,
    pub mcp: tokio::sync::Mutex<Option<McpServer>>,
    pub ci: Arc<CiRunner>,
    pub pipeline: Arc<Pipeline>,
    pub settings: RwLock<AppSettings>,
    pub data_dir: PathBuf,
    /// Run ids waiting for the review worker (one review at a time).
    pub queue_tx: mpsc::UnboundedSender<i64>,
    /// Poll requests: `None` = every enabled project now, `Some(id)` = that project now.
    pub poll_tx: mpsc::UnboundedSender<Option<i64>>,
    pub forge_checks: Mutex<HashMap<String, ForgeCheck>>,
    /// Diff files of finished runs (run id -> files) so `get_run` does not re-diff every time.
    pub files_cache: Mutex<HashMap<i64, Vec<FileDiff>>>,
}

pub type Shared = Arc<AppState>;

pub fn norm_base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

/// `~/Library/Application Support/Warden` (falls back to `~/.warden` without $HOME layout).
pub fn default_data_dir() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("Warden"),
        None => PathBuf::from(".warden"),
    }
}

impl AppState {
    pub fn settings(&self) -> AppSettings {
        self.settings.read().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn set_settings_cache(&self, s: &AppSettings) {
        if let Ok(mut w) = self.settings.write() {
            *w = s.clone();
        }
    }

    pub fn worktrees(&self) -> WorktreeManager {
        WorktreeManager::new(self.settings().worktree_root)
    }

    pub fn engine(&self) -> ReviewEngine {
        self.pipeline.engine(&self.settings())
    }

    pub fn poster(&self) -> Poster {
        Poster::new(self.events.clone())
    }

    pub fn notice(&self, level: &str, message: impl Into<String>) {
        self.events.emit(AppEvent::Notice {
            level: level.into(),
            message: message.into(),
        });
    }

    /// Provider for a project's forge with the token from the keychain; `None` for local
    /// projects or when no token is stored.
    pub fn provider(&self, project: &Project) -> Result<Option<Arc<dyn ForgeProvider>>> {
        if is_local_project(project) {
            return Ok(None);
        }
        let key = secrets::service_key(project.forge, &norm_base(&project.base_url));
        Ok(secrets::get_token(&key)?.map(|t| provider_for(project, t)))
    }

    pub fn require_provider(&self, project: &Project) -> Result<Arc<dyn ForgeProvider>> {
        self.provider(project)?.ok_or_else(|| {
            WardenError::Config(format!(
                "no {} token saved for {} (add it under Connections)",
                project.forge, project.base_url
            ))
        })
    }

    pub async fn known_forges(&self) -> Result<Vec<ForgeHost>> {
        let mut hosts: Vec<ForgeHost> = self
            .db
            .get_kv(FORGES_KV)
            .await?
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        for p in self.db.list_projects().await? {
            if is_local_project(&p) {
                continue;
            }
            let h = ForgeHost {
                forge: p.forge,
                base_url: norm_base(&p.base_url),
            };
            if !hosts.contains(&h) {
                hosts.push(h);
            }
        }
        Ok(hosts)
    }

    pub async fn remember_forge(&self, host: ForgeHost) -> Result<()> {
        let mut hosts: Vec<ForgeHost> = self
            .db
            .get_kv(FORGES_KV)
            .await?
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        if !hosts.contains(&host) {
            hosts.push(host);
            self.db
                .set_kv(FORGES_KV, &serde_json::to_value(&hosts)?)
                .await?;
        }
        Ok(())
    }

    /// Hands a queued run to the review worker.
    pub fn enqueue(&self, run_id: i64) {
        let _ = self.queue_tx.send(run_id);
    }

    pub fn request_poll(&self, project_id: Option<i64>) {
        let _ = self.poll_tx.send(project_id);
    }

    /// Executes one queued run (called by the worker under the review semaphore).
    async fn process_run(self: &Arc<Self>, run_id: i64) {
        let provider = match self.provider_for_run(run_id).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("run {run_id}: cannot build forge provider: {e}");
                None
            }
        };
        match self.pipeline.execute_queued(run_id, provider).await {
            Ok(ExecOutcome::Deferred) => {
                self.notice(
                    "info",
                    "Hourly review limit reached; queued review will start later",
                );
                let tx = self.queue_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(DEFER_RETRY).await;
                    let _ = tx.send(run_id);
                });
            }
            Ok(ExecOutcome::Finished(run)) => {
                tracing::info!("run {} finished with status {}", run.id, run.status);
            }
            Err(e) => {
                tracing::error!("run {run_id} could not execute: {e}");
                self.notice("error", format!("Review run {run_id} could not start: {e}"));
            }
        }
    }

    async fn provider_for_run(&self, run_id: i64) -> Result<Option<Arc<dyn ForgeProvider>>> {
        let run = self.db.get_run(run_id).await?;
        let mr = self.db.get_mr(run.mr_id).await?;
        let project = self.db.get_project(mr.project_id).await?;
        self.provider(&project)
    }

    /// Polls one project and queues the resulting reviews.
    async fn poll_project(
        self: &Arc<Self>,
        project: &Project,
        last_error: &mut HashMap<i64, String>,
    ) {
        let provider = match self.provider(project) {
            Ok(Some(p)) => p,
            Ok(None) => {
                let msg = format!(
                    "No {} token for {}; {} is not polled",
                    project.forge, project.base_url, project.name
                );
                if last_error.get(&project.id) != Some(&msg) {
                    self.notice("warn", msg.clone());
                    last_error.insert(project.id, msg);
                }
                return;
            }
            Err(e) => {
                tracing::warn!("keychain read failed for {}: {e}", project.name);
                return;
            }
        };
        let res = Poller::new()
            .poll_project(&self.db, provider.as_ref(), project)
            .await;
        match res {
            Ok(changes) => {
                last_error.remove(&project.id);
                match self.pipeline.handle_poll_changes(project, &changes).await {
                    Ok(ids) => ids.into_iter().for_each(|id| self.enqueue(id)),
                    Err(e) => self.notice("error", format!("Poll of {} failed: {e}", project.name)),
                }
                self.events.emit(AppEvent::MrsUpdated {
                    project_id: project.id,
                });
            }
            Err(e) => {
                let msg = format!("Poll of {} failed: {e}", project.name);
                if last_error.get(&project.id) != Some(&msg) {
                    self.notice("warn", msg.clone());
                    last_error.insert(project.id, msg);
                }
            }
        }
    }
}

/// Opens the DB, loads settings, starts the MCP server and builds the pipeline. Background loops
/// are started by [`spawn_background`].
pub async fn init() -> Result<(
    Shared,
    mpsc::UnboundedReceiver<i64>,
    mpsc::UnboundedReceiver<Option<i64>>,
)> {
    let data_dir = default_data_dir();
    std::fs::create_dir_all(&data_dir)?;
    let db = Db::open(&data_dir.join("warden.db")).await?;
    let mut settings = db.get_settings().await?;
    settings.data_dir = data_dir.to_string_lossy().to_string();

    let events = EventBus::new();
    let registry = Arc::new(RunRegistry::new());
    let mcp = match McpServer::start(settings.mcp_port, registry.clone()).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("{e}; falling back to a free port");
            McpServer::start(0, registry.clone()).await?
        }
    };
    if mcp.port() != settings.mcp_port {
        settings.mcp_port = mcp.port();
    }
    db.save_settings(&settings).await?;

    let ci = Arc::new(Pipeline::ci_runner(
        &db,
        events.clone(),
        data_dir.join("logs").join("ci"),
    ));
    let pipeline = Arc::new(Pipeline::new(
        db.clone(),
        events.clone(),
        registry.clone(),
        ci.clone(),
        mcp.url(),
    ));
    let (queue_tx, queue_rx) = mpsc::unbounded_channel();
    let (poll_tx, poll_rx) = mpsc::unbounded_channel();
    let state = Arc::new(AppState {
        db,
        events,
        registry,
        mcp: tokio::sync::Mutex::new(Some(mcp)),
        ci,
        pipeline,
        settings: RwLock::new(settings),
        data_dir,
        queue_tx,
        poll_tx,
        forge_checks: Mutex::new(HashMap::new()),
        files_cache: Mutex::new(HashMap::new()),
    });
    Ok((state, queue_rx, poll_rx))
}

/// Starts: event forwarding (+ notifications), the review worker, the poller scheduler, and
/// recovery of runs left behind by a previous session.
pub fn spawn_background(
    app: AppHandle,
    state: Shared,
    queue_rx: mpsc::UnboundedReceiver<i64>,
    poll_rx: mpsc::UnboundedReceiver<Option<i64>>,
) {
    tauri::async_runtime::spawn(forward_events(app, state.clone()));
    tauri::async_runtime::spawn(review_worker(state.clone(), queue_rx));
    tauri::async_runtime::spawn(poll_scheduler(state.clone(), poll_rx));
    tauri::async_runtime::spawn(recover_runs(state));
}

async fn forward_events(app: AppHandle, state: Shared) {
    let mut rx = state.events.subscribe();
    let mut notified: HashSet<(i64, RunStatus)> = HashSet::new();
    loop {
        match rx.recv().await {
            Ok(ev) => {
                if let AppEvent::RunUpdated { run } = &ev {
                    if matches!(run.status, RunStatus::AwaitingApproval | RunStatus::Failed)
                        && notified.insert((run.id, run.status))
                    {
                        notify_run(&app, &state, run).await;
                    }
                }
                if let Err(e) = app.emit(EVENT_CHANNEL, &ev) {
                    tracing::warn!("emit failed: {e}");
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("event forwarder lagged by {n} events");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn notify_run(app: &AppHandle, state: &Shared, run: &warden_core::ReviewRun) {
    let label = match state.db.get_mr(run.mr_id).await {
        Ok(mr) if mr.iid > 0 => format!("!{} {}", mr.iid, mr.title),
        Ok(mr) => mr.title,
        Err(_) => format!("run {}", run.id),
    };
    let (title, body) = match run.status {
        RunStatus::AwaitingApproval => {
            let n = state
                .db
                .list_findings(run.id)
                .await
                .map(|f| {
                    f.iter()
                        .filter(|f| f.status == warden_core::FindingStatus::Draft)
                        .count()
                })
                .unwrap_or(0);
            (
                "Review ready for approval".to_string(),
                format!("{label}: {n} finding(s) waiting"),
            )
        }
        _ => (
            "Review failed".to_string(),
            format!(
                "{label}: {}",
                run.error.clone().unwrap_or_else(|| "unknown error".into())
            ),
        ),
    };
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!("notification failed: {e}");
    }
}

async fn review_worker(state: Shared, mut rx: mpsc::UnboundedReceiver<i64>) {
    let sem = Arc::new(Semaphore::new(1));
    while let Some(run_id) = rx.recv().await {
        let Ok(permit) = sem.clone().acquire_owned().await else {
            break;
        };
        let st = state.clone();
        tauri::async_runtime::spawn(async move {
            let _permit = permit;
            st.process_run(run_id).await;
        });
    }
}

async fn poll_scheduler(state: Shared, mut rx: mpsc::UnboundedReceiver<Option<i64>>) {
    let mut next_due: HashMap<i64, Instant> = HashMap::new();
    let mut last_error: HashMap<i64, String> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        let (force_all, force_one) = tokio::select! {
            _ = tick.tick() => (false, None),
            req = rx.recv() => match req {
                Some(None) => (true, None),
                Some(Some(id)) => (false, Some(id)),
                None => break,
            },
        };
        let projects = match state.db.list_projects().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("list projects failed: {e}");
                continue;
            }
        };
        let now = Instant::now();
        for p in projects
            .iter()
            .filter(|p| p.enabled && !is_local_project(p))
        {
            let due = force_all
                || force_one == Some(p.id)
                || next_due.get(&p.id).is_none_or(|t| now >= *t);
            if !due {
                continue;
            }
            let every = Duration::from_secs(u64::from(p.poll_interval_secs).max(MIN_POLL_SECS));
            next_due.insert(p.id, now + every);
            state.poll_project(p, &mut last_error).await;
        }
    }
}

/// Runs interrupted mid-flight by a previous session become Failed; queued ones are re-queued.
async fn recover_runs(state: Shared) {
    let in_flight = [
        RunStatus::Preparing,
        RunStatus::Checking,
        RunStatus::Reviewing,
        RunStatus::Filtering,
        RunStatus::Posting,
    ];
    if let Ok(runs) = state.db.runs_with_status(&in_flight).await {
        for mut run in runs {
            run.status = RunStatus::Failed;
            run.error =
                Some("interrupted: Warden was closed while this run was in progress".into());
            run.ended_at = Some(warden_core::db::now_ts());
            let _ = state.db.update_run(&run).await;
        }
    }
    if let Ok(runs) = state.db.runs_with_status(&[RunStatus::Queued]).await {
        for run in runs {
            state.enqueue(run.id);
        }
    }
    state.request_poll(None);
}
