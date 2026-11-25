// OWNER: engine
//! Parser for `claude -p --output-format stream-json --verbose` lines.
//!
//! Shapes (Claude Code 2.x):
//! - `{"type":"system","subtype":"init","session_id":"…","tools":["Read",…],"mcp_servers":[{"name":"warden","status":"connected"}],"model":"…"}`
//! - `{"type":"assistant","message":{"content":[{"type":"text","text":"…"},{"type":"tool_use","id":"toolu_…","name":"mcp__warden__mr_get_diff","input":{…}}]},"session_id":"…"}`
//! - `{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_…","is_error":false,"content":…}]}}`
//! - `{"type":"result","subtype":"success","is_error":false,"num_turns":7,"duration_ms":81234,"result":"…","session_id":"…","total_cost_usd":…}`

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StreamEvent {
    /// `{"type":"system","subtype":"init",...}`
    Init {
        session_id: Option<String>,
        tools: Vec<String>,
        /// `(name, status)` of each MCP server, e.g. `("warden", "connected")`.
        mcp_servers: Vec<(String, String)>,
        model: Option<String>,
    },
    /// assistant message `tool_use` block
    ToolUse {
        id: Option<String>,
        name: String,
        input: serde_json::Value,
    },
    /// user message `tool_result` block
    ToolResult {
        tool_use_id: Option<String>,
        is_error: bool,
    },
    /// assistant message `text` block
    Text(String),
    /// Marks the start of one assistant message (one model turn). Emitted before its blocks.
    AssistantTurn,
    /// `{"type":"result",...}`
    Result {
        subtype: Option<String>,
        num_turns: u32,
        duration_ms: u64,
        is_error: bool,
        result: Option<String>,
        session_id: Option<String>,
    },
    Other,
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_string)
}

/// Parses one stream-json line into zero or more events (an assistant message can hold several blocks).
/// Blank lines yield nothing; non-JSON lines yield `Other`.
pub fn parse_line(line: &str) -> Vec<StreamEvent> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return vec![StreamEvent::Other];
    };
    match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
        "system" if v.get("subtype").and_then(|t| t.as_str()) == Some("init") => {
            let tools = v
                .get("tools")
                .and_then(|t| t.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string).or_else(|| s(x, "name")))
                        .collect()
                })
                .unwrap_or_default();
            let mcp_servers = v
                .get("mcp_servers")
                .and_then(|t| t.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| Some((s(x, "name")?, s(x, "status").unwrap_or_default())))
                        .collect()
                })
                .unwrap_or_default();
            vec![StreamEvent::Init {
                session_id: s(&v, "session_id"),
                tools,
                mcp_servers,
                model: s(&v, "model"),
            }]
        }
        "assistant" => {
            let mut out = vec![StreamEvent::AssistantTurn];
            let content = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array());
            for block in content.into_iter().flatten() {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("tool_use") | Some("server_tool_use") => {
                        if let Some(name) = s(block, "name") {
                            out.push(StreamEvent::ToolUse {
                                id: s(block, "id"),
                                name,
                                input: block.get("input").cloned().unwrap_or(Value::Null),
                            });
                        }
                    }
                    Some("text") => {
                        if let Some(t) = s(block, "text") {
                            out.push(StreamEvent::Text(t));
                        }
                    }
                    _ => {}
                }
            }
            out
        }
        "user" => {
            let content = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array());
            let out: Vec<StreamEvent> = content
                .into_iter()
                .flatten()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                .map(|b| StreamEvent::ToolResult {
                    tool_use_id: s(b, "tool_use_id"),
                    is_error: b.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false),
                })
                .collect();
            if out.is_empty() {
                vec![StreamEvent::Other]
            } else {
                out
            }
        }
        "result" => {
            let subtype = s(&v, "subtype");
            let is_error = v
                .get("is_error")
                .and_then(|x| x.as_bool())
                .unwrap_or_else(|| subtype.as_deref().is_some_and(|st| st != "success"));
            vec![StreamEvent::Result {
                subtype,
                num_turns: v.get("num_turns").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
                duration_ms: v.get("duration_ms").and_then(|x| x.as_u64()).unwrap_or(0),
                is_error,
                result: s(&v, "result"),
                session_id: s(&v, "session_id"),
            }]
        }
        _ => vec![StreamEvent::Other],
    }
}

/// `mcp__warden__mr_get_diff` -> `mr_get_diff`; other names unchanged.
pub fn short_tool_name(name: &str) -> &str {
    name.strip_prefix("mcp__warden__").unwrap_or(name)
}

/// One-line progress text for a tool call, e.g. `mr_get_diff (file=app/api.py)`.
pub fn describe_tool_use(name: &str, input: &Value) -> String {
    let short = short_tool_name(name);
    let detail = match short {
        "mr_get_diff" => {
            let mut parts = Vec::new();
            if let Some(f) = input.get("file").and_then(|x| x.as_str()) {
                parts.push(format!("file={f}"));
            }
            if let Some(p) = input.get("page").and_then(|x| x.as_u64()) {
                parts.push(format!("page={p}"));
            }
            parts.join(", ")
        }
        "repo_read_file" | "Read" => input
            .get("path")
            .or_else(|| input.get("file_path"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        "repo_search" | "Grep" => input
            .get("query")
            .or_else(|| input.get("pattern"))
            .and_then(|x| x.as_str())
            .map(|q| format!("{q:?}"))
            .unwrap_or_default(),
        "Glob" => input
            .get("pattern")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        "ci_run_recipe" => input
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        "review_add_finding" => {
            let file = input.get("file").and_then(|x| x.as_str()).unwrap_or("?");
            let line = input.get("line").and_then(|x| x.as_u64()).unwrap_or(0);
            let sev = input
                .get("severity")
                .and_then(|x| x.as_str())
                .unwrap_or("?");
            let title = input.get("title").and_then(|x| x.as_str()).unwrap_or("");
            format!("{sev} {file}:{line} {title}")
        }
        "review_submit" => input
            .get("verdict")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        _ => String::new(),
    };
    let detail: String = detail.chars().take(160).collect();
    if detail.is_empty() {
        short.to_string()
    } else {
        format!("{short} ({detail})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_and_garbage() {
        assert!(parse_line("").is_empty());
        assert!(parse_line("   ").is_empty());
        assert_eq!(parse_line("not json"), vec![StreamEvent::Other]);
        assert_eq!(parse_line(r#"{"type":"weird"}"#), vec![StreamEvent::Other]);
    }

    #[test]
    fn short_names() {
        assert_eq!(
            short_tool_name("mcp__warden__review_submit"),
            "review_submit"
        );
        assert_eq!(short_tool_name("Read"), "Read");
    }
}
