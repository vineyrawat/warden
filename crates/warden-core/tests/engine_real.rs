//! Checks against the REAL Claude Code CLI. `real_status` sends no prompt. `real_status_deep` sends
//! one tiny prompt on the user's subscription, so it is `#[ignore]`d; run it by hand with
//! `cargo test -p warden-core --test engine_real -- --ignored`.

use warden_core::config::AppSettings;
use warden_core::engine::{locate_claude, ReviewEngine};
use warden_core::EventBus;

#[tokio::test]
async fn real_status() {
    let settings = AppSettings::default();
    if locate_claude(&settings).is_none() {
        eprintln!("claude not installed; skipping");
        return;
    }
    let st = ReviewEngine::new(settings, EventBus::new()).status().await;
    eprintln!(
        "status: ok={} version={:?} logged_in={} api_key_in_env={} message={}",
        st.ok, st.version, st.logged_in, st.api_key_in_env, st.message
    );
    assert!(st.cli_path.is_some());
    assert!(st.version.is_some(), "{}", st.message);
}

#[tokio::test]
#[ignore = "sends one real prompt (subscription); run manually"]
async fn real_status_deep() {
    let settings = AppSettings {
        model: "haiku".into(),
        ..Default::default()
    };
    let st = ReviewEngine::new(settings, EventBus::new())
        .status_deep()
        .await;
    eprintln!(
        "status_deep: ok={} logged_in={} message={}",
        st.ok, st.logged_in, st.message
    );
    assert!(st.ok, "{}", st.message);
}
