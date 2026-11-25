// OWNER: integration
//! Menu-bar tray: Open Warden, Poll now, Pause/Resume posting, Quit.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Wry};

use crate::state::Shared;

const ID_OPEN: &str = "open";
const ID_POLL: &str = "poll";
const ID_PAUSE: &str = "pause";
const ID_QUIT: &str = "quit";

/// Managed handle so the pause item's label follows the setting.
pub struct TrayHandles {
    pub pause: MenuItem<Wry>,
}

fn pause_label(paused: bool) -> &'static str {
    if paused {
        "Resume posting"
    } else {
        "Pause posting"
    }
}

pub fn build(app: &AppHandle, paused: bool) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, ID_OPEN, "Open Warden", true, None::<&str>)?;
    let poll = MenuItem::with_id(app, ID_POLL, "Poll now", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, ID_PAUSE, pause_label(paused), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit Warden", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &poll, &pause, &sep, &quit])?;

    let mut builder = TrayIconBuilder::with_id("warden-tray")
        .tooltip("Warden")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            ID_OPEN => show_main(app),
            ID_POLL => {
                if let Some(state) = app.try_state::<Shared>() {
                    state.request_poll(None);
                }
            }
            ID_PAUSE => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(state) = app.try_state::<Shared>() {
                        let state: Shared = state.inner().clone();
                        let paused = !state.settings().posting_paused;
                        if let Err(e) = crate::set_paused(&app, &state, paused).await {
                            tracing::warn!("toggle posting pause failed: {e}");
                        }
                    }
                });
            }
            ID_QUIT => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.manage(TrayHandles { pause });
    Ok(())
}

pub fn set_paused_label(app: &AppHandle, paused: bool) {
    if let Some(h) = app.try_state::<TrayHandles>() {
        let _ = h.pause.set_text(pause_label(paused));
    }
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}
