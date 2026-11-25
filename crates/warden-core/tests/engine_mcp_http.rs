//! MCP server end to end, in process: start on port 0, register a run, speak streamable HTTP
//! (JSON-RPC over POST; responses may be SSE) the way Claude Code does.

mod engine_common;

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderMap, CONTENT_TYPE};
use reqwest::StatusCode;
use serde_json::{json, Value};
use warden_core::mcp::{McpServer, RunRegistry, WardenTools};

use engine_common::{make_ctx, write_worktree};

const PROTOCOL: &str = "2025-06-18";

struct Client {
    http: reqwest::Client,
    url: String,
    token: Option<String>,
    session: Option<String>,
    next_id: u64,
}

impl Client {
    fn new(url: &str, token: Option<&str>) -> Self {
        Self {
            http: reqwest::Client::new(),
            url: url.to_string(),
            token: token.map(str::to_string),
            session: None,
            next_id: 1,
        }
    }

    fn request(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut r = self
            .http
            .post(&self.url)
            .header("Accept", "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_string());
        if let Some(t) = &self.token {
            r = r.header("X-Warden-Run", t);
        }
        if let Some(s) = &self.session {
            r = r
                .header("Mcp-Session-Id", s)
                .header("MCP-Protocol-Version", PROTOCOL);
        }
        r
    }

    /// Sends a JSON-RPC request and returns (status, headers, response message).
    async fn call(
        &mut self,
        method: &str,
        params: Value,
    ) -> (StatusCode, HeaderMap, Option<Value>) {
        let id = self.next_id;
        self.next_id += 1;
        let body = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let mut resp = self.request(&body).send().await.expect("http");
        let status = resp.status();
        let headers = resp.headers().clone();
        if !status.is_success() {
            return (status, headers, None);
        }
        let ct = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if ct.starts_with("application/json") {
            let v: Value = resp.json().await.unwrap();
            return (status, headers, Some(v));
        }
        assert!(ct.starts_with("text/event-stream"), "content-type {ct}");
        // Read SSE events until the response with our id arrives.
        let fut = async {
            let mut buf = String::new();
            loop {
                let chunk = resp.chunk().await.expect("sse chunk");
                let chunk = chunk?;
                buf.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(idx) = buf.find("\n\n") {
                    let event: String = buf[..idx].to_string();
                    buf.drain(..idx + 2);
                    let data: String = event
                        .lines()
                        .filter_map(|l| l.strip_prefix("data:"))
                        .map(str::trim_start)
                        .collect::<Vec<_>>()
                        .join("\n");
                    if data.trim().is_empty() {
                        continue;
                    }
                    let v: Value = serde_json::from_str(&data).expect("sse json");
                    if v.get("id") == Some(&json!(id)) {
                        return Some(v);
                    }
                }
            }
        };
        let msg = tokio::time::timeout(Duration::from_secs(10), fut)
            .await
            .expect("sse timeout");
        (status, headers, msg)
    }

    async fn notify(&self, method: &str) -> StatusCode {
        let body = json!({"jsonrpc":"2.0","method":method});
        self.request(&body).send().await.unwrap().status()
    }

    async fn initialize(&mut self) {
        let (status, headers, msg) = self
            .call(
                "initialize",
                json!({"protocolVersion":PROTOCOL,"capabilities":{},"clientInfo":{"name":"claude-code-test","version":"1.0"}}),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let msg = msg.unwrap();
        assert_eq!(msg["result"]["serverInfo"]["name"], "warden");
        assert!(msg["result"]["capabilities"]["tools"].is_object());
        assert!(msg["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("untrusted_mr_content"));
        self.session = headers
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let st = self.notify("notifications/initialized").await;
        assert!(st.is_success(), "initialized -> {st}");
    }

    /// tools/call -> (is_error, text)
    async fn tool(&mut self, name: &str, args: Value) -> (bool, String) {
        let (status, _, msg) = self
            .call("tools/call", json!({"name":name,"arguments":args}))
            .await;
        assert_eq!(status, StatusCode::OK);
        let msg = msg.unwrap();
        if let Some(err) = msg.get("error") {
            return (true, format!("protocol error: {err}"));
        }
        let r = &msg["result"];
        let text = r["content"][0]["text"].as_str().unwrap_or("").to_string();
        (r["isError"].as_bool().unwrap_or(false), text)
    }
}

#[tokio::test]
async fn end_to_end_over_http() {
    let dir = tempfile::tempdir().unwrap();
    write_worktree(dir.path());
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "TOP SECRET").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        dir.path().join("evil_link"),
    )
    .unwrap();

    let registry = Arc::new(RunRegistry::new());
    let (ctx, mut rx) = make_ctx(dir.path());
    let token = registry.register(ctx);
    assert_eq!(token.len(), 64);
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));

