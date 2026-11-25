//! ReviewEngine against a FAKE `claude` (shell script) passed via settings.claude_cli_path.
//! No real Claude call is made here.

mod engine_common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use warden_core::config::AppSettings;
use warden_core::engine::{locate_claude, review_args, ReviewEngine, ReviewRequest};
use warden_core::types::AppEvent;
use warden_core::EventBus;

use engine_common::write_script;

const STREAM_OK: &str = r#"{"type":"system","subtype":"init","session_id":"sess-abc","tools":["Read","mcp__warden__review_submit"],"mcp_servers":[{"name":"warden","status":"connected"}],"model":"opus"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1","name":"mcp__warden__mr_get_overview","input":{}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false,"content":"ok"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"mcp__warden__review_add_finding","input":{"file":"app/api.py","line":5,"severity":"blocker","title":"SQL injection"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","is_error":false,"content":"accepted"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t3","name":"mcp__warden__review_submit","input":{"summary":"One blocker.","verdict":"request_changes"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t3","is_error":false,"content":"submitted"}]}}
{"type":"result","subtype":"success","is_error":false,"duration_ms":4321,"num_turns":4,"result":"Done.","session_id":"sess-abc","total_cost_usd":0.1}"#;

struct Fake {
    dir: tempfile::TempDir,
    script: PathBuf,
}

impl Fake {
    fn out(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.out(name)).unwrap_or_default()
    }
}

/// `body` runs for review invocations; `--version` and `auth status` are answered first.
fn fake_claude(body: &str) -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().display().to_string();
    let script = dir.path().join("claude");
    let text = format!(
        r#"#!/bin/sh
OUT="{out}"
case "$1" in
  --version) echo "9.9.9 (Claude Code)"; exit 0;;
  auth) echo '{{"loggedIn": true, "authMethod": "claude.ai", "subscriptionType": "max"}}'; exit 0;;
esac
printf '%s\n' "$@" > "$OUT/args.txt"
prev=""
for a in "$@"; do
  if [ "$prev" = "--mcp-config" ]; then cat "$a" > "$OUT/mcp.json"; ls -l "$a" > "$OUT/mcp_perm.txt"; fi
  prev="$a"
done
pwd > "$OUT/cwd.txt"
echo "key=${{ANTHROPIC_API_KEY-unset}} base=${{ANTHROPIC_BASE_URL-unset}}" > "$OUT/env.txt"
{body}
"#
    );
    write_script(&script, &text);
    Fake { dir, script }
}

fn engine_for(fake: &Fake) -> ReviewEngine {
    let settings = AppSettings {
        claude_cli_path: Some(fake.script.display().to_string()),
        ..Default::default()
    };
    ReviewEngine::new(settings, EventBus::new())
}

fn request(worktree: &Path, timeout: Duration) -> ReviewRequest {
    ReviewRequest {
        run_id: 5,
        worktree: worktree.to_path_buf(),
        mcp_url: "http://127.0.0.1:7811/mcp".into(),
        run_token: "tok123".into(),
        system_prompt: "SYSTEM PROMPT TEXT".into(),
        user_prompt: "Review merge request !42".into(),
        max_turns: 12,
        timeout,
        model: "opus".into(),
    }
}

fn drain(rx: &mut tokio::sync::broadcast::Receiver<AppEvent>) -> Vec<(String, u32)> {
    let mut v = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::RunProgress {
            run_id,
            message,
            turn,
        } = ev
        {
            assert_eq!(run_id, 5);
            v.push((message, turn));
        }
    }
    v
}

