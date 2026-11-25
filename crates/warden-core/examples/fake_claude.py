#!/usr/bin/env python3
"""Fake `claude` CLI for the review_local smoke example.

It answers `--version` / `auth status`, and for a review (`-p ... --mcp-config <file>`) it talks to
Warden's real MCP server over streamable HTTP exactly like Claude Code would: initialize, tools/list,
mr_get_overview, mr_get_diff, ci_get_results, review_add_finding (one finding that the guardrails
must reject, one valid), review_submit. It prints stream-json events on stdout.

It never contacts anything except the MCP URL in the --mcp-config file (127.0.0.1).
"""
import json
import sys
import urllib.request

PROTOCOL = "2025-06-18"


def out(obj):
    print(json.dumps(obj), flush=True)


def main(argv):
    if "--version" in argv:
        print("9.9.9 (Claude Code, fake)")
        return 0
    if argv[:2] == ["auth", "status"]:
        print(json.dumps({"loggedIn": True, "authMethod": "fake", "subscriptionType": "fake"}))
        return 0
    if "--mcp-config" not in argv:
        print(json.dumps({"type": "result", "subtype": "success", "is_error": False,
                          "result": "OK", "num_turns": 1}))
        return 0

    cfg = json.load(open(argv[argv.index("--mcp-config") + 1]))
    server = cfg["mcpServers"]["warden"]
    url, headers = server["url"], dict(server.get("headers", {}))
    session = {"id": None}
    counter = {"n": 0}

    def rpc(method, params=None, notify=False):
        body = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            body["params"] = params
        if not notify:
            counter["n"] += 1
            body["id"] = counter["n"]
        h = {"Content-Type": "application/json",
             "Accept": "application/json, text/event-stream",
             "MCP-Protocol-Version": PROTOCOL, **headers}
        if session["id"]:
            h["Mcp-Session-Id"] = session["id"]
        req = urllib.request.Request(url, data=json.dumps(body).encode(), headers=h, method="POST")
        with urllib.request.urlopen(req, timeout=60) as resp:
            sid = resp.headers.get("mcp-session-id")
            if sid:
                session["id"] = sid
            raw = resp.read().decode()
        if notify:
            return None
        if raw.lstrip().startswith("{"):
            return json.loads(raw)
        for line in raw.splitlines():
            if line.startswith("data:"):
                data = line[5:].strip()
                if data:
                    msg = json.loads(data)
                    if msg.get("id") == body["id"]:
                        return msg
        raise RuntimeError("no response for %s: %r" % (method, raw[:300]))

    rpc("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                       "clientInfo": {"name": "fake-claude", "version": "9.9.9"}})
    rpc("notifications/initialized", notify=True)
    tools = [t["name"] for t in rpc("tools/list", {})["result"]["tools"]]
    out({"type": "system", "subtype": "init", "session_id": "fake-session", "model": "fake",
         "tools": ["mcp__warden__" + t for t in tools],
         "mcp_servers": [{"name": "warden", "status": "connected"}]})

    turn = {"n": 0}

    def call(name, args):
        turn["n"] += 1
        tid = "tu%d" % turn["n"]
        out({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": tid, "name": "mcp__warden__" + name, "input": args}]}})
        res = rpc("tools/call", {"name": name, "arguments": args})["result"]
        text = "".join(c.get("text", "") for c in res.get("content", []))
        out({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": tid, "is_error": bool(res.get("isError")),
             "content": text}]}})
        return res, text

    call("mr_get_overview", {})
    _, diff = call("mr_get_diff", {})
    call("ci_get_results", {})
    # Pick the f-string SQL line from the diff so the anchor is real.
    target_line = None
    file_path = None
    new_line = 0
    for line in diff.splitlines():
        if line.startswith("+++ b/"):
            file_path = line[6:]
        elif line.startswith("@@"):
            new_line = int(line.split("+")[1].split(",")[0].split(" ")[0]) - 1
        elif file_path and file_path.endswith(".py"):
            if line.startswith("+") and not line.startswith("+++"):
                new_line += 1
                if "def link_import_rows" in line:
                    target_line = (file_path, new_line)
            elif line.startswith(" "):
                new_line += 1
    if not target_line:
        target_line = (file_path or "entries_crm/api/import_links.py", 14)

    # 1) a finding the guardrails must reject (unchanged line far outside the diff)
    call("review_add_finding", {
        "file": target_line[0], "line": 1, "side": "new", "severity": "minor",
        "category": "style", "title": "Header comment is out of date",
        "body": "Maybe the header comment could be better.", "confidence": 0.5})
    # 2) a valid finding on the whitelisted function
    res, text = call("review_add_finding", {
        "file": target_line[0], "line": target_line[1], "side": "new", "severity": "major",
        "category": "security", "rule_id": "FR-001",
        "title": "Whitelisted method has no permission check",
        "body": "Any logged-in user can call `link_import_rows` and relink rows of an import "
                "they cannot read. Check access to the import before touching rows.",
        "suggestion": None,
        "evidence": "@frappe.whitelist() with no frappe.has_permission / check_permission call",
        "confidence": 0.91})
    call("review_submit", {"summary": "Fake review: permission check missing on the new "
                                      "whitelisted method.", "verdict": "request_changes"})
    out({"type": "result", "subtype": "success", "is_error": False, "num_turns": turn["n"],
         "duration_ms": 1234, "session_id": "fake-session", "result": "done"})
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
