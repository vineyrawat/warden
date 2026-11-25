//! stream-json parser on canned Claude Code output.

use serde_json::json;
use warden_core::engine::stream::{describe_tool_use, parse_line, StreamEvent};
use warden_core::engine::StreamState;

const INIT: &str = r#"{"type":"system","subtype":"init","cwd":"/tmp/wt","session_id":"sess-123","tools":["Read","Grep","Glob","mcp__warden__mr_get_overview","mcp__warden__review_submit"],"mcp_servers":[{"name":"warden","status":"connected"}],"model":"claude-opus-4","permissionMode":"default","apiKeySource":"none"}"#;
const ASSISTANT_TOOLS: &str = r#"{"type":"assistant","message":{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"Let me look at the MR."},{"type":"tool_use","id":"toolu_1","name":"mcp__warden__mr_get_overview","input":{}},{"type":"tool_use","id":"toolu_2","name":"mcp__warden__mr_get_diff","input":{"file":"app/api.py","page":2}}]},"session_id":"sess-123"}"#;
const USER_RESULT: &str = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_1","type":"tool_result","content":[{"type":"text","text":"..."}],"is_error":false},{"tool_use_id":"toolu_2","type":"tool_result","content":"bad","is_error":true}]},"session_id":"sess-123"}"#;
const SUBMIT: &str = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_9","name":"mcp__warden__review_submit","input":{"summary":"One blocker: SQL injection.","verdict":"request_changes"}}]},"session_id":"sess-123"}"#;
const RESULT_OK: &str = r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":81234,"duration_api_ms":70000,"num_turns":7,"result":"Review submitted.","session_id":"sess-123","total_cost_usd":0.42,"usage":{"input_tokens":10}}"#;
const RESULT_MAX_TURNS: &str = r#"{"type":"result","subtype":"error_max_turns","duration_ms":5000,"num_turns":30,"session_id":"sess-9"}"#;

