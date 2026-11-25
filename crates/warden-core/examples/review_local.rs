//! End-to-end smoke test of the review pipeline with no forge.
//!
//! Opens a temp DB, starts the real MCP server, CI runner and review engine, and runs
//! `Pipeline::review_local` on a local repo.
//!
//! ```sh
//! cargo run -p warden-core --example review_local -- \
//!     --repo /private/tmp/warden-fixture --base main --head feature/import-links \
//!     [--claude /path/to/claude] [--max-turns 12] [--model opus] [--timeout 600]
//! ```
//! `--claude crates/warden-core/examples/fake_claude.py` proves the plumbing without a real model.
//! Without `--claude` the installed Claude Code CLI is used (subscription login; the API-key env
//! vars are stripped from the child by the engine).

use std::sync::Arc;

use warden_core::config::AppSettings;
use warden_core::db::Db;
use warden_core::mcp::{McpServer, RunRegistry};
use warden_core::pipeline::Pipeline;
use warden_core::{AppEvent, EventBus};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

#[tokio::main]
async fn main() -> warden_core::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let repo = arg(&args, "--repo").unwrap_or_else(|| "/private/tmp/warden-fixture".into());
    let base = arg(&args, "--base").unwrap_or_else(|| "main".into());
    let head = arg(&args, "--head").unwrap_or_else(|| "feature/import-links".into());

    let tmp = tempfile::tempdir()?;
    let db = Db::open(&tmp.path().join("warden.db")).await?;
    let mut settings = AppSettings {
        data_dir: tmp.path().to_string_lossy().into(),
        worktree_root: tmp.path().join("worktrees").to_string_lossy().into(),
        ..Default::default()
    };
    settings.claude_cli_path = arg(&args, "--claude");
    if let Some(t) = arg(&args, "--max-turns") {
        settings.max_turns = t.parse().unwrap_or(12);
    } else {
        settings.max_turns = 12;
    }
    if let Some(m) = arg(&args, "--model") {
        settings.model = m;
    }
    if let Some(t) = arg(&args, "--timeout") {
        settings.review_timeout_secs = t.parse().unwrap_or(600);
    }
    db.save_settings(&settings).await?;

    let events = EventBus::new();
    let registry = Arc::new(RunRegistry::new());
    let mcp = McpServer::start(0, registry.clone()).await?;
    let ci = Arc::new(Pipeline::ci_runner(
        &db,
        events.clone(),
        tmp.path().join("logs").join("ci"),
    ));
    let pipeline = Pipeline::new(db.clone(), events.clone(), registry, ci, mcp.url());

    let engine_status = pipeline.engine(&settings).status().await;
    println!(
        "engine: path={:?} version={:?} logged_in={} api_key_in_env={} ({})",
        engine_status.cli_path,
        engine_status.version,
        engine_status.logged_in,
        engine_status.api_key_in_env,
        engine_status.message
    );
    println!("mcp: {}", mcp.url());

    let mut rx = events.subscribe();
    let printer = tokio::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            match ev {
                AppEvent::RunUpdated { run } => println!("[run {}] status={}", run.id, run.status),
                AppEvent::RunProgress {
                    run_id,
                    message,
                    turn,
                } => println!("[run {run_id}] t{turn} {message}"),
                AppEvent::CiLog { job_id, line, .. } => println!("    [ci job {job_id}] {line}"),
                AppEvent::CiJobUpdated { job } => {
                    println!("    [ci] {} -> {}", job.name, job.status)
                }
                AppEvent::Notice { level, message } => println!("[notice:{level}] {message}"),
                _ => {}
            }
        }
    });

    let started = std::time::Instant::now();
    let run = pipeline.review_local(&repo, &base, &head).await?;
    let elapsed = started.elapsed();

    println!("\n================ RESULT ================");
    println!(
        "run {}: status={} turns={} engine_duration_ms={} wall={:.1}s session={:?}",
        run.id,
        run.status,
        run.turns,
        run.duration_ms,
        elapsed.as_secs_f64(),
        run.cli_session_id
    );
    println!("verdict={:?}", run.verdict);
    println!("summary={:?}", run.summary);
    if let Some(e) = &run.error {
        println!("error={e}");
    }
    println!("\nCI jobs:");
    for j in db.list_ci_jobs(run.id).await? {
        println!(
            "  #{} {} [{}] {} ({} ms) {}",
            j.id, j.name, j.kind, j.status, j.duration_ms, j.command
        );
    }
    println!("\nFindings:");
    for f in db.list_findings(run.id).await? {
        println!(
            "  #{} [{}] {} {} {}:{} rule={:?} conf={:.2} pos={} {}{}",
            f.id,
            f.source,
            f.status,
            f.severity,
            f.file,
            f.line,
            f.rule_id,
            f.confidence,
            f.position.is_some(),
            f.title,
            f.guardrail_reason
                .as_deref()
                .map(|r| format!("  <- {r}"))
                .unwrap_or_default()
        );
    }
    let audit = db.list_audit(Some(run.id), 500).await?;
    let mut kinds = std::collections::BTreeMap::<String, u32>::new();
    for a in &audit {
        *kinds.entry(a.kind.clone()).or_default() += 1;
    }
    println!("\nAudit events: {kinds:?}");

    printer.abort();
    mcp.stop().await;
    Ok(())
}
