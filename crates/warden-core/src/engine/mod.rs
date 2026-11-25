// OWNER: engine
//! ReviewEngine: spawns the user's local `claude -p` (subscription login, never an API key), with the
//! Warden MCP server as its only MCP server, and collects the outcome from stream-json output.
//! Env stripped from the child: ANTHROPIC_API_KEY, ANTHROPIC_AUTH_TOKEN, ANTHROPIC_BASE_URL,
//! CLAUDE_CODE_USE_BEDROCK, CLAUDE_CODE_USE_VERTEX (hard rule 2).
//!
//! CLI flags were checked against `claude --help` of Claude Code 2.1.330: `--max-turns` is accepted
//! but hidden from the help text, and `--permission-mode default` is accepted as a hidden alias
//! (the listed choices are acceptEdits|auto|bypassPermissions|manual|dontAsk|plan).

pub mod prompt;
pub mod stream;

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;

use crate::config::AppSettings;
use crate::error::{Result, WardenError};
use crate::events::EventBus;
use crate::types::{AppEvent, EngineStatus};

pub use prompt::{build_system_prompt, build_user_prompt};
use stream::{describe_tool_use, parse_line, short_tool_name, StreamEvent};

/// Env vars removed from the `claude` child process.
pub const STRIPPED_ENV: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
];

/// `--allowedTools` for review runs. Only Warden's MCP tools: the built-in file tools (Read, Grep,
/// Glob, ...) are NOT path-limited and would let a prompt-injected review read `~/.ssh`,
/// `~/.git-credentials` or the glab config, bypassing the worktree containment and secret
/// redaction of `repo_read_file` / `repo_search` (which cover the same needs).
pub const ALLOWED_TOOLS: &[&str] = &["mcp__warden__*"];
/// `--disallowedTools` for review runs (deny rules win over any allow rule in user settings).
pub const DISALLOWED_TOOLS: &[&str] = &[
    "Read",
    "Grep",
    "Glob",
    "LS",
    "NotebookRead",
    "Bash",
    "Edit",
    "Write",
    "MultiEdit",
    "NotebookEdit",
    "WebFetch",
    "WebSearch",
    "Task",
];

const VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const DEEP_TIMEOUT: Duration = Duration::from_secs(120);
const KILL_GRACE: Duration = Duration::from_secs(3);
const STDERR_TAIL_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone)]
pub struct ReviewEngine {
    pub settings: AppSettings,
    pub events: EventBus,
    /// Engine addition: run_id -> cancel signal for in-flight reviews (see [`ReviewEngine::cancel`]).
    pub cancels: Arc<Mutex<HashMap<i64, watch::Sender<bool>>>>,
}

