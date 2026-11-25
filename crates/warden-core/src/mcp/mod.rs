// OWNER: engine
//! rmcp streamable-HTTP MCP server bound to 127.0.0.1. Each review run is registered under a random
//! token; Claude's MCP config sends it as `X-Warden-Run: <token>`. Requests without a known token are
//! rejected. Claude never gets a forge token or any tool that writes to the forge (hard rule 1).
//!
//! Two layers check the token:
//! 1. an axum middleware in front of `/mcp` answers `401` to any HTTP request whose
//!    `X-Warden-Run` header is missing or unknown (so an unknown client cannot even initialize);
//! 2. every tool resolves its [`RunContext`] from the same header of the HTTP request that carried
//!    the `tools/call` (rmcp injects `http::request::Parts` into the request extensions).

pub mod tools;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::sync::mpsc;

use crate::ci::{CiRunner, RecipeContext};
use crate::config::{RecipeConfig, WardenConfig};
use crate::diff::FileDiff;
use crate::error::{Result, WardenError};
use crate::types::{CiJob, MergeRequest, NewFinding};

pub use tools::WardenTools;

/// HTTP header carrying the run token (header names are case-insensitive).
pub const RUN_HEADER: &str = "x-warden-run";

/// Validates a finding submitted through `review_add_finding`. `Err(reason)` rejects it and the
/// reason is returned to Claude so it can fix the finding. Integration wires this to
/// `guardrails::validate_new_finding` (keep = Ok, else Err(reason)).
pub type FindingValidator =
    Arc<dyn Fn(&NewFinding) -> std::result::Result<(), String> + Send + Sync>;

/// Redacts secrets from text before it is shown to Claude (file contents, search hits, CI
/// excerpts). Integration wires this to `|s| checks::redact_secrets(s).0`.
pub type Redactor = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Something that can re-run one CI recipe for `ci_run_recipe`. Implemented for [`CiRunner`];
/// tests (and integration, if it wants to wrap the runner with DB writes) can supply their own.
#[async_trait]
pub trait CiRerun: Send + Sync {
    async fn rerun(
        &self,
        run_id: i64,
        worktree: &Path,
        recipe: &RecipeConfig,
        ctx: &RecipeContext,
    ) -> Result<CiJob>;
}

#[async_trait]
impl CiRerun for CiRunner {
    async fn rerun(
        &self,
        run_id: i64,
        worktree: &Path,
        recipe: &RecipeConfig,
        ctx: &RecipeContext,
    ) -> Result<CiJob> {
        self.run_one(run_id, worktree, recipe, ctx).await
    }
}

/// What Claude handed in with `review_submit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub summary: String,
    /// `approve` | `comment` | `request_changes`
    pub verdict: String,
}

/// Mutable per-run state shared between tool calls (and readable by the pipeline).
#[derive(Debug, Default, Clone)]
pub struct RunState {
    /// Number of `ci_run_recipe` reruns started in this run (max [`tools::MAX_CI_RERUNS`]).
    pub ci_reruns: u32,
    /// Jobs produced by reruns (latest last). `ci_get_results` shows these over `ci_jobs`.
    pub rerun_jobs: Vec<CiJob>,
    pub accepted: u32,
    pub rejected: u32,
    /// Dedupe keys of accepted findings (file + normalized title + rule id).
    pub accepted_keys: HashSet<String>,
    pub submission: Option<Submission>,
    /// Tool calls served for this run (for audit / progress).
    pub tool_calls: u32,
}

/// Everything the MCP tools may see for one run.
#[derive(Clone)]
pub struct RunContext {
    pub run_id: i64,
    pub worktree: PathBuf,
    /// Unified diff with secrets redacted.
    pub diff_text: String,
    pub files: Vec<FileDiff>,
    pub mr: MergeRequest,
    /// Bodies (or short renderings) of existing MR discussions.
    pub existing_discussions: Vec<String>,
    pub ci_jobs: Vec<CiJob>,
    pub check_findings: Vec<NewFinding>,
    /// Accepted findings from `review_add_finding` are forwarded here (pipeline stores them).
    pub findings_tx: mpsc::Sender<NewFinding>,
    pub ci_runner: Option<Arc<CiRunner>>,
    pub cfg: WardenConfig,
    /// Scaffold addition (not in SPEC's list): fingerprints already stored for this MR, so
    /// `review_add_finding` can run the duplicate check of `guardrails::validate_new_finding`.
    pub existing_fingerprints: HashSet<String>,
    /// Engine addition: guardrail hook run by `review_add_finding` after the built-in schema
    /// checks. `None` = only the built-in checks.
    pub validator: Option<FindingValidator>,
    /// Engine addition: secret redaction for text shown to Claude. `None` = no redaction.
    pub redactor: Option<Redactor>,
    /// Engine addition: overrides `ci_runner` for `ci_run_recipe` when set.
    pub ci_rerun: Option<Arc<dyn CiRerun>>,
    /// Engine addition: mutable state (reruns, accepted findings, submission).
    pub state: Arc<Mutex<RunState>>,
}

