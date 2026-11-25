// OWNER: integration
//! Warden desktop app: state + background loops (state.rs), Tauri commands (commands.rs), tray
//! (tray.rs). Closing the window hides it; the app keeps polling from the menu-bar tray.

pub mod commands;
pub mod state;
pub mod tray;

use tauri::{AppHandle, Manager, RunEvent, WindowEvent};

use warden_core::Result;

use crate::state::Shared;

/// Shared by the `set_posting_paused` command and the tray toggle.
pub async fn set_paused(app: &AppHandle, state: &Shared, paused: bool) -> Result<()> {
    let mut s = state.db.get_settings().await?;
    s.posting_paused = paused;
    state.db.save_settings(&s).await?;
    state.set_settings_cache(&s);
    tray::set_paused_label(app, paused);
    let _ = state
        .db
        .audit(
            None,
            "post",
            serde_json::json!({"event": if paused { "posting_paused" } else { "posting_resumed" }}),
        )
        .await;
    state.notice(
        if paused { "warn" } else { "info" },
        if paused {
            "Posting paused: reviews continue, nothing is written to the forge"
        } else {
            "Posting resumed"
        },
    );
    Ok(())
}

/// Logs go to `<data dir>/logs/warden.log` (and stderr). `RUST_LOG` overrides the default level.
fn init_logging() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,warden=info,warden_lib=info,warden_core=info"));
    let dir = state::default_data_dir().join("logs");
    let file = std::fs::create_dir_all(&dir)
        .ok()
        .and_then(|_| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("warden.log"))
                .ok()
        });
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr));
    let res = match file {
        Some(f) => registry
            .with(fmt::layer().with_ansi(false).with_writer(std::sync::Mutex::new(f)))
            .try_init(),
        None => registry.try_init(),
    };
    let _ = res;
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let (state, queue_rx, poll_rx) = tauri::async_runtime::block_on(state::init())
                .map_err(|e| Box::new(std::io::Error::other(e.to_string())))?;
            let paused = state.settings().posting_paused;
            app.manage::<Shared>(state.clone());
            state::spawn_background(app.handle().clone(), state, queue_rx, poll_rx);
            if let Err(e) = tray::build(app.handle(), paused) {
                tracing::warn!("tray icon unavailable: {e}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_engine_status,
            commands::check_engine_deep,
            commands::detect_tools,
            commands::get_usage,
            commands::get_settings,
            commands::save_settings,
            commands::set_forge_token,
            commands::forge_status,
            commands::search_remote_projects,
            commands::discover_projects,
            commands::add_projects,
            commands::open_mr_link,
            commands::pick_folder,
            commands::list_projects,
            commands::save_project,
            commands::delete_project,
            commands::get_project_config,
            commands::get_project_overrides,
            commands::save_project_overrides,
            commands::poll_now,
            commands::list_mrs,
            commands::start_review,
            commands::review_local,
            commands::cancel_run,
            commands::get_run,
            commands::list_runs,
            commands::update_finding,
            commands::set_finding_status,
            commands::post_findings,
            commands::rerun_ci_job,
            commands::read_ci_log,
            commands::list_audit,
            commands::set_posting_paused,
            commands::list_run_rows,
            commands::get_stats,
            commands::rule_stats,
            commands::simulate_guardrails,
            commands::preview_note,
            commands::list_pending_findings,
            commands::post_findings_bulk,
            commands::open_settings_window,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Warden");

    app.run(|app, event| match event {
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => tray::show_main(app),
        RunEvent::Exit => {
            if let Some(state) = app.try_state::<Shared>() {
                let state: Shared = state.inner().clone();
                tauri::async_runtime::block_on(async move {
                    if let Some(mcp) = state.mcp.lock().await.take() {
                        mcp.stop().await;
                    }
                });
            }
        }
        _ => {}
    });
}