    let server = McpServer::start(0, registry.clone()).await.unwrap();
    let url = server.url();
    assert!(url.starts_with("http://127.0.0.1:"));
    assert!(url.ends_with("/mcp"));
    assert_ne!(server.port(), 0);

    // --- missing / unknown header -> HTTP 401 before any MCP processing
    let mut anon = Client::new(&url, None);
    let (st, _, _) = anon
        .call("initialize", json!({"protocolVersion":PROTOCOL,"capabilities":{},"clientInfo":{"name":"x","version":"1"}}))
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let mut wrong = Client::new(&url, Some("deadbeef"));
    let (st, _, _) = wrong
        .call("initialize", json!({"protocolVersion":PROTOCOL,"capabilities":{},"clientInfo":{"name":"x","version":"1"}}))
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // --- proper client
    let mut c = Client::new(&url, Some(&token));
    c.initialize().await;

    let (st, _, msg) = c.call("tools/list", json!({})).await;
    assert_eq!(st, StatusCode::OK);
    let tools = msg.unwrap()["result"]["tools"].as_array().unwrap().clone();
    let mut names: Vec<String> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "ci_get_results",
            "ci_run_recipe",
            "mr_get_diff",
            "mr_get_overview",
            "repo_read_file",
            "repo_search",
            "review_add_finding",
            "review_submit"
        ]
    );
    let schema_of = |n: &str| tools.iter().find(|t| t["name"] == n).unwrap()["inputSchema"].clone();
    let add = schema_of("review_add_finding");
    assert_eq!(add["type"], "object");
    let req: Vec<&str> = add["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for f in [
        "file",
        "line",
        "severity",
        "category",
        "title",
        "body",
        "confidence",
    ] {
        assert!(req.contains(&f), "{f} should be required");
    }
    assert!(add["properties"]["rule_id"].is_object());
    assert!(
        add.to_string().contains("blocker"),
        "severity enum in schema"
    );
    let rf = schema_of("repo_read_file");
    assert!(rf["properties"]["ref"].is_object());
    assert_eq!(rf["required"], json!(["path"]));
    let sub = schema_of("review_submit");
    assert!(sub.to_string().contains("request_changes"));
    assert!(tools
        .iter()
        .all(|t| !t["description"].as_str().unwrap_or("").is_empty()));

    // overview
    let (err, text) = c.tool("mr_get_overview", json!({})).await;
    assert!(!err, "{text}");
    assert!(text.contains("Merge request !42"));
    assert!(text.contains("<untrusted_mr_content field=\"description\">"));

    // diff
    let (err, text) = c.tool("mr_get_diff", json!({"file":"app/api.py"})).await;
    assert!(!err);
    assert!(text.contains("+@frappe.whitelist()"));
    let (err, _) = c.tool("mr_get_diff", json!({"page": 5})).await;
    assert!(err);

    // read file + traversal rejection
    let (err, text) = c
        .tool(
            "repo_read_file",
            json!({"path":"app/api.py","start_line":3,"end_line":4}),
        )
        .await;
    assert!(!err, "{text}");
    assert!(text.contains("     3\t@frappe.whitelist()"));
    for bad in ["../../etc/passwd", "/etc/passwd", "evil_link"] {
        let (err, text) = c.tool("repo_read_file", json!({"path": bad})).await;
        assert!(err, "{bad} should fail");
        assert!(
            !text.contains("TOP SECRET") && !text.contains("root:"),
            "{bad} leaked: {text}"
        );
    }
    // bad argument type -> error result, not a crash
    let (err, _) = c.tool("repo_read_file", json!({"path": 5})).await;
    assert!(err);

    // search
    let (err, text) = c
        .tool("repo_search", json!({"query":"whitelist","glob":"**/*.py"}))
        .await;
    assert!(!err, "{text}");
    assert!(text.contains("app/api.py:3:"));

    // ci
    let (err, text) = c.tool("ci_get_results", json!({})).await;
    assert!(!err);
    assert!(text.contains("reruns_left"));
    let (err, text) = c.tool("ci_run_recipe", json!({"name":"lint"})).await;
    assert!(err);
    assert!(text.contains("unknown recipe"));

    // findings
    let finding = json!({
        "file":"app/api.py","line":5,"severity":"blocker","category":"security","rule_id":"FR-002",
        "title":"SQL injection via f-string","body":"name is interpolated into SQL; pass it as a parameter.",
        "evidence":"frappe.db.sql(f\"...{name}\")","confidence":0.97
    });
    let (err, text) = c.tool("review_add_finding", finding.clone()).await;
    assert!(!err, "{text}");
    assert!(text.contains("\"accepted\""));
    let nf = rx.try_recv().expect("finding forwarded to findings_tx");
    assert_eq!(nf.rule_id.as_deref(), Some("FR-002"));
    assert_eq!(nf.source, "claude");
    let (err, text) = c.tool("review_add_finding", finding).await;
    assert!(err);
    assert!(text.contains("duplicate"));
    let (err, text) = c
        .tool(
            "review_add_finding",
            json!({"file":"app/api.py","line":99,"severity":"major","category":"x","title":"t","body":"b","confidence":0.9}),
        )
        .await;
    assert!(err);
    assert!(text.contains("not inside a diff hunk"));
    let (err, _) = c
        .tool(
            "review_add_finding",
            json!({"file":"app/api.py","line":5,"severity":"catastrophic","category":"x","title":"t","body":"b","confidence":0.9}),
        )
        .await;
    assert!(err, "invalid severity enum is rejected");

    // submit
    let (err, _) = c
        .tool("review_submit", json!({"summary":"x","verdict":"lgtm"}))
        .await;
    assert!(err, "invalid verdict rejected");
    assert!(registry.submission(&token).is_none());
    let (err, text) = c
        .tool(
            "review_submit",
            json!({"summary":"One blocker (SQL injection).","verdict":"request_changes"}),
        )
        .await;
    assert!(!err, "{text}");
    let s = registry.submission(&token).unwrap();
    assert_eq!(s.verdict, "request_changes");
    assert!(registry.get(&token).unwrap().state_snapshot().tool_calls >= 10);

    // after the run is removed, the same client is refused
    registry.remove(&token);
    let (st, _, _) = c.call("tools/list", json!({})).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    server.stop().await;
    // port is released
    assert!(reqwest::Client::new()
        .post(&url)
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .is_err());
}