impl RunContext {
    /// A context with empty diff/discussions/CI and no hooks; set the remaining pub fields directly.
    pub fn new(
        run_id: i64,
        worktree: impl Into<PathBuf>,
        mr: MergeRequest,
        cfg: WardenConfig,
        findings_tx: mpsc::Sender<NewFinding>,
    ) -> Self {
        Self {
            run_id,
            worktree: worktree.into(),
            diff_text: String::new(),
            files: Vec::new(),
            mr,
            existing_discussions: Vec::new(),
            ci_jobs: Vec::new(),
            check_findings: Vec::new(),
            findings_tx,
            ci_runner: None,
            cfg,
            existing_fingerprints: HashSet::new(),
            validator: None,
            redactor: None,
            ci_rerun: None,
            state: Arc::new(Mutex::new(RunState::default())),
        }
    }

    /// Applies the redactor (if any).
    pub fn redact(&self, text: &str) -> String {
        match &self.redactor {
            Some(r) => r(text),
            None => text.to_string(),
        }
    }

    /// Snapshot of the mutable state.
    pub fn state_snapshot(&self) -> RunState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Claude's `review_submit`, if it was called.
    pub fn submission(&self) -> Option<Submission> {
        self.state.lock().ok().and_then(|s| s.submission.clone())
    }
}

#[derive(Default)]
pub struct RunRegistry {
    pub runs: RwLock<HashMap<String, Arc<RunContext>>>,
}

impl RunRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stores the context under a fresh random token and returns the token.
    pub fn register(&self, ctx: RunContext) -> String {
        // 2 x 122 random bits; hex only, so it is safe in a header and a file name.
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut runs = self.runs.write().unwrap_or_else(|e| e.into_inner());
        runs.insert(token.clone(), Arc::new(ctx));
        token
    }

    pub fn remove(&self, token: &str) {
        let mut runs = self.runs.write().unwrap_or_else(|e| e.into_inner());
        runs.remove(token);
    }

    pub fn get(&self, token: &str) -> Option<Arc<RunContext>> {
        if token.is_empty() {
            return None;
        }
        let runs = self.runs.read().unwrap_or_else(|e| e.into_inner());
        runs.get(token).cloned()
    }

    /// Claude's `review_submit` for the run, if any (works until `remove`).
    pub fn submission(&self, token: &str) -> Option<Submission> {
        self.get(token).and_then(|c| c.submission())
    }

    pub fn len(&self) -> usize {
        self.runs.read().map(|r| r.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct McpServer {
    pub addr: SocketAddr,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl McpServer {
    /// Binds 127.0.0.1:<port> (port 0 = random free port) and serves the MCP endpoint at `/mcp`.
    pub async fn start(port: u16, registry: Arc<RunRegistry>) -> Result<McpServer> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| WardenError::Engine(format!("mcp: cannot bind 127.0.0.1:{port}: {e}")))?;
        let addr = listener.local_addr()?;

        let config = StreamableHttpServerConfig::default().with_allowed_hosts([
            "127.0.0.1".to_string(),
            format!("127.0.0.1:{}", addr.port()),
            "localhost".to_string(),
            format!("localhost:{}", addr.port()),
        ]);
        let cancel = config.cancellation_token.clone();

        let factory_registry = registry.clone();
        let service: StreamableHttpService<WardenTools, LocalSessionManager> =
            StreamableHttpService::new(
                move || Ok(WardenTools::new(factory_registry.clone())),
                Default::default(),
                config,
            );

        let router = axum::Router::new().nest_service("/mcp", service).layer(
            axum::middleware::from_fn_with_state(registry.clone(), require_run_token),
        );

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let shutdown = async move {
                let _ = shutdown_rx.await;
                // Terminates open sessions / SSE streams so graceful shutdown can finish.
                cancel.cancel();
            };
            if let Err(e) = axum::serve(listener, router)
                .with_graceful_shutdown(shutdown)
                .await
            {
                tracing::warn!("mcp server stopped with error: {e}");
            }
        });

        tracing::info!("warden mcp server listening on http://{addr}/mcp");
        Ok(McpServer {
            addr,
            shutdown_tx: Some(shutdown_tx),
            task: Some(task),
        })
    }

    /// e.g. `http://127.0.0.1:7811/mcp`
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.addr.port())
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(mut task) = self.task.take() {
            if tokio::time::timeout(std::time::Duration::from_secs(3), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Rejects every HTTP request whose `X-Warden-Run` header is missing or not a registered run.
async fn require_run_token(
    State(registry): State<Arc<RunRegistry>>,
    req: Request,
    next: Next,
) -> Response {
    let token = req
        .headers()
        .get(RUN_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .unwrap_or("");
    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "missing X-Warden-Run header").into_response();
    }
    if registry.get(token).is_none() {
        return (StatusCode::UNAUTHORIZED, "unknown X-Warden-Run token").into_response();
    }
    next.run(req).await
}