#[derive(Debug, Clone)]
pub struct ReviewRequest {
    pub run_id: i64,
    pub worktree: PathBuf,
    pub mcp_url: String,
    pub run_token: String,
    pub system_prompt: String,
    pub user_prompt: String,
    pub max_turns: u32,
    pub timeout: Duration,
    pub model: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EngineOutcome {
    pub turns: u32,
    pub duration_ms: u64,
    pub session_id: Option<String>,
    /// True if Claude called `review_submit` (and the call was not rejected).
    pub submitted: bool,
    pub summary: Option<String>,
    pub verdict: Option<String>,
    pub raw_result: Option<String>,
    pub is_error: bool,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Binary discovery / env
// ---------------------------------------------------------------------------------------------

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Expands a leading `~/`.
pub fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(p));
    }
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(h) = home_dir() {
            return h.join(rest);
        }
    }
    PathBuf::from(p)
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Finds the Claude Code CLI: `settings.claude_cli_path` if set (only that), else `claude` on PATH,
/// else the usual install locations (`~/.claude/local/claude`, `~/.local/bin/claude`,
/// `/opt/homebrew/bin/claude`, `/usr/local/bin/claude`). GUI apps on macOS get a minimal PATH, so
/// the fallbacks matter.
pub fn locate_claude(settings: &AppSettings) -> Option<PathBuf> {
    if let Some(p) = settings
        .claude_cli_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let p = expand_tilde(p);
        return is_executable(&p).then_some(p);
    }
    if let Ok(p) = which::which("claude") {
        return Some(p);
    }
    let mut candidates = Vec::new();
    if let Some(h) = home_dir() {
        candidates.push(h.join(".claude/local/claude"));
        candidates.push(h.join(".local/bin/claude"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/claude"));
    candidates.push(PathBuf::from("/usr/local/bin/claude"));
    candidates.into_iter().find(|p| is_executable(p))
}

/// True if an Anthropic API credential is present in Warden's own environment (it is stripped from
/// the child either way; the UI warns about it).
pub fn api_key_in_env() -> bool {
    ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
        .iter()
        .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
}

/// PATH for the child: the inherited PATH plus the CLI's own dir and common bin dirs (so a
/// node-shebang install still finds `node` when Warden runs as a GUI app).
fn child_path(cli: &Path) -> OsString {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let mut extra = Vec::new();
    if let Some(parent) = cli.parent() {
        extra.push(parent.to_path_buf());
    }
    extra.push(PathBuf::from("/opt/homebrew/bin"));
    extra.push(PathBuf::from("/usr/local/bin"));
    extra.push(PathBuf::from("/usr/bin"));
    extra.push(PathBuf::from("/bin"));
    for e in extra {
        if !dirs.contains(&e) {
            dirs.push(e);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// A `claude` command with the stripped environment (hard rule 2).
pub fn claude_command(cli: &Path) -> Command {
    let mut cmd = Command::new(cli);
    for k in STRIPPED_ENV {
        cmd.env_remove(k);
    }
    cmd.env("PATH", child_path(cli));
    cmd.stdin(Stdio::null());
    cmd
}

/// Runs a short command and captures output; `None` on spawn failure or timeout.
async fn run_capture(mut cmd: Command, timeout: Duration) -> Option<(bool, String, String)> {
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = cmd.spawn().ok()?;
    let out = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    Some((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    ))
}

/// `2.1.330 (Claude Code)` -> `2.1.330`.
pub fn parse_version(stdout: &str) -> Option<String> {
    let line = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    let tok = line.split_whitespace().next()?;
    tok.chars()
        .next()
        .filter(|c| c.is_ascii_digit())
        .map(|_| tok.to_string())
}

/// Parsed `claude auth status --json`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AuthInfo {
    pub logged_in: bool,
    pub auth_method: Option<String>,
    pub subscription: Option<String>,
}

pub fn parse_auth_status(stdout: &str) -> Option<AuthInfo> {
    let start = stdout.find('{')?;
    let end = stdout.rfind('}')?;
    let v: serde_json::Value = serde_json::from_str(stdout.get(start..=end)?).ok()?;
    let logged_in = v.get("loggedIn").and_then(|x| x.as_bool())?;
    let s = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .filter(|x| !x.is_empty())
            .map(str::to_string)
    };
    Some(AuthInfo {
        logged_in,
        auth_method: s("authMethod"),
        subscription: s("subscriptionType"),
    })
}

/// Parses the single JSON result of `claude -p --output-format json`:
/// returns `(is_error, result_text)`.
pub fn parse_json_result(stdout: &str) -> Option<(bool, String)> {
    let candidates = std::iter::once(stdout.trim()).chain(stdout.lines().rev().map(str::trim));
    for c in candidates {
        if !c.starts_with('{') && !c.starts_with('[') {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(c) else {
            continue;
        };
        // Some versions emit an array of messages; take the result element.
        let obj = match &v {
            serde_json::Value::Array(a) => a
                .iter()
                .rev()
                .find(|x| x.get("type").and_then(|t| t.as_str()) == Some("result"))
                .cloned(),
            serde_json::Value::Object(_) => Some(v.clone()),
            _ => None,
        };
        if let Some(o) = obj {
            if o.get("type").and_then(|t| t.as_str()) == Some("result") || o.get("result").is_some()
            {
                let is_error = o.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false);
                let text = o
                    .get("result")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                return Some((is_error, text));
            }
        }
    }
    None
}

/// The JSON written to the temp `--mcp-config` file.
pub fn mcp_config_json(mcp_url: &str, run_token: &str) -> serde_json::Value {
    serde_json::json!({
        "mcpServers": {
            "warden": {
                "type": "http",
                "url": mcp_url,
                "headers": { "X-Warden-Run": run_token }
            }
        }
    })
}

fn write_mcp_config(mcp_url: &str, run_token: &str) -> Result<tempfile::NamedTempFile> {
    // NamedTempFile is created 0600, so the run token is not world-readable.
    let mut f = tempfile::Builder::new()
        .prefix("warden-mcp-")
        .suffix(".json")
        .tempfile()?;
    let body = serde_json::to_vec_pretty(&mcp_config_json(mcp_url, run_token))?;
    f.write_all(&body)?;
    f.flush()?;
    Ok(f)
}

/// Full argv (after the binary) for a review run.
pub fn review_args(req: &ReviewRequest, mcp_config_path: &Path) -> Vec<OsString> {
    // A prompt starting with '-' would be parsed as an option.
    let prompt = if req.user_prompt.starts_with('-') {
        format!(" {}", req.user_prompt)
    } else {
        req.user_prompt.clone()
    };
    let mut a: Vec<OsString> = vec![
        "-p".into(),
        prompt.into(),
        "--append-system-prompt".into(),
        req.system_prompt.clone().into(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--mcp-config".into(),
        mcp_config_path.as_os_str().to_owned(),
        "--strict-mcp-config".into(),
        "--allowedTools".into(),
    ];
    a.extend(ALLOWED_TOOLS.iter().map(OsString::from));
    a.push("--disallowedTools".into());
    a.extend(DISALLOWED_TOOLS.iter().map(OsString::from));
    a.extend([
        "--permission-mode".into(),
        "default".into(),
        "--model".into(),
        req.model.clone().into(),
        "--max-turns".into(),
        req.max_turns.max(1).to_string().into(),
    ]);
    a
}

// ---------------------------------------------------------------------------------------------
// Process-group handling
// ---------------------------------------------------------------------------------------------

/// Kills the child's whole process group when dropped (cancelled future, panic) unless disarmed.
struct GroupGuard {
    pgid: Option<i32>,
}

impl GroupGuard {
    fn signal(&self, sig: i32) {
        if let Some(pgid) = self.pgid {
            if pgid > 1 {
                // SAFETY: killpg only sends a signal; pgid is the group we created for the child.
                unsafe {
                    libc::killpg(pgid, sig);
                }
            }
        }
    }

    fn disarm(&mut self) {
        self.pgid = None;
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        self.signal(libc::SIGKILL);
    }
}

async fn terminate(child: &mut tokio::process::Child, guard: &mut GroupGuard) {
    guard.signal(libc::SIGTERM);
    if tokio::time::timeout(KILL_GRACE, child.wait())
        .await
        .is_err()
    {
        guard.signal(libc::SIGKILL);
        let _ = child.start_kill();
        let _ = tokio::time::timeout(KILL_GRACE, child.wait()).await;
    }
    // Stragglers in the group (grandchildren) get SIGKILL too.
    guard.signal(libc::SIGKILL);
    guard.disarm();
}

// ---------------------------------------------------------------------------------------------
// Stream accumulation
// ---------------------------------------------------------------------------------------------

/// The `result` event of a stream.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultInfo {
    pub subtype: Option<String>,
    pub num_turns: u32,
    pub duration_ms: u64,
    pub is_error: bool,
    pub result: Option<String>,
}

/// Folds stream events into an outcome and emits progress.
#[derive(Debug, Default)]
pub struct StreamState {
    pub turns: u32,
    pub session_id: Option<String>,
    pub submitted: bool,
    pub summary: Option<String>,
    pub verdict: Option<String>,
    pub submit_tool_id: Option<String>,
    pub last_text: Option<String>,
    pub result: Option<ResultInfo>,
    pub mcp_problem: Option<String>,
    pub tool_calls: u32,
}

impl StreamState {
    /// Applies one event; returns a progress message to emit (if any).
    pub fn apply(&mut self, ev: StreamEvent) -> Option<String> {
        match ev {
            StreamEvent::Init {
                session_id,
                mcp_servers,
                model,
                ..
            } => {
                self.session_id = session_id.or(self.session_id.take());
                let warden = mcp_servers.iter().find(|(n, _)| n == "warden");
                match warden {
                    Some((_, status)) if status != "connected" => {
                        self.mcp_problem = Some(format!("warden MCP server status: {status}"));
                        Some(format!("Warden MCP server not connected ({status})"))
                    }
                    _ => Some(format!(
                        "Claude session started{}",
                        model.map(|m| format!(" ({m})")).unwrap_or_default()
                    )),
                }
            }
            StreamEvent::AssistantTurn => {
                self.turns += 1;
                None
            }
            StreamEvent::ToolUse { id, name, input } => {
                self.tool_calls += 1;
                if short_tool_name(&name) == "review_submit" {
                    self.submitted = true;
                    self.summary = input
                        .get("summary")
                        .and_then(|x| x.as_str())
                        .map(str::to_string);
                    self.verdict = input
                        .get("verdict")
                        .and_then(|x| x.as_str())
                        .map(str::to_string);
                    self.submit_tool_id = id;
                }
                Some(describe_tool_use(&name, &input))
            }
            StreamEvent::ToolResult {
                tool_use_id,
                is_error,
            } => {
                if is_error && tool_use_id.is_some() && tool_use_id == self.submit_tool_id {
                    self.submitted = false;
                    self.summary = None;
                    self.verdict = None;
                    self.submit_tool_id = None;
                }
                None
            }
            StreamEvent::Text(t) => {
                if !t.trim().is_empty() {
                    self.last_text = Some(t);
                }
                None
            }
            StreamEvent::Result {
                subtype,
                num_turns,
                duration_ms,
                is_error,
                result,
                session_id,
            } => {
                if session_id.is_some() {
                    self.session_id = session_id;
                }
                self.result = Some(ResultInfo {
                    subtype,
                    num_turns,
                    duration_ms,
                    is_error,
                    result,
                });
                None
            }
            StreamEvent::Other => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// ReviewEngine
// ---------------------------------------------------------------------------------------------

impl ReviewEngine {
    pub fn new(settings: AppSettings, events: EventBus) -> Self {
        Self {
            settings,
            events,
            cancels: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Cancels the in-flight review of `run_id` (its process group is killed). No-op if none.
    pub fn cancel(&self, run_id: i64) {
        if let Ok(map) = self.cancels.lock() {
            if let Some(tx) = map.get(&run_id) {
                let _ = tx.send(true);
            }
        }
    }

    fn progress(&self, run_id: i64, message: impl Into<String>, turn: u32) {
        self.events.emit(AppEvent::RunProgress {
            run_id,
            message: message.into(),
            turn,
        });
    }

    /// Cheap: find the binary, `claude --version`, `claude auth status` (no prompt is sent), env check.
    pub async fn status(&self) -> EngineStatus {
        let mut st = EngineStatus {
            api_key_in_env: api_key_in_env(),
            ..Default::default()
        };
        let key_note = if st.api_key_in_env {
            " ANTHROPIC_API_KEY/AUTH_TOKEN is set in Warden's environment; it is stripped from the claude process so your subscription login is used."
        } else {
            ""
        };
        let Some(cli) = locate_claude(&self.settings) else {
            st.message = match self.settings.claude_cli_path.as_deref().filter(|p| !p.trim().is_empty()) {
                Some(p) => format!("Claude Code CLI not found at configured path {p}.{key_note}"),
                None => format!(
                    "Claude Code CLI not found (looked on PATH, ~/.claude/local, ~/.local/bin, /opt/homebrew/bin, /usr/local/bin). Install it and run `claude` once to log in.{key_note}"
                ),
            };
            return st;
        };
        st.cli_path = Some(cli.display().to_string());

        let mut ver = claude_command(&cli);
        ver.arg("--version");
        match run_capture(ver, VERSION_TIMEOUT).await {
            Some((true, out, _)) => st.version = parse_version(&out),
            Some((false, out, err)) => {
                st.message = format!(
                    "`claude --version` failed: {}{key_note}",
                    first_line(if err.trim().is_empty() { &out } else { &err })
                );
                return st;
            }
            None => {
                st.message = format!("`claude --version` did not run or timed out.{key_note}");
                return st;
            }
        }

        let mut auth = claude_command(&cli);
        auth.args(["auth", "status", "--json"]);
        let auth_info = match run_capture(auth, VERSION_TIMEOUT).await {
            Some((_, out, _)) => parse_auth_status(&out),
            None => None,
        };
        let version = st
            .version
            .clone()
            .unwrap_or_else(|| "unknown version".into());
        match auth_info {
            Some(a) => {
                st.logged_in = a.logged_in;
                st.subscription = a.subscription.clone();
                if a.logged_in {
                    st.ok = true;
                    let how = match (&a.subscription, &a.auth_method) {
                        (Some(s), _) => format!("{s} subscription"),
                        (None, Some(m)) => m.clone(),
                        _ => "logged in".into(),
                    };
                    st.message = format!("Claude Code {version} ready ({how}).{key_note}");
                } else {
                    st.ok = false;
                    st.message = format!(
                        "Claude Code {version} found but not logged in. Run `claude` in a terminal and use /login with your subscription.{key_note}"
                    );
                }
            }
            None => {
                // Older CLIs lack `auth status`; the deep check verifies login.
                st.ok = st.version.is_some();
                st.message = format!(
                    "Claude Code {version} found; login not verified (run the deep check).{key_note}"
                );
            }
        }
        st
    }

    /// Also checks login with `claude -p "reply OK" --max-turns 1 --output-format json`.
    /// Only when the user explicitly asks (it uses the subscription).
    pub async fn status_deep(&self) -> EngineStatus {
        let mut st = self.status().await;
        let Some(cli) = st.cli_path.clone().map(PathBuf::from) else {
            return st;
        };
        let tmp = tempfile::tempdir().ok();
        let mut cmd = claude_command(&cli);
        cmd.args([
            "-p",
            "Reply with exactly: OK",
            "--max-turns",
            "1",
            "--output-format",
            "json",
            "--strict-mcp-config",
            "--tools",
            "",
        ]);
        if !self.settings.model.trim().is_empty() {
            cmd.arg("--model").arg(&self.settings.model);
        }
        if let Some(t) = &tmp {
            cmd.current_dir(t.path());
        }
        let started = Instant::now();
        match run_capture(cmd, DEEP_TIMEOUT).await {
            None => {
                st.ok = false;
                st.message = "Deep check: claude did not answer within 120 s.".into();
            }
            Some((success, out, err)) => match parse_json_result(&out) {
                Some((false, text)) if success || !text.is_empty() => {
                    st.logged_in = true;
                    st.ok = true;
                    st.message = format!(
                        "Login verified: Claude replied {:?} in {} ms.{}",
                        first_line(&text),
                        started.elapsed().as_millis(),
                        if st.api_key_in_env {
                            " (API key in Warden's environment was stripped.)"
                        } else {
                            ""
                        }
                    );
                }
                Some((_, text)) => {
                    st.ok = false;
                    st.logged_in = !looks_like_auth_error(&text);
                    st.message = format!("Deep check failed: {}", first_line(&text));
                }
                None => {
                    let msg = if err.trim().is_empty() { out } else { err };
                    st.ok = false;
                    if looks_like_auth_error(&msg) {
                        st.logged_in = false;
                    }
                    st.message = format!("Deep check failed: {}", tail(&msg, 300));
                }
            },
        }
        st
    }

    /// Spawns claude (cwd = worktree), streams progress, kills the process group on timeout/cancel.
    pub async fn review(&self, req: ReviewRequest) -> Result<EngineOutcome> {
        let started = Instant::now();
        let cli = locate_claude(&self.settings).ok_or_else(|| {
            WardenError::Engine("Claude Code CLI not found; set its path in Settings".into())
        })?;
        if !req.worktree.is_dir() {
            return Err(WardenError::Engine(format!(
                "worktree {} does not exist",
                req.worktree.display()
            )));
        }
        let mcp_cfg = write_mcp_config(&req.mcp_url, &req.run_token)?;

        let mut cmd = claude_command(&cli);
        cmd.args(review_args(&req, mcp_cfg.path()))
            .current_dir(&req.worktree)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);

        // Register the cancel signal before spawning.
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        if let Ok(mut map) = self.cancels.lock() {
            map.insert(req.run_id, cancel_tx);
        }
        let _unregister = Unregister {
            map: self.cancels.clone(),
            run_id: req.run_id,
        };

        self.progress(
            req.run_id,
            format!(
                "Starting Claude Code ({}, max {} turns)",
                req.model, req.max_turns
            ),
            0,
        );
        let mut child = cmd
            .spawn()
            .map_err(|e| WardenError::Engine(format!("failed to start {}: {e}", cli.display())))?;
        let mut guard = GroupGuard {
            pgid: child.id().map(|p| p as i32),
        };

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| WardenError::Engine("claude: no stdout".into()))?;
        let stderr = child.stderr.take();
        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(mut se) = stderr {
                let mut chunk = [0u8; 4096];
                loop {
                    match se.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.len() > STDERR_TAIL_BYTES * 2 {
                                let cut = buf.len() - STDERR_TAIL_BYTES;
                                buf.drain(..cut);
                            }
                        }
                    }
                }
            }
            String::from_utf8_lossy(&buf).to_string()
        });

        let mut state = StreamState::default();
        let mut lines = BufReader::new(stdout).lines();
        let deadline = tokio::time::sleep(req.timeout);
        tokio::pin!(deadline);
        let mut timed_out = false;
        let mut cancelled = false;
        let mut cancel_open = true;
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(l)) => {
                        for ev in parse_line(&l) {
                            if let Some(msg) = state.apply(ev) {
                                self.progress(req.run_id, msg, state.turns);
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        tracing::warn!("claude stdout read error: {e}");
                        break;
                    }
                },
                _ = &mut deadline => {
                    timed_out = true;
                    break;
                }
                changed = cancel_rx.changed(), if cancel_open => {
                    match changed {
                        Ok(()) if *cancel_rx.borrow() => {
                            cancelled = true;
                            break;
                        }
                        Ok(()) => {}
                        Err(_) => cancel_open = false,
                    }
                }
            }
        }

        let exit_status = if timed_out || cancelled {
            terminate(&mut child, &mut guard).await;
            None
        } else {
            match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
                Ok(Ok(s)) => {
                    guard.disarm();
                    Some(s)
                }
                _ => {
                    terminate(&mut child, &mut guard).await;
                    None
                }
            }
        };
        let stderr_text = tokio::time::timeout(Duration::from_secs(2), stderr_task)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or_default();
        drop(mcp_cfg);

        let elapsed_ms = started.elapsed().as_millis() as u64;
        let mut out = EngineOutcome {
            turns: state.turns,
            duration_ms: elapsed_ms,
            session_id: state.session_id.clone(),
            submitted: state.submitted,
            summary: state.summary.clone(),
            verdict: state.verdict.clone(),
            raw_result: state.last_text.clone(),
            is_error: false,
            error: None,
        };
        if let Some(ResultInfo {
            subtype,
            num_turns,
            duration_ms,
            is_error,
            result,
        }) = state.result.clone()
        {
            if num_turns > 0 {
                out.turns = num_turns;
            }
            if duration_ms > 0 {
                out.duration_ms = duration_ms;
            }
            if result.is_some() {
                out.raw_result = result.clone();
            }
            out.is_error = is_error;
            if is_error {
                out.error = Some(match subtype.as_deref() {
                    Some("error_max_turns") => {
                        format!("Claude reached the max turns limit ({})", req.max_turns)
                    }
                    _ => result
                        .clone()
                        .filter(|r| !r.trim().is_empty())
                        .or(subtype.clone())
                        .unwrap_or_else(|| "claude reported an error".into()),
                });
            }
        }

        if timed_out {
            out.is_error = true;
            out.error = Some(format!(
                "review timed out after {} s",
                req.timeout.as_secs()
            ));
        } else if cancelled {
            out.is_error = true;
            out.error = Some("review cancelled".into());
        } else if state.result.is_none() {
            out.is_error = true;
            let code = exit_status
                .and_then(|s| s.code())
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".into());
            let detail = if stderr_text.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", tail(&stderr_text, 600))
            };
            out.error = Some(format!(
                "claude exited (status {code}) without a result{detail}"
            ));
        }
        if let Some(p) = &state.mcp_problem {
            if !out.submitted {
                let base = out.error.take().unwrap_or_default();
                out.error = Some(if base.is_empty() {
                    p.clone()
                } else {
                    format!("{base}; {p}")
                });
                out.is_error = true;
            }
        }

        self.progress(
            req.run_id,
            format!(
                "Claude finished: {} turn(s), {} tool call(s), {}{}",
                out.turns,
                state.tool_calls,
                if out.submitted {
                    "review submitted"
                } else {
                    "no review_submit"
                },
                out.error
                    .as_deref()
                    .map(|e| format!(" - {e}"))
                    .unwrap_or_default()
            ),
            out.turns,
        );
        Ok(out)
    }
}