#[tokio::test]
async fn two_runs_are_isolated() {
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    write_worktree(d1.path());
    write_worktree(d2.path());
    std::fs::write(d2.path().join("only_in_two.txt"), "run two\n").unwrap();
    let registry = Arc::new(RunRegistry::new());
    let (c1, _rx1) = make_ctx(d1.path());
    let (mut c2, _rx2) = make_ctx(d2.path());
    c2.run_id = 22;
    let t1 = registry.register(c1);
    let t2 = registry.register(c2);
    assert_ne!(t1, t2);
    let server = McpServer::start(0, registry.clone()).await.unwrap();

    let mut a = Client::new(&server.url(), Some(&t1));
    a.initialize().await;
    let mut b = Client::new(&server.url(), Some(&t2));
    b.initialize().await;
    let (err, _) = a
        .tool("repo_read_file", json!({"path":"only_in_two.txt"}))
        .await;
    assert!(err);
    let (err, text) = b
        .tool("repo_read_file", json!({"path":"only_in_two.txt"}))
        .await;
    assert!(!err && text.contains("run two"));
    let (_, text) = b.tool("mr_get_overview", json!({})).await;
    assert!(text.contains("(run 22)"));
    server.stop().await;
}

#[tokio::test]
async fn busy_port_is_an_error() {
    let registry = Arc::new(RunRegistry::new());
    let s1 = McpServer::start(0, registry.clone()).await.unwrap();
    let err = McpServer::start(s1.port(), registry.clone())
        .await
        .err()
        .unwrap();
    assert!(err.to_string().contains("cannot bind"));
    s1.stop().await;
}

#[test]
fn tool_handler_rejects_missing_or_unknown_header() {
    let registry = Arc::new(RunRegistry::new());
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _rx) = make_ctx(dir.path());
    let token = registry.register(ctx);
    let tools = WardenTools::new(registry.clone());

    let (parts, _) = axum::http::Request::builder()
        .body(())
        .unwrap()
        .into_parts();
    let e = tools.context(&parts).err().unwrap();
    assert!(e.message.contains("missing X-Warden-Run"));

    let (parts, _) = axum::http::Request::builder()
        .header("X-Warden-Run", "nope")
        .body(())
        .unwrap()
        .into_parts();
    assert!(tools
        .context(&parts)
        .err()
        .unwrap()
        .message
        .contains("unknown"));

    let (parts, _) = axum::http::Request::builder()
        .header("x-warden-run", &token)
        .body(())
        .unwrap()
        .into_parts();
    assert_eq!(tools.context(&parts).unwrap().run_id, 11);
    assert_eq!(registry.len(), 1);
    registry.remove(&token);
    assert!(registry.is_empty());
    assert!(tools.context(&parts).is_err());
}