#[tokio::test]
async fn successful_review_parses_stream_and_passes_flags() {
    let fake = fake_claude(&format!("cat <<'EOF'\n{STREAM_OK}\nEOF\nexit 0"));
    let wt = tempfile::tempdir().unwrap();
    let engine = engine_for(&fake);
    let mut rx = engine.events.subscribe();

    let out = engine
        .review(request(wt.path(), Duration::from_secs(20)))
        .await
        .unwrap();
    assert!(!out.is_error, "{:?}", out.error);
    assert_eq!(out.error, None);
    assert_eq!(out.turns, 4);
    assert_eq!(out.duration_ms, 4321);
    assert_eq!(out.session_id.as_deref(), Some("sess-abc"));
    assert!(out.submitted);
    assert_eq!(out.summary.as_deref(), Some("One blocker."));
    assert_eq!(out.verdict.as_deref(), Some("request_changes"));
    assert_eq!(out.raw_result.as_deref(), Some("Done."));

    // progress events carry tool names and turn counts
    let progress = drain(&mut rx);
    let msgs: Vec<&str> = progress.iter().map(|(m, _)| m.as_str()).collect();
    assert!(msgs[0].starts_with("Starting Claude Code (opus, max 12 turns)"));
    assert!(msgs.contains(&"mr_get_overview"));
    assert!(msgs.contains(&"review_add_finding (blocker app/api.py:5 SQL injection)"));
    assert!(msgs.contains(&"review_submit (request_changes)"));
    assert_eq!(
        progress
            .iter()
            .find(|(m, _)| m == "mr_get_overview")
            .unwrap()
            .1,
        1
    );
    assert_eq!(
        progress
            .iter()
            .find(|(m, _)| m.starts_with("review_submit"))
            .unwrap()
            .1,
        3
    );
    assert!(msgs
        .last()
        .unwrap()
        .starts_with("Claude finished: 4 turn(s), 3 tool call(s), review submitted"));

    // exact CLI contract
    let args: Vec<String> = fake.read("args.txt").lines().map(str::to_string).collect();
    let pos = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag} missing: {args:?}"))
    };
    assert_eq!(args[0], "-p");
    assert_eq!(args[1], "Review merge request !42");
    assert_eq!(
        args[pos("--append-system-prompt") + 1],
        "SYSTEM PROMPT TEXT"
    );
    assert_eq!(args[pos("--output-format") + 1], "stream-json");
    pos("--verbose");
    pos("--strict-mcp-config");
    let a = pos("--allowedTools");
    // Security regression: only the Warden MCP tools are allowed; the built-in file tools are not
    // path-limited (they could read ~/.ssh or forge credentials), so they are denied outright.
    assert_eq!(args[a + 1], "mcp__warden__*");
    assert!(args[a + 2].starts_with("--"), "{args:?}");
    let d = pos("--disallowedTools");
    assert_eq!(
        &args[d + 1..d + 14],
        &[
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
            "Task"
        ]
    );
    assert_eq!(args[pos("--permission-mode") + 1], "default");
    assert_eq!(args[pos("--model") + 1], "opus");
    assert_eq!(args[pos("--max-turns") + 1], "12");

    // temp MCP config: content, 0600, removed afterwards
    let cfg: serde_json::Value = serde_json::from_str(&fake.read("mcp.json")).unwrap();
    assert_eq!(cfg["mcpServers"]["warden"]["type"], "http");
    assert_eq!(
        cfg["mcpServers"]["warden"]["url"],
        "http://127.0.0.1:7811/mcp"
    );
    assert_eq!(
        cfg["mcpServers"]["warden"]["headers"]["X-Warden-Run"],
        "tok123"
    );
    assert!(
        fake.read("mcp_perm.txt").starts_with("-rw-------"),
        "{}",
        fake.read("mcp_perm.txt")
    );
    let cfg_path = &args[pos("--mcp-config") + 1];
    assert!(
        !Path::new(cfg_path).exists(),
        "temp mcp config must be deleted"
    );

    // cwd = worktree
    let cwd = PathBuf::from(fake.read("cwd.txt").trim());
    assert_eq!(
        cwd.canonicalize().unwrap(),
        wt.path().canonicalize().unwrap()
    );
    // the cancel registration is cleaned up
    assert!(engine.cancels.lock().unwrap().is_empty());
}

#[tokio::test]
async fn max_turns_error_is_reported() {
    let fake = fake_claude(
        r#"echo '{"type":"system","subtype":"init","session_id":"s1","tools":[],"mcp_servers":[{"name":"warden","status":"connected"}]}'
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"a","name":"mcp__warden__mr_get_diff","input":{}}]}}'
echo '{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":12,"duration_ms":999,"session_id":"s1"}'
exit 1"#,
    );
    let wt = tempfile::tempdir().unwrap();
    let out = engine_for(&fake)
        .review(request(wt.path(), Duration::from_secs(20)))
        .await
        .unwrap();
    assert!(out.is_error);
    assert!(!out.submitted);
    assert_eq!(out.turns, 12);
    assert!(out.error.unwrap().contains("max turns limit (12)"));
}

