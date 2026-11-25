// OWNER: ci
//! Local CI recipe runner: parallel jobs, per-job timeout, live log streaming (AppEvent::CiLog),
//! log files under `log_dir`, failure excerpts. Commands starting with "bench " take a per-bench mutex
//! (bench root = nearest ancestor dir containing "sites/").
//!
//! ## Job ids
//! `CiJob.id` is assigned by the database, which this module does not touch. Two modes:
//! * **default** — every returned `CiJob` has `id = 0` and `AppEvent::CiLog`/`CiJobUpdated` carry
//!   `job_id = 0`; the caller inserts the returned jobs with `Db::insert_ci_job`.
//! * **allocator** — [`CiRunner::with_job_id_allocator`] installs an async callback that receives the
//!   job (status `Running`, or its final status for skipped / not-installed jobs) *before* it runs and
//!   returns its id (typically `db.insert_ci_job(&job)`). The id is then used in every event and in the
//!   returned job; the caller should `Db::update_ci_job` the returned (final) jobs.
//!
//! ## Execution
//! Each recipe runs as `<shell> <rendered command>` (`/bin/zsh -lc` when available, else `/bin/sh -c`)
//! in `worktree.join(recipe.cwd)`, with a clean environment (`PATH`, `HOME`, `USER`, `LANG`, `SHELL`
//! from Warden's own environment + the recipe's `env`), in its own process group so a timeout or
//! [`CiRunner::cancel`] kills the whole tree. stdout and stderr lines are written to
//! `<log_dir>/<run_id>/<job>.log` and emitted as `AppEvent::CiLog`.

pub mod gitlab_ci_local;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::config::{CiConfig, RecipeConfig};
use crate::error::{Result, WardenError};
use crate::events::EventBus;
use crate::types::{AppEvent, CiJob, CiStatus};

/// Per-job timeout when the recipe has none (or an unparsable one).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Max lines in a failure excerpt.
pub const EXCERPT_LINES: usize = 40;
/// Lines kept before the first error-looking line in an excerpt.
const EXCERPT_BEFORE: usize = 10;
/// Lines longer than this are truncated in events and excerpts (the log file keeps them whole).
const MAX_EVENT_LINE: usize = 4000;
/// After SIGTERM, how long the process group gets before SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(2);
/// After the shell exits, how long we keep reading pipes held open by stray background children.
const PIPE_GRACE: Duration = Duration::from_secs(3);

/// Reason recorded on jobs skipped because their file-list placeholders are empty.
pub const SKIP_NO_FILES: &str = "no matching files";

const FILE_PLACEHOLDERS: [&str; 3] = ["changed_files", "changed_py", "changed_ts"];
const TS_EXTS: [&str; 5] = ["ts", "tsx", "js", "jsx", "vue"];

static ERROR_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)error|fail|exception|traceback").expect("static regex"));

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipeContext {
    pub changed_files: Vec<String>,
    pub base_sha: String,
    pub head_sha: String,
}

/// Async job-id allocator, see the module docs.
pub type JobIdAllocator =
    Arc<dyn Fn(CiJob) -> Pin<Box<dyn Future<Output = Result<i64>> + Send>> + Send + Sync>;

/// Cancel signal shared by every job of one run, with a count of the jobs/runs holding it.
#[derive(Debug)]
pub struct RunCancel {
    tx: watch::Sender<bool>,
    active: usize,
}

#[derive(Clone)]
pub struct CiRunner {
    pub events: EventBus,
    pub log_dir: PathBuf,
    /// bench root -> lock serializing `bench ...` commands on that bench.
    pub bench_locks: Arc<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>>,
    /// run_id -> cancel signal for the jobs of that run currently running.
    pub cancels: Arc<Mutex<HashMap<i64, RunCancel>>>,
    /// Program + args the rendered command is appended to (`["/bin/zsh", "-lc"]` by default).
    pub shell: Vec<String>,
    /// Environment every job starts from (the environment is otherwise cleared).
    pub base_env: Vec<(String, String)>,
    pub default_timeout: Duration,
    pub job_id_allocator: Option<JobIdAllocator>,
}

impl std::fmt::Debug for CiRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CiRunner")
            .field("log_dir", &self.log_dir)
            .field("shell", &self.shell)
            .field("default_timeout", &self.default_timeout)
            .field("job_id_allocator", &self.job_id_allocator.is_some())
            .finish()
    }
}