#[test]
fn parses_init() {
    let evs = parse_line(INIT);
    assert_eq!(evs.len(), 1);
    match &evs[0] {
        StreamEvent::Init {
            session_id,
            tools,
            mcp_servers,
            model,
        } => {
            assert_eq!(session_id.as_deref(), Some("sess-123"));
            assert!(tools.contains(&"mcp__warden__review_submit".to_string()));
            assert_eq!(
                mcp_servers,
                &vec![("warden".to_string(), "connected".to_string())]
            );
            assert_eq!(model.as_deref(), Some("claude-opus-4"));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn parses_assistant_blocks_in_order() {
    let evs = parse_line(ASSISTANT_TOOLS);
    assert_eq!(evs.len(), 4);
    assert_eq!(evs[0], StreamEvent::AssistantTurn);
    assert_eq!(evs[1], StreamEvent::Text("Let me look at the MR.".into()));
    assert_eq!(
        evs[2],
        StreamEvent::ToolUse {
            id: Some("toolu_1".into()),
            name: "mcp__warden__mr_get_overview".into(),
            input: json!({}),
        }
    );
    match &evs[3] {
        StreamEvent::ToolUse { name, input, .. } => {
            assert_eq!(name, "mcp__warden__mr_get_diff");
            assert_eq!(input["page"], 2);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn parses_tool_results() {
    let evs = parse_line(USER_RESULT);
    assert_eq!(
        evs,
        vec![
            StreamEvent::ToolResult {
                tool_use_id: Some("toolu_1".into()),
                is_error: false
            },
            StreamEvent::ToolResult {
                tool_use_id: Some("toolu_2".into()),
                is_error: true
            },
        ]
    );
}

#[test]
fn parses_results() {
    match &parse_line(RESULT_OK)[0] {
        StreamEvent::Result {
            subtype,
            num_turns,
            duration_ms,
            is_error,
            result,
            session_id,
        } => {
            assert_eq!(subtype.as_deref(), Some("success"));
            assert_eq!(*num_turns, 7);
            assert_eq!(*duration_ms, 81234);
            assert!(!is_error);
            assert_eq!(result.as_deref(), Some("Review submitted."));
            assert_eq!(session_id.as_deref(), Some("sess-123"));
        }
        other => panic!("unexpected {other:?}"),
    }
    // Missing is_error: derived from subtype.
    match &parse_line(RESULT_MAX_TURNS)[0] {
        StreamEvent::Result {
            is_error,
            num_turns,
            ..
        } => {
            assert!(*is_error);
            assert_eq!(*num_turns, 30);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn ignores_noise() {
    assert!(parse_line("").is_empty());
    assert_eq!(parse_line("Warning: something"), vec![StreamEvent::Other]);
    assert_eq!(
        parse_line(r#"{"type":"system","subtype":"hook_started"}"#),
        vec![StreamEvent::Other]
    );
    assert_eq!(
        parse_line(r#"{"type":"user","message":{"content":"plain text"}}"#),
        vec![StreamEvent::Other]
    );
}

#[test]
fn state_folds_a_full_session() {
    let mut st = StreamState::default();
    let mut progress = Vec::new();
    for l in [INIT, ASSISTANT_TOOLS, USER_RESULT, SUBMIT, RESULT_OK] {
        for ev in parse_line(l) {
            if let Some(m) = st.apply(ev) {
                progress.push((m, st.turns));
            }
        }
    }
    assert_eq!(st.turns, 2);
    assert_eq!(st.session_id.as_deref(), Some("sess-123"));
    assert!(st.submitted);
    assert_eq!(st.verdict.as_deref(), Some("request_changes"));
    assert_eq!(st.summary.as_deref(), Some("One blocker: SQL injection."));
    assert_eq!(st.tool_calls, 3);
    assert!(st.mcp_problem.is_none());
    let msgs: Vec<&str> = progress.iter().map(|(m, _)| m.as_str()).collect();
    assert!(msgs[0].starts_with("Claude session started"));
    assert!(msgs.contains(&"mr_get_overview"));
    assert!(msgs.contains(&"mr_get_diff (file=app/api.py, page=2)"));
    assert!(msgs.contains(&"review_submit (request_changes)"));
    // turn counter attached to progress
    assert_eq!(
        progress
            .iter()
            .find(|(m, _)| m == "mr_get_overview")
            .unwrap()
            .1,
        1
    );
    assert_eq!(progress.last().unwrap().1, 2);
    let r = st.result.clone().unwrap();
    let (turns, dur, err) = (r.num_turns, r.duration_ms, r.is_error);
    assert_eq!((turns, dur, err), (7, 81234, false));
}

#[test]
fn rejected_submit_is_not_counted() {
    let mut st = StreamState::default();
    for ev in parse_line(SUBMIT) {
        st.apply(ev);
    }
    assert!(st.submitted);
    for ev in parse_line(
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_9","is_error":true}]}}"#,
    ) {
        st.apply(ev);
    }
    assert!(!st.submitted);
    assert!(st.verdict.is_none());
}

#[test]
fn mcp_failure_is_flagged() {
    let mut st = StreamState::default();
    let line = r#"{"type":"system","subtype":"init","session_id":"s","tools":[],"mcp_servers":[{"name":"warden","status":"failed"}]}"#;
    let msgs: Vec<String> = parse_line(line)
        .into_iter()
        .filter_map(|e| st.apply(e))
        .collect();
    assert_eq!(
        st.mcp_problem.as_deref(),
        Some("warden MCP server status: failed")
    );
    assert!(msgs[0].contains("not connected"));
}

#[test]
fn describes_tools() {
    assert_eq!(
        describe_tool_use(
            "mcp__warden__review_add_finding",
            &json!({"file":"a.py","line":3,"severity":"major","title":"Missing permission check"})
        ),
        "review_add_finding (major a.py:3 Missing permission check)"
    );
    assert_eq!(
        describe_tool_use(
            "mcp__warden__repo_search",
            &json!({"query":"has_permission"})
        ),
        "repo_search (\"has_permission\")"
    );
    assert_eq!(
        describe_tool_use("Read", &json!({"file_path":"/x/y.py"})),
        "Read (/x/y.py)"
    );
    assert_eq!(
        describe_tool_use("mcp__warden__ci_get_results", &json!({})),
        "ci_get_results"
    );
}