struct Unregister {
    map: Arc<Mutex<HashMap<i64, watch::Sender<bool>>>>,
    run_id: i64,
}

impl Drop for Unregister {
    fn drop(&mut self) {
        if let Ok(mut m) = self.map.lock() {
            m.remove(&self.run_id);
        }
    }
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .chars()
        .take(300)
        .collect()
}

fn tail(s: &str, max_chars: usize) -> String {
    let s = s.trim();
    let n = s.chars().count();
    if n <= max_chars {
        return s.to_string();
    }
    let t: String = s.chars().skip(n - max_chars).collect();
    format!("…{t}")
}

fn looks_like_auth_error(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("/login")
        || l.contains("not logged in")
        || l.contains("invalid api key")
        || l.contains("authentication")
        || l.contains("oauth")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parse() {
        assert_eq!(
            parse_version("2.1.330 (Claude Code)\n").as_deref(),
            Some("2.1.330")
        );
        assert_eq!(parse_version("\n  1.0.0\n").as_deref(), Some("1.0.0"));
        assert_eq!(parse_version("error: nope"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn tilde() {
        if let Some(h) = home_dir() {
            assert_eq!(expand_tilde("~/x/y"), h.join("x/y"));
        }
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
    }

    #[test]
    fn auth_parse() {
        let a = parse_auth_status(
            "warning line\n{\"loggedIn\": true, \"authMethod\": \"claude.ai\", \"subscriptionType\": \"max\"}",
        )
        .unwrap();
        assert!(a.logged_in);
        assert_eq!(a.subscription.as_deref(), Some("max"));
        assert_eq!(a.auth_method.as_deref(), Some("claude.ai"));
        let b = parse_auth_status("{\"loggedIn\": false}").unwrap();
        assert!(!b.logged_in);
        assert!(parse_auth_status("garbage").is_none());
    }

    #[test]
    fn json_result_parse() {
        let out =
            r#"{"type":"result","subtype":"success","is_error":false,"result":"OK","num_turns":1}"#;
        assert_eq!(parse_json_result(out), Some((false, "OK".into())));
        let arr =
            r#"[{"type":"system"},{"type":"result","is_error":true,"result":"Invalid API key"}]"#;
        assert_eq!(
            parse_json_result(arr),
            Some((true, "Invalid API key".into()))
        );
        assert_eq!(parse_json_result("nothing"), None);
    }
}