impl CiRunner {
    pub fn new(events: EventBus, log_dir: impl Into<PathBuf>) -> Self {
        Self {
            events,
            log_dir: log_dir.into(),
            bench_locks: Arc::new(Mutex::new(HashMap::new())),
            cancels: Arc::new(Mutex::new(HashMap::new())),
            shell: default_shell(),
            base_env: default_env(),
            default_timeout: DEFAULT_TIMEOUT,
            job_id_allocator: None,
        }
    }

    /// Replace the shell prefix, e.g. `vec!["/bin/sh".into(), "-c".into()]`.
    pub fn with_shell(mut self, shell: Vec<String>) -> Self {
        if !shell.is_empty() {
            self.shell = shell;
        }
        self
    }

    /// Set (or override) one variable of the base environment, e.g. `PATH`.
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let key = key.into();
        self.base_env.retain(|(k, _)| *k != key);
        self.base_env.push((key, value.into()));
        self
    }

    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Install the job-id allocator (see the module docs).
    pub fn with_job_id_allocator<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(CiJob) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<i64>> + Send + 'static,
    {
        self.job_id_allocator = Some(Arc::new(move |job| Box::pin(f(job))));
        self
    }

    /// Runs all recipes in parallel. Returned jobs are in recipe order. A recipe that cannot even be
    /// set up becomes a `Failed` job; this only errors if nothing could run at all.
    pub async fn run_all(
        &self,
        run_id: i64,
        worktree: &Path,
        cfg: &CiConfig,
        ctx: &RecipeContext,
    ) -> Result<Vec<CiJob>> {
        if cfg.recipes.is_empty() {
            return Ok(Vec::new());
        }
        // Keep the run's cancel signal registered for the whole batch.
        let _guard = RunGuard::new(&self.cancels, run_id);
        let mut set = tokio::task::JoinSet::new();
        for (idx, recipe) in cfg.recipes.iter().enumerate() {
            let this = self.clone();
            let worktree = worktree.to_path_buf();
            let recipe = recipe.clone();
            let ctx = ctx.clone();
            set.spawn(async move {
                let res = this.run_one(run_id, &worktree, &recipe, &ctx).await;
                (idx, res)
            });
        }
        let mut out: Vec<Option<CiJob>> = vec![None; cfg.recipes.len()];
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((idx, Ok(job))) => out[idx] = Some(job),
                Ok((idx, Err(e))) => {
                    out[idx] = Some(self.setup_failure(run_id, &cfg.recipes[idx], &e.to_string()))
                }
                Err(e) => tracing::error!("ci job task failed: {e}"),
            }
        }
        Ok(out
            .into_iter()
            .enumerate()
            .map(|(idx, j)| {
                j.unwrap_or_else(|| {
                    self.setup_failure(run_id, &cfg.recipes[idx], "job task panicked")
                })
            })
            .collect())
    }

    /// Runs one recipe. Recipe-level problems (bad cwd, command failure, missing tool) are reported
    /// in the returned job's status; `Err` only for log-file I/O errors.
    pub async fn run_one(
        &self,
        run_id: i64,
        worktree: &Path,
        recipe: &RecipeConfig,
        ctx: &RecipeContext,
    ) -> Result<CiJob> {
        let guard = RunGuard::new(&self.cancels, run_id);
        let mut cancel_rx = guard.rx.clone();
        let log_path = self.log_file_path(run_id, &recipe.name).await?;
        let mut job = CiJob {
            id: 0,
            run_id,
            name: recipe.name.clone(),
            kind: "recipe".into(),
            command: String::new(),
            status: CiStatus::Running,
            duration_ms: 0,
            log_path: log_path.to_string_lossy().into_owned(),
            failure_excerpt: None,
        };
        let started = Instant::now();

        // ---- resolve the command (or an early final status) ----
        let cwd = match resolve_cwd(worktree, recipe.cwd.as_deref()) {
            Ok(c) => c,
            Err(msg) => {
                job.command = recipe.run.clone().unwrap_or_default();
                return self
                    .finish_early(job, CiStatus::Failed, &msg, &log_path)
                    .await;
            }
        };
        if gitlab_ci_local::is_adapter(recipe) {
            job.kind = "gitlab_ci_local".into();
            job.command = gitlab_ci_local::display_command(recipe);
            if recipe.jobs.iter().all(|j| j.trim().is_empty()) {
                return self
                    .finish_early(
                        job,
                        CiStatus::Skipped,
                        "no gitlab-ci-local jobs listed",
                        &log_path,
                    )
                    .await;
            }
            let path_env = self.env_value("PATH").map(std::ffi::OsString::from);
            match gitlab_ci_local::detect(path_env.as_deref(), &cwd).await {
                Some(program) => {
                    job.command =
                        gitlab_ci_local::command_for(recipe, &program).unwrap_or_default();
                }
                None => {
                    return self
                        .finish_early(
                            job,
                            CiStatus::NotInstalled,
                            gitlab_ci_local::INSTALL_HINT,
                            &log_path,
                        )
                        .await;
                }
            }
        } else if let Some(adapter) = recipe.use_.as_deref().filter(|u| !u.trim().is_empty()) {
            job.kind = adapter.replace('-', "_");
            job.command = adapter.to_string();
            let msg = format!("unsupported adapter `{adapter}` (supported: gitlab-ci-local)");
            return self
                .finish_early(job, CiStatus::Skipped, &msg, &log_path)
                .await;
        } else if let Some(template) = recipe.run.as_deref().filter(|r| !r.trim().is_empty()) {
            job.command = render_command(template, ctx);
            if let Some(reason) = skip_reason(template, ctx) {
                return self
                    .finish_early(job, CiStatus::Skipped, &reason, &log_path)
                    .await;
            }
        } else {
            return self
                .finish_early(
                    job,
                    CiStatus::Failed,
                    "recipe has neither `run` nor `use`",
                    &log_path,
                )
                .await;
        }

        let timeout = match recipe.timeout.as_deref().filter(|t| !t.trim().is_empty()) {
            None => self.default_timeout,
            Some(t) => parse_timeout(t).unwrap_or(self.default_timeout),
        };

        job.id = self.allocate_id(&job).await;
        self.events
            .emit(AppEvent::CiJobUpdated { job: job.clone() });

        let mut log = tokio::fs::File::create(&log_path).await?;
        log.write_all(format!("$ {}\n", job.command).as_bytes())
            .await?;
        if let Some(t) = recipe.timeout.as_deref() {
            if parse_timeout(t).is_none() {
                let warn = format!(
                    "[warden] invalid timeout {t:?}, using {}s\n",
                    timeout.as_secs()
                );
                log.write_all(warn.as_bytes()).await?;
            }
        }

        // ---- bench lock (serializes `bench ...` on the same bench) ----
        let bench_lock = if job.command.trim_start().starts_with("bench ") {
            bench_root(&cwd).map(|root| self.bench_lock(root))
        } else {
            None
        };
        let _bench_guard = match bench_lock {
            Some(lock) => {
                tokio::select! {
                    g = lock.lock_owned() => Some(g),
                    _ = wait_cancel(&mut cancel_rx) => {
                        return self.finish_run(job, started, Outcome::Cancelled, None, &mut log).await;
                    }
                }
            }
            None => None,
        };
        if *cancel_rx.borrow() {
            return self
                .finish_run(job, started, Outcome::Cancelled, None, &mut log)
                .await;
        }

        // ---- spawn ----
        let mut cmd = Command::new(&self.shell[0]);
        cmd.args(&self.shell[1..])
            .arg(&job.command)
            .current_dir(&cwd)
            .env_clear()
            .envs(self.base_env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .envs(recipe.env.iter())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("failed to start {}: {e}", self.shell[0]);
                log.write_all(format!("[warden] {msg}\n").as_bytes())
                    .await?;
                return self
                    .finish_run(job, started, Outcome::Exited(None), Some(msg), &mut log)
                    .await;
            }
        };
        let pgid = child.id().map(|p| p as i32);

        let (line_tx, mut line_rx) = mpsc::channel::<Vec<u8>>(1024);
        if let Some(out) = child.stdout.take() {
            tokio::spawn(read_lines(out, line_tx.clone()));
        }
        if let Some(err) = child.stderr.take() {
            tokio::spawn(read_lines(err, line_tx.clone()));
        }
        drop(line_tx);

        let mut excerpt = ExcerptBuilder::new();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        let pipe_grace = tokio::time::sleep(Duration::from_secs(365 * 24 * 3600));
        tokio::pin!(pipe_grace);
        let mut exit: Option<Option<i32>> = None;
        let mut pipes_open = true;
        let mut outcome: Option<Outcome> = None;

        loop {
            if exit.is_some() && !pipes_open {
                break;
            }
            tokio::select! {
                biased;
                _ = wait_cancel(&mut cancel_rx) => {
                    outcome = Some(Outcome::Cancelled);
                    break;
                }
                _ = &mut deadline => {
                    outcome = Some(Outcome::TimedOut(timeout));
                    break;
                }
                line = line_rx.recv(), if pipes_open => match line {
                    Some(raw) => self.handle_line(&job, &raw, &mut log, &mut excerpt).await,
                    None => pipes_open = false,
                },
                st = child.wait(), if exit.is_none() => {
                    exit = Some(st.ok().and_then(|s| s.code()));
                    pipe_grace.as_mut().reset(tokio::time::Instant::now() + PIPE_GRACE);
                }
                _ = &mut pipe_grace, if exit.is_some() && pipes_open => {
                    // the shell is gone but a background child holds the pipes: stop it.
                    break;
                }
            }
        }

        let killed = outcome.is_some() || pipes_open;
        if killed {
            if let Some(pgid) = pgid {
                kill_group(pgid, &mut child).await;
            }
            // Drain whatever is still buffered (bounded wait: the group is dead).
            let drain_until = tokio::time::Instant::now() + KILL_GRACE;
            while let Ok(Some(raw)) = tokio::time::timeout_at(drain_until, line_rx.recv()).await {
                self.handle_line(&job, &raw, &mut log, &mut excerpt).await;
            }
            if exit.is_none() {
                exit = Some(
                    tokio::time::timeout(KILL_GRACE, child.wait())
                        .await
                        .ok()
                        .and_then(|r| r.ok())
                        .and_then(|s| s.code()),
                );
            }
        }
        let outcome = outcome.unwrap_or(Outcome::Exited(exit.flatten()));
        let excerpt_text = excerpt.finish();
        self.finish_run(job, started, outcome, excerpt_text, &mut log)
            .await
    }

    /// Kills every running job of `run_id` (jobs report `Cancelled`). No-op if nothing runs.
    pub fn cancel(&self, run_id: i64) {
        let map = self.cancels.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(rc) = map.get(&run_id) {
            rc.tx.send_replace(true);
        }
    }

    // ------------------------------------------------------------------------------------------

    fn env_value(&self, key: &str) -> Option<String> {
        self.base_env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    fn bench_lock(&self, root: PathBuf) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.bench_locks.lock().unwrap_or_else(|p| p.into_inner());
        map.entry(root).or_default().clone()
    }

    async fn allocate_id(&self, job: &CiJob) -> i64 {
        match &self.job_id_allocator {
            None => 0,
            Some(alloc) => match alloc(job.clone()).await {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!("ci job id allocation failed for {}: {e}", job.name);
                    0
                }
            },
        }
    }

    /// `<log_dir>/<run_id>/<job>.log`, with `-2`, `-3`... appended when a log already exists (reruns).
    async fn log_file_path(&self, run_id: i64, name: &str) -> Result<PathBuf> {
        let dir = self.log_dir.join(run_id.to_string());
        tokio::fs::create_dir_all(&dir).await?;
        let base = sanitize_job_name(name);
        let mut candidate = dir.join(format!("{base}.log"));
        let mut n = 2;
        loop {
            match tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
                .await
            {
                Ok(_) => return Ok(candidate),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    candidate = dir.join(format!("{base}-{n}.log"));
                    n += 1;
                    if n > 10_000 {
                        return Err(WardenError::Other(format!("too many logs for job {name}")));
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    async fn handle_line(
        &self,
        job: &CiJob,
        raw: &[u8],
        log: &mut tokio::fs::File,
        excerpt: &mut ExcerptBuilder,
    ) {
        let _ = log.write_all(raw).await;
        let _ = log.write_all(b"\n").await;
        let mut line = String::from_utf8_lossy(raw).into_owned();
        if line.len() > MAX_EVENT_LINE {
            let mut cut = MAX_EVENT_LINE;
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            line.truncate(cut);
            line.push_str(" …");
        }
        excerpt.push(&line);
        self.events.emit(AppEvent::CiLog {
            run_id: job.run_id,
            job_id: job.id,
            line,
        });
    }

    /// Final status without running anything (skipped / not installed / misconfigured).
    async fn finish_early(
        &self,
        mut job: CiJob,
        status: CiStatus,
        message: &str,
        log_path: &Path,
    ) -> Result<CiJob> {
        job.status = status;
        job.failure_excerpt = Some(message.to_string());
        job.id = self.allocate_id(&job).await;
        let mut body = String::new();
        if !job.command.is_empty() {
            body.push_str(&format!("$ {}\n", job.command));
        }
        body.push_str(&format!("[warden] {}: {message}\n", status.as_str()));
        tokio::fs::write(log_path, body).await?;
        self.events.emit(AppEvent::CiLog {
            run_id: job.run_id,
            job_id: job.id,
            line: format!("[warden] {}: {message}", status.as_str()),
        });
        self.events
            .emit(AppEvent::CiJobUpdated { job: job.clone() });
        Ok(job)
    }

    async fn finish_run(
        &self,
        mut job: CiJob,
        started: Instant,
        outcome: Outcome,
        excerpt: Option<String>,
        log: &mut tokio::fs::File,
    ) -> Result<CiJob> {
        job.duration_ms = started.elapsed().as_millis() as u64;
        let (status, footer) = match outcome {
            Outcome::Exited(Some(0)) => (CiStatus::Passed, "exit status 0".to_string()),
            Outcome::Exited(Some(c)) => (CiStatus::Failed, format!("exit status {c}")),
            Outcome::Exited(None) => (
                CiStatus::Failed,
                "terminated without exit status".to_string(),
            ),
            Outcome::TimedOut(t) => (
                CiStatus::TimedOut,
                format!("timed out after {}", fmt_duration(t)),
            ),
            Outcome::Cancelled => (CiStatus::Cancelled, "cancelled".to_string()),
        };
        job.status = status;
        job.failure_excerpt = match status {
            CiStatus::Failed => Some(excerpt.unwrap_or_else(|| format!("[warden] {footer}"))),
            CiStatus::TimedOut => Some(match excerpt {
                Some(e) => format!("[warden] {footer}\n{e}"),
                None => format!("[warden] {footer}"),
            }),
            _ => None,
        };
        let footer_line = format!("[warden] {footer} ({} ms)", job.duration_ms);
        let _ = log.write_all(format!("{footer_line}\n").as_bytes()).await;
        let _ = log.flush().await;
        self.events.emit(AppEvent::CiLog {
            run_id: job.run_id,
            job_id: job.id,
            line: footer_line,
        });
        self.events
            .emit(AppEvent::CiJobUpdated { job: job.clone() });
        Ok(job)
    }

    fn setup_failure(&self, run_id: i64, recipe: &RecipeConfig, msg: &str) -> CiJob {
        let job = CiJob {
            id: 0,
            run_id,
            name: recipe.name.clone(),
            kind: if gitlab_ci_local::is_adapter(recipe) {
                "gitlab_ci_local".into()
            } else {
                "recipe".into()
            },
            command: recipe.run.clone().unwrap_or_default(),
            status: CiStatus::Failed,
            duration_ms: 0,
            log_path: String::new(),
            failure_excerpt: Some(format!("[warden] {msg}")),
        };
        self.events
            .emit(AppEvent::CiJobUpdated { job: job.clone() });
        job
    }
}

#[derive(Debug, Clone, Copy)]
enum Outcome {
    Exited(Option<i32>),
    TimedOut(Duration),
    Cancelled,
}

/// Registers a job (or a batch) under its run's cancel signal; deregisters on drop.
struct RunGuard {
    map: Arc<Mutex<HashMap<i64, RunCancel>>>,
    run_id: i64,
    rx: watch::Receiver<bool>,
}

impl RunGuard {
    fn new(map: &Arc<Mutex<HashMap<i64, RunCancel>>>, run_id: i64) -> Self {
        let mut m = map.lock().unwrap_or_else(|p| p.into_inner());
        let entry = m.entry(run_id).or_insert_with(|| RunCancel {
            tx: watch::channel(false).0,
            active: 0,
        });
        entry.active += 1;
        let rx = entry.tx.subscribe();
        drop(m);
        Self {
            map: map.clone(),
            run_id,
            rx,
        }
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        let mut m = self.map.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(e) = m.get_mut(&self.run_id) {
            e.active = e.active.saturating_sub(1);
            if e.active == 0 {
                m.remove(&self.run_id);
            }
        }
    }
}

/// Resolves when the run is cancelled; never resolves if the sender is gone.
async fn wait_cancel(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|v| *v).await.is_err() {
        std::future::pending::<()>().await;
    }
}

async fn read_lines<R: AsyncRead + Unpin>(r: R, tx: mpsc::Sender<Vec<u8>>) {
    let mut reader = BufReader::new(r);
    let mut buf = Vec::with_capacity(256);
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
                    buf.pop();
                }
                if tx.send(buf.clone()).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// SIGTERM the process group, then SIGKILL if anything is still alive after a grace period.
async fn kill_group(pgid: i32, child: &mut tokio::process::Child) {
    // SAFETY: plain syscall; pgid is the child's own process group (spawned with process_group(0)).
    unsafe {
        libc::killpg(pgid, libc::SIGTERM);
    }
    let _ = tokio::time::timeout(KILL_GRACE, child.wait()).await;
    // Background children in the group may survive the shell; always finish with SIGKILL.
    unsafe {
        libc::killpg(pgid, libc::SIGKILL);
    }
    let _ = child.start_kill();
}

fn default_shell() -> Vec<String> {
    if Path::new("/bin/zsh").exists() {
        vec!["/bin/zsh".into(), "-lc".into()]
    } else {
        vec!["/bin/sh".into(), "-c".into()]
    }
}

fn default_env() -> Vec<(String, String)> {
    let mut env = Vec::new();
    for key in ["PATH", "HOME", "USER", "LANG", "SHELL"] {
        if let Ok(v) = std::env::var(key) {
            env.push((key.to_string(), v));
        }
    }
    if !env.iter().any(|(k, _)| k == "PATH") {
        env.push(("PATH".into(), "/usr/bin:/bin:/usr/sbin:/sbin".into()));
    }
    if !env.iter().any(|(k, _)| k == "LANG") {
        env.push(("LANG".into(), "en_US.UTF-8".into()));
    }
    env
}

/// `worktree.join(cwd)`, which must exist and stay inside the worktree.
fn resolve_cwd(worktree: &Path, cwd: Option<&str>) -> std::result::Result<PathBuf, String> {
    let rel = cwd.map(str::trim).filter(|c| !c.is_empty()).unwrap_or(".");
    let wt = worktree
        .canonicalize()
        .map_err(|e| format!("worktree {} is not accessible: {e}", worktree.display()))?;
    let joined = wt.join(rel);
    let canon = joined
        .canonicalize()
        .map_err(|e| format!("cwd {rel:?} does not exist in the worktree: {e}"))?;
    if !canon.starts_with(&wt) {
        return Err(format!("cwd {rel:?} escapes the worktree"));
    }
    if !canon.is_dir() {
        return Err(format!("cwd {rel:?} is not a directory"));
    }
    Ok(canon)
}

/// Nearest ancestor of `start` (inclusive) that contains a `sites/` directory.
pub fn bench_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|p| p.join("sites").is_dir())
        .map(Path::to_path_buf)
}