#[tokio::test]
async fn crash_without_result_reports_stderr() {
    let fake = fake_claude("echo 'Error: Invalid API key · Please run /login' >&2\nexit 3");
    let wt = tempfile::tempdir().unwrap();
    let out = engine_for(&fake)
        .review(request(wt.path(), Duration::from_secs(20)))
        .await
        .unwrap();
    assert!(out.is_error);
    let e = out.error.unwrap();
    assert!(e.contains("status 3"), "{e}");
    assert!(e.contains("Please run /login"), "{e}");
}

#[tokio::test]
async fn mcp_connection_failure_is_an_error_when_nothing_submitted() {
    let fake = fake_claude(
        r#"echo '{"type":"system","subtype":"init","session_id":"s1","tools":[],"mcp_servers":[{"name":"warden","status":"failed"}]}'
echo '{"type":"result","subtype":"success","is_error":false,"num_turns":1,"duration_ms":10,"result":"I could not use the tools."}'"#,
    );
    let wt = tempfile::tempdir().unwrap();
    let out = engine_for(&fake)
        .review(request(wt.path(), Duration::from_secs(20)))
        .await
        .unwrap();
    assert!(out.is_error);
    assert!(out
        .error
        .unwrap()
        .contains("warden MCP server status: failed"));
}

fn pid_alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

