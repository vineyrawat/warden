//! Hard rule 2: Anthropic credentials / provider switches never reach the `claude` child.
//! Own test binary because it mutates the process environment.

mod engine_common;

use std::time::Duration;

use warden_core::config::AppSettings;
use warden_core::engine::{api_key_in_env, ReviewEngine, ReviewRequest, STRIPPED_ENV};
use warden_core::EventBus;

use engine_common::write_script;

#[tokio::test]
async fn child_env_is_stripped() {
    // SAFETY (edition 2021: set_var is safe): this binary has a single test.
    for k in STRIPPED_ENV {
        std::env::set_var(k, "should-not-leak");
    }
    std::env::set_var("WARDEN_TEST_KEEP", "kept");
    assert!(api_key_in_env());

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("env.txt");
    let script = dir.path().join("claude");
    write_script(
        &script,
        &format!(
            r#"#!/bin/sh
case "$1" in
  --version) echo "9.9.9 (Claude Code)"; exit 0;;
  auth) echo "{{\"loggedIn\": true}}"; echo "auth_key=${{ANTHROPIC_API_KEY-unset}}" >> '{out}.auth'; exit 0;;
esac
{{
  echo "ANTHROPIC_API_KEY=${{ANTHROPIC_API_KEY-unset}}"
  echo "ANTHROPIC_AUTH_TOKEN=${{ANTHROPIC_AUTH_TOKEN-unset}}"
  echo "ANTHROPIC_BASE_URL=${{ANTHROPIC_BASE_URL-unset}}"
  echo "CLAUDE_CODE_USE_BEDROCK=${{CLAUDE_CODE_USE_BEDROCK-unset}}"
  echo "CLAUDE_CODE_USE_VERTEX=${{CLAUDE_CODE_USE_VERTEX-unset}}"
  echo "WARDEN_TEST_KEEP=${{WARDEN_TEST_KEEP-unset}}"
}} > '{out}'
echo '{{"type":"result","subtype":"success","is_error":false,"num_turns":1,"duration_ms":1,"result":"ok"}}'
"#,
            out = out.display()
        ),
    );
    let engine = ReviewEngine::new(
        AppSettings {
            claude_cli_path: Some(script.display().to_string()),
            ..Default::default()
        },
        EventBus::new(),
    );

    // status: flags the key in Warden's env, but the child (auth status) does not see it
    let st = engine.status().await;
    assert!(st.api_key_in_env);
    assert!(st.message.contains("stripped"));
    let auth = std::fs::read_to_string(format!("{}.auth", out.display())).unwrap();
    assert_eq!(auth.trim(), "auth_key=unset");

    let wt = tempfile::tempdir().unwrap();
    let res = engine
        .review(ReviewRequest {
            run_id: 1,
            worktree: wt.path().to_path_buf(),
            mcp_url: "http://127.0.0.1:1/mcp".into(),
            run_token: "t".into(),
            system_prompt: "s".into(),
            user_prompt: "u".into(),
            max_turns: 1,
            timeout: Duration::from_secs(20),
            model: "opus".into(),
        })
        .await
        .unwrap();
    assert!(!res.is_error, "{:?}", res.error);

    let env = std::fs::read_to_string(&out).unwrap();
    for k in STRIPPED_ENV {
        assert!(
            env.contains(&format!("{k}=unset")),
            "{k} leaked into child env:\n{env}"
        );
    }
    assert!(
        env.contains("WARDEN_TEST_KEEP=kept"),
        "other env vars are inherited:\n{env}"
    );
    assert!(!env.contains("should-not-leak"));
}