fn sanitize_job_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let s = s.trim_matches('.').to_string();
    if s.is_empty() {
        "job".into()
    } else {
        s
    }
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s == 0 {
        format!("{}ms", d.as_millis())
    } else if s.is_multiple_of(3600) {
        format!("{}h", s / 3600)
    } else if s.is_multiple_of(60) {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

/// Parses `30s`, `5m`, `1h`, `1h30m`, `500ms` or bare seconds (`90`). None if invalid or zero.
pub fn parse_timeout(s: &str) -> Option<Duration> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<u64>() {
        return (n > 0).then(|| Duration::from_secs(n));
    }
    let mut total = Duration::ZERO;
    let mut rest = s.as_str();
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        if digits == 0 {
            return None;
        }
        let n: u64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit_len = rest
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(rest.len());
        let unit = rest[..unit_len].trim();
        rest = &rest[unit_len..];
        total += match unit {
            "ms" => Duration::from_millis(n),
            "s" | "sec" | "secs" => Duration::from_secs(n),
            "m" | "min" | "mins" => Duration::from_secs(n * 60),
            "h" | "hr" | "hrs" => Duration::from_secs(n * 3600),
            _ => return None,
        };
    }
    (total > Duration::ZERO).then_some(total)
}

// ------------------------------------------------------------------------------------------------
// Failure excerpt
// ------------------------------------------------------------------------------------------------