async fn wait_dead(pid: i32) -> bool {
    for _ in 0..50 {
        if !pid_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

const HANG: &str = r#"sleep 300 &
echo $! > "$OUT/child.pid"
echo '{"type":"system","subtype":"init","session_id":"s-hang","tools":[],"mcp_servers":[]}'
sleep 300
"#;

#[tokio::test]
async fn timeout_kills_the_process_group() {
    let fake = fake_claude(HANG);
    let wt = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let out = engine_for(&fake)
        .review(request(wt.path(), Duration::from_secs(5)))
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(out.is_error);
    assert_eq!(out.error.as_deref(), Some("review timed out after 5 s"));
    assert_eq!(out.session_id.as_deref(), Some("s-hang"));
    let pid: i32 = fake.read("child.pid").trim().parse().unwrap();
    assert!(
        wait_dead(pid).await,
        "grandchild {pid} survived the timeout"
    );
}

#[tokio::test]
async fn cancel_kills_the_process_group() {
    let fake = fake_claude(HANG);
    let wt = tempfile::tempdir().unwrap();
    let engine = engine_for(&fake);
    let e2 = engine.clone();
    let child_pid_file = fake.out("child.pid");
    let canceller = tokio::spawn(async move {
        // wait until the fake is running
        for _ in 0..100 {
            if child_pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        e2.cancel(5);
    });
    let started = Instant::now();
    let out = engine
        .review(request(wt.path(), Duration::from_secs(60)))
        .await
        .unwrap();
    canceller.await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(15));
    assert!(out.is_error);
    assert_eq!(out.error.as_deref(), Some("review cancelled"));
    let pid: i32 = fake.read("child.pid").trim().parse().unwrap();
    assert!(wait_dead(pid).await, "grandchild {pid} survived cancel");
    // cancelling an unknown run is a no-op
    engine.cancel(999);
}

#[tokio::test]
async fn dropping_the_future_kills_the_process_group() {
    let fake = fake_claude(HANG);
    let wt = tempfile::tempdir().unwrap();
    let engine = engine_for(&fake);
    let req = request(wt.path(), Duration::from_secs(60));
    let task = tokio::spawn(async move { engine.review(req).await });
    // wait until the fake (and its grandchild) is running, then drop the future via abort
    for _ in 0..200 {
        if !fake.read("child.pid").trim().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    task.abort();
    let _ = task.await;
    let pid: i32 = fake.read("child.pid").trim().parse().unwrap();
    assert!(wait_dead(pid).await, "grandchild {pid} survived drop");
}

#[tokio::test]
async fn missing_cli_or_worktree_is_an_error() {
    let engine = ReviewEngine::new(
        AppSettings {
            claude_cli_path: Some("/nonexistent/claude".into()),
            ..Default::default()
        },
        EventBus::new(),
    );
    let wt = tempfile::tempdir().unwrap();
    let e = engine
        .review(request(wt.path(), Duration::from_secs(5)))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not found"));

    let fake = fake_claude("exit 0");
    let e = engine_for(&fake)
        .review(request(
            Path::new("/nonexistent/worktree"),
            Duration::from_secs(5),
        ))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("does not exist"));
}

#[tokio::test]
async fn status_uses_version_and_auth_status() {
    let fake = fake_claude("exit 0");
    let st = engine_for(&fake).status().await;
    assert_eq!(st.cli_path.as_deref(), Some(fake.script.to_str().unwrap()));
    assert_eq!(st.version.as_deref(), Some("9.9.9"));
    assert!(st.logged_in);
    assert_eq!(st.subscription.as_deref(), Some("max"));
    assert!(st.ok, "{}", st.message);
    assert!(st.message.contains("9.9.9"));
    assert!(st.message.contains("max subscription"));
    // status never runs a prompt
    assert!(!fake.out("args.txt").exists());
}

#[tokio::test]
async fn status_reports_logged_out_and_missing() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("claude");
    write_script(
        &script,
        "#!/bin/sh\ncase \"$1\" in --version) echo '2.0.0 (Claude Code)';; auth) echo '{\"loggedIn\": false}';; esac\n",
    );
    let engine = ReviewEngine::new(
        AppSettings {
            claude_cli_path: Some(script.display().to_string()),
            ..Default::default()
        },
        EventBus::new(),
    );
    let st = engine.status().await;
    assert!(!st.ok);
    assert!(!st.logged_in);
    assert!(st.message.contains("not logged in"));

    // old CLI without `auth status`: ok but unverified
    write_script(&script, "#!/bin/sh\ncase \"$1\" in --version) echo '1.0.0 (Claude Code)';; *) echo 'unknown command' >&2; exit 1;; esac\n");
    let st = engine.status().await;
    assert!(st.ok);
    assert!(st.message.contains("login not verified"));

    let missing = ReviewEngine::new(
        AppSettings {
            claude_cli_path: Some("/nonexistent/claude".into()),
            ..Default::default()
        },
        EventBus::new(),
    );
    let st = missing.status().await;
    assert!(!st.ok);
    assert!(st.cli_path.is_none());
    assert!(st.message.contains("not found at configured path"));
}

#[tokio::test]
async fn status_deep_with_fake_cli() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("claude");
    let args_file = dir.path().join("deep_args.txt");
    write_script(
        &script,
        &format!(
            "#!/bin/sh\ncase \"$1\" in --version) echo '9.9.9 (Claude Code)'; exit 0;; auth) echo '{{\"loggedIn\": true}}'; exit 0;; esac\nprintf '%s\\n' \"$@\" > '{}'\necho '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"OK\",\"num_turns\":1}}'\n",
            args_file.display()
        ),
    );
    let engine = ReviewEngine::new(
        AppSettings {
            claude_cli_path: Some(script.display().to_string()),
            ..Default::default()
        },
        EventBus::new(),
    );
    let st = engine.status_deep().await;
    assert!(st.ok, "{}", st.message);
    assert!(st.logged_in);
    assert!(st.message.contains("Login verified"));
    let args = std::fs::read_to_string(&args_file).unwrap();
    assert!(args.starts_with("-p\n"));
    assert!(args.contains("--max-turns\n1\n"));
    assert!(args.contains("--output-format\njson\n"));

    // failing deep check
    write_script(
        &script,
        "#!/bin/sh\ncase \"$1\" in --version) echo '9.9.9 (Claude Code)'; exit 0;; auth) echo '{\"loggedIn\": true}'; exit 0;; esac\necho '{\"type\":\"result\",\"is_error\":true,\"result\":\"Invalid API key · Please run /login\"}'\nexit 1\n",
    );
    let st = engine.status_deep().await;
    assert!(!st.ok);
    assert!(!st.logged_in);
    assert!(st.message.contains("Deep check failed"));
}

#[test]
fn locate_prefers_configured_path() {
    let fake = fake_claude("exit 0");
    let s = AppSettings {
        claude_cli_path: Some(fake.script.display().to_string()),
        ..Default::default()
    };
    assert_eq!(locate_claude(&s), Some(fake.script.clone()));
    // non-executable configured path is not used
    let plain = fake.dir.path().join("plain");
    std::fs::write(&plain, "x").unwrap();
    let s = AppSettings {
        claude_cli_path: Some(plain.display().to_string()),
        ..Default::default()
    };
    assert_eq!(locate_claude(&s), None);
}

#[test]
fn prompt_starting_with_dash_is_not_an_option() {
    let mut req = request(Path::new("/tmp"), Duration::from_secs(1));
    req.user_prompt = "--dangerously-skip-permissions".into();
    let args = review_args(&req, Path::new("/tmp/c.json"));
    assert_eq!(args[1], " --dangerously-skip-permissions");
    assert!(!args.iter().any(|a| a == "--dangerously-skip-permissions"));
}