/// Streaming failure-excerpt builder: up to [`EXCERPT_LINES`] lines around the first line matching
/// `error|fail|exception|traceback` (case-insensitive; 10 lines before it, the rest after), else the
/// last [`EXCERPT_LINES`] lines.
#[derive(Debug, Default)]
pub struct ExcerptBuilder {
    before: VecDeque<String>,
    matched: Option<Vec<String>>,
    tail: VecDeque<String>,
}

impl ExcerptBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, line: &str) {
        if self.tail.len() == EXCERPT_LINES {
            self.tail.pop_front();
        }
        self.tail.push_back(line.to_string());
        match &mut self.matched {
            Some(m) => {
                if m.len() < EXCERPT_LINES {
                    m.push(line.to_string());
                }
            }
            None => {
                if ERROR_LINE.is_match(line) {
                    let mut m: Vec<String> = self.before.drain(..).collect();
                    m.push(line.to_string());
                    self.matched = Some(m);
                } else {
                    if self.before.len() == EXCERPT_BEFORE {
                        self.before.pop_front();
                    }
                    self.before.push_back(line.to_string());
                }
            }
        }
    }

    pub fn finish(self) -> Option<String> {
        let lines: Vec<String> = match self.matched {
            Some(m) => m,
            None => self.tail.into_iter().collect(),
        };
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }
}

/// Convenience over [`ExcerptBuilder`] for a whole log text.
pub fn failure_excerpt(log: &str) -> Option<String> {
    let mut b = ExcerptBuilder::new();
    for l in log.lines() {
        b.push(l);
    }
    b.finish()
}

// ------------------------------------------------------------------------------------------------
// Command rendering
// ------------------------------------------------------------------------------------------------

fn placeholder_value(name: &str, ctx: &RecipeContext) -> Option<String> {
    match name {
        "changed_files" => Some(quote_files(ctx.changed_files.iter())),
        "changed_py" => Some(quote_files(
            ctx.changed_files.iter().filter(|f| has_ext(f, &["py"])),
        )),
        "changed_ts" => Some(quote_files(
            ctx.changed_files.iter().filter(|f| has_ext(f, &TS_EXTS)),
        )),
        "base_sha" => Some(quote_word(&ctx.base_sha)),
        "head_sha" => Some(quote_word(&ctx.head_sha)),
        _ => None,
    }
}

fn has_ext(path: &str, exts: &[&str]) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

fn quote_files<'a>(files: impl Iterator<Item = &'a String>) -> String {
    files
        .filter(|f| !f.is_empty())
        .filter_map(|f| {
            // A file named like an option must not be read as one (`ruff check --fix`).
            let f = if f.starts_with('-') {
                format!("./{f}")
            } else {
                f.clone()
            };
            shlex::try_quote(&f).ok().map(|q| q.into_owned())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_word(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    shlex::try_quote(s)
        .map(|q| q.into_owned())
        .unwrap_or_default()
}

/// Calls `f(name)` for each `{name}` in the template (single pass, so substituted values are never
/// re-expanded) and builds the output; unknown placeholders are kept literally.
fn expand(template: &str, mut f: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if is_ident(&after[..close]) => {
                let name = &after[..close];
                match f(name) {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Expands `{changed_files}`, `{changed_py}`, `{changed_ts}` (ts/tsx/js/jsx/vue), `{base_sha}`,
/// `{head_sha}`; file lists are shell-quoted and space-separated.
pub fn render_command(template: &str, ctx: &RecipeContext) -> String {
    expand(template, |name| placeholder_value(name, ctx))
}

/// `Some("no matching files")` when the template uses file-list placeholders and every one of them
/// expands to nothing (e.g. `ruff check {changed_py}` on an MR without Python changes).
pub fn skip_reason(template: &str, ctx: &RecipeContext) -> Option<String> {
    let mut used = BTreeSet::new();
    expand(template, |name| {
        if FILE_PLACEHOLDERS.contains(&name) {
            used.insert(name.to_string());
        }
        None
    });
    if used.is_empty() {
        return None;
    }
    let all_empty = used.iter().all(|name| {
        placeholder_value(name, ctx)
            .map(|v| v.is_empty())
            .unwrap_or(true)
    });
    all_empty.then(|| SKIP_NO_FILES.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(files: &[&str]) -> RecipeContext {
        RecipeContext {
            changed_files: files.iter().map(|s| s.to_string()).collect(),
            base_sha: "aaa111".into(),
            head_sha: "bbb222".into(),
        }
    }

    #[test]
    fn renders_placeholders_with_quoting_and_filters() {
        let c = ctx(&[
            "app/api.py",
            "ui/App.tsx",
            "my file.py",
            "README.md",
            "x.VUE",
            "-rf.py",
        ]);
        assert_eq!(
            render_command("ruff check {changed_py}", &c),
            "ruff check app/api.py 'my file.py' ./-rf.py"
        );
        assert_eq!(
            render_command("eslint {changed_ts}", &c),
            "eslint ui/App.tsx x.VUE"
        );
        assert_eq!(
            render_command("echo {changed_files}", &c),
            "echo app/api.py ui/App.tsx 'my file.py' README.md x.VUE ./-rf.py"
        );
        assert_eq!(
            render_command("git diff {base_sha}..{head_sha}", &c),
            "git diff aaa111..bbb222"
        );
        // unknown placeholders and non-placeholder braces are untouched
        assert_eq!(
            render_command("echo {nope} ${HOME} {", &c),
            "echo {nope} ${HOME} {"
        );
        assert_eq!(render_command("awk '{print $1}'", &c), "awk '{print $1}'");
    }

    #[test]
    fn substituted_values_are_not_reexpanded() {
        let c = ctx(&["{head_sha}.py"]);
        assert_eq!(
            render_command("cat {changed_py}", &c),
            "cat '{head_sha}.py'"
        );
    }

    #[test]
    fn skip_when_only_empty_file_placeholders() {
        let c = ctx(&["README.md"]);
        assert_eq!(
            skip_reason("ruff check {changed_py}", &c).as_deref(),
            Some(SKIP_NO_FILES)
        );
        assert_eq!(
            skip_reason("lint {changed_py} {changed_ts}", &c).as_deref(),
            Some(SKIP_NO_FILES)
        );
        assert_eq!(skip_reason("echo {changed_files}", &c), None);
        assert_eq!(skip_reason("pytest", &c), None);
        assert_eq!(skip_reason("git log {base_sha}..{head_sha}", &c), None);
        let c2 = ctx(&["a.ts"]);
        assert_eq!(skip_reason("lint {changed_py} {changed_ts}", &c2), None);
        assert_eq!(
            skip_reason("echo {changed_files}", &ctx(&[])).as_deref(),
            Some(SKIP_NO_FILES)
        );
    }

    #[test]
    fn timeouts() {
        assert_eq!(parse_timeout("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_timeout("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_timeout("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_timeout("1h30m"), Some(Duration::from_secs(5400)));
        assert_eq!(parse_timeout("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_timeout(" 90 "), Some(Duration::from_secs(90)));
        assert_eq!(parse_timeout("0"), None);
        assert_eq!(parse_timeout("abc"), None);
        assert_eq!(parse_timeout("5x"), None);
        assert_eq!(parse_timeout(""), None);
    }

    #[test]
    fn excerpt_around_first_error() {
        let mut log = String::new();
        for i in 0..100 {
            if i == 50 {
                log.push_str("Traceback (most recent call last):\n");
            } else {
                log.push_str(&format!("line {i}\n"));
            }
        }
        let ex = failure_excerpt(&log).unwrap();
        let lines: Vec<&str> = ex.lines().collect();
        assert_eq!(lines.len(), EXCERPT_LINES);
        assert_eq!(lines[0], "line 40");
        assert_eq!(lines[10], "Traceback (most recent call last):");
        assert_eq!(*lines.last().unwrap(), "line 79");
    }

    #[test]
    fn excerpt_error_near_start_and_tail_fallback() {
        let ex = failure_excerpt("ERROR: boom\nnext\n").unwrap();
        assert_eq!(ex, "ERROR: boom\nnext");
        let log: String = (0..100).map(|i| format!("ok {i}\n")).collect();
        let ex = failure_excerpt(&log).unwrap();
        let lines: Vec<&str> = ex.lines().collect();
        assert_eq!(lines.len(), 40);
        assert_eq!(lines[0], "ok 60");
        assert_eq!(lines[39], "ok 99");
        assert_eq!(failure_excerpt(""), None);
    }

    #[test]
    fn bench_root_detection() {
        let dir = tempfile::tempdir().unwrap();
        let bench = dir.path().join("bench");
        std::fs::create_dir_all(bench.join("sites")).unwrap();
        let deep = bench.join("apps/crm/x");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(bench_root(&deep), Some(bench.clone()));
        assert_eq!(bench_root(&bench), Some(bench));
        let other = dir.path().join("other");
        std::fs::create_dir_all(&other).unwrap();
        // The temp dir has no `sites` ancestor (unless the machine has /sites, which it does not).
        assert_eq!(bench_root(&other), None);
    }

    #[test]
    fn cwd_must_stay_inside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        assert!(resolve_cwd(dir.path(), None).is_ok());
        assert!(resolve_cwd(dir.path(), Some("sub"))
            .unwrap()
            .ends_with("sub"));
        assert!(resolve_cwd(dir.path(), Some(".."))
            .unwrap_err()
            .contains("escapes"));
        assert!(resolve_cwd(dir.path(), Some("missing"))
            .unwrap_err()
            .contains("does not exist"));
    }

    #[test]
    fn job_name_sanitizing() {
        assert_eq!(sanitize_job_name("lint"), "lint");
        assert_eq!(sanitize_job_name("unit tests/py"), "unit_tests_py");
        assert_eq!(sanitize_job_name("../x"), "_x");
        assert_eq!(sanitize_job_name(""), "job");
    }
}
