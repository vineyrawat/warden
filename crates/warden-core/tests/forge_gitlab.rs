// OWNER: forge
//! GitLab provider against wiremock only (hard rule 3: never a real server).

use std::collections::HashMap;

use pretty_assertions::assert_eq;
use serde_json::json;
use warden_core::forge::{
    is_auth_error, is_position_error, provider_for, ForgeProvider, GitlabProvider,
};
use warden_core::{DiffPosition, ForgeKind, PostingMode, Project, WardenError};
use wiremock::matchers::{
    body_string_contains, header, method, path, query_param, query_param_is_missing,
};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

const TOKEN: &str = "glpat-test-token";
const PROJ: &str = "/api/v4/projects/entries%2Fentries_crm";

fn project(base: &str) -> Project {
    Project {
        id: 7,
        forge: ForgeKind::Gitlab,
        base_url: base.to_string(),
        remote_id: "entries/entries_crm".into(),
        name: "entries_crm".into(),
        local_path: "/tmp/none".into(),
        target_branches: vec!["main".into()],
        mode: PostingMode::Draft,
        poll_interval_secs: 60,
        enabled: true,
    }
}

fn provider(server: &MockServer) -> GitlabProvider {
    GitlabProvider::new(server.uri(), TOKEN)
}

fn mr_json(iid: u64, with_refs: bool) -> serde_json::Value {
    let mut v = json!({
        "id": 1000 + iid, "iid": iid, "project_id": 55,
        "title": format!("MR {iid}"), "description": "desc",
        "author": {"username": "alice"},
        "source_branch": format!("feat/{iid}"), "target_branch": "main",
        "web_url": format!("https://gl/x/-/merge_requests/{iid}"),
        "draft": false, "work_in_progress": false,
        "labels": ["backend"], "sha": format!("head{iid}"),
        "state": "opened", "updated_at": "2026-11-20T10:00:00Z",
        "changes_count": "3"
    });
    if with_refs {
        v["diff_refs"] = json!({
            "base_sha": format!("base{iid}"), "start_sha": format!("start{iid}"), "head_sha": format!("head{iid}")
        });
    }
    v
}

/// Decodes an application/x-www-form-urlencoded body.
fn form(body: &[u8]) -> HashMap<String, String> {
    fn dec(s: &str) -> String {
        let s = s.replace('+', " ");
        let bytes = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let h = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
                out.push(u8::from_str_radix(h, 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }
    String::from_utf8_lossy(body)
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (dec(k), dec(v))
        })
        .collect()
}

/// Matches when every (key, value) pair is present in the form body.
struct FormHas(Vec<(&'static str, &'static str)>);
impl Match for FormHas {
    fn matches(&self, req: &Request) -> bool {
        let f = form(&req.body);
        self.0
            .iter()
            .all(|(k, v)| f.get(*k).map(String::as_str) == Some(*v))
    }
}

/// Matches when the form body does NOT contain `key`.
struct FormLacks(&'static str);
impl Match for FormLacks {
    fn matches(&self, req: &Request) -> bool {
        !form(&req.body).contains_key(self.0)
    }
}

fn pos() -> DiffPosition {
    DiffPosition {
        base_sha: "b1".into(),
        start_sha: "s1".into(),
        head_sha: "h1".into(),
        old_path: "app/api.py".into(),
        new_path: "app/api.py".into(),
        old_line: None,
        new_line: Some(42),
    }
}

#[tokio::test]
async fn whoami_sends_private_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": 1, "username": "warden-bot"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(provider(&server).whoami().await.unwrap(), "warden-bot");
}

#[tokio::test]
async fn unauthorized_maps_to_forge_401() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({"message": "401 Unauthorized"})),
        )
        .mount(&server)
        .await;
    let err = provider(&server).whoami().await.unwrap_err();
    match &err {
        WardenError::Forge { status, msg } => {
            assert_eq!(*status, 401);
            assert!(msg.contains("401 Unauthorized"), "{msg}");
            assert!(!msg.contains(TOKEN));
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(is_auth_error(&err));
    assert!(!is_position_error(&err));
}

#[tokio::test]
async fn base_url_with_api_suffix_and_trailing_slash() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"username": "u"})))
        .expect(1)
        .mount(&server)
        .await;
    let p = GitlabProvider::new(format!("{}/api/v4/", server.uri()), TOKEN);
    assert_eq!(p.whoami().await.unwrap(), "u");
}

#[tokio::test]
async fn list_projects_with_search() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects"))
        .and(query_param("search", "crm"))
        .and(query_param("membership", "true"))
        .and(query_param("per_page", "100"))
        .respond_with(ResponseTemplate::new(200).insert_header("x-next-page", "").set_body_json(json!([
            {"id": 1, "path_with_namespace": "entries/entries_crm", "name_with_namespace": "Entries / entries_crm", "name": "entries_crm"},
            {"id": 2, "path_with_namespace": "entries/crm-ui", "name": "crm-ui"}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let got = provider(&server).list_projects("crm").await.unwrap();
    assert_eq!(
        got,
        vec![
            (
                "entries/entries_crm".to_string(),
                "Entries / entries_crm".to_string()
            ),
            ("entries/crm-ui".to_string(), "crm-ui".to_string()),
        ]
    );
}

#[tokio::test]
async fn list_projects_empty_search_omits_param() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects"))
        .and(query_param_is_missing("search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    assert!(provider(&server)
        .list_projects("  ")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn list_open_mrs_paginates_and_fills_diff_refs() {
    let server = MockServer::start().await;
    let list = format!("{PROJ}/merge_requests");
    // page 1 -> next page 2 ; page 2 -> last page
    Mock::given(method("GET"))
        .and(path(list.as_str()))
        .and(query_param("state", "opened"))
        .and(query_param("page", "1"))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-next-page", "2")
                .insert_header("x-total-pages", "2")
                .set_body_json(json!([mr_json(1, false), mr_json(2, true)])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(list.as_str()))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-next-page", "")
                .insert_header("x-total-pages", "2")
                .set_body_json(json!([mr_json(3, false)])),
        )
        .expect(1)
        .mount(&server)
        .await;
    // detail calls only for MRs without diff_refs (1 and 3)
    for iid in [1u64, 3] {
        Mock::given(method("GET"))
            .and(path(format!("{PROJ}/merge_requests/{iid}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(mr_json(iid, true)))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(format!("{PROJ}/merge_requests/2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(mr_json(2, true)))
        .expect(0)
        .mount(&server)
        .await;

    let base = server.uri();
    let mrs = provider(&server)
        .list_open_mrs(&project(&base))
        .await
        .unwrap();
    assert_eq!(mrs.iter().map(|m| m.iid).collect::<Vec<_>>(), vec![1, 2, 3]);
    for m in &mrs {
        assert_eq!(m.project_id, 7, "uses Warden's project id, not GitLab's");
        assert_eq!(m.base_sha, format!("base{}", m.iid));
        assert_eq!(m.start_sha, format!("start{}", m.iid));
        assert_eq!(m.head_sha, format!("head{}", m.iid));
        assert_eq!(m.author, "alice");
        assert_eq!(m.state, "opened");
        assert_eq!(m.labels, vec!["backend".to_string()]);
        assert_eq!(m.last_reviewed_sha, None);
    }
}

#[tokio::test]
async fn pagination_falls_back_to_total_pages() {
    let server = MockServer::start().await;
    let list = format!("{PROJ}/merge_requests");
    Mock::given(method("GET"))
        .and(path(list.as_str()))
        .and(query_param("page", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-total-pages", "2")
                .set_body_json(json!([mr_json(1, true)])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(list.as_str()))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-total-pages", "2")
                .set_body_json(json!([mr_json(2, true)])),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mrs = provider(&server)
        .list_open_mrs(&project(&server.uri()))
        .await
        .unwrap();
    assert_eq!(mrs.len(), 2);
}

#[tokio::test]
async fn numeric_project_id_is_not_encoded() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/1234/merge_requests/5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(mr_json(5, true)))
        .expect(1)
        .mount(&server)
        .await;
    let mut p = project(&server.uri());
    p.remote_id = "1234".into();
    assert_eq!(provider(&server).get_mr(&p, 5).await.unwrap().iid, 5);
}

#[tokio::test]
async fn get_mr_maps_fields_and_draft_flags() {
    let server = MockServer::start().await;
    let mut v = mr_json(9, true);
    v["work_in_progress"] = json!(true);
    v["draft"] = json!(null);
    v["labels"] = json!(["no-ai-review", {"name": "frontend"}]);
    v["description"] = json!(null);
    v["additions"] = json!(12);
    v["deletions"] = json!(4);
    Mock::given(method("GET"))
        .and(path(format!("{PROJ}/merge_requests/9")))
        .respond_with(ResponseTemplate::new(200).set_body_json(v))
        .mount(&server)
        .await;
    let mr = provider(&server)
        .get_mr(&project(&server.uri()), 9)
        .await
        .unwrap();
    assert!(mr.draft);
    assert_eq!(
        mr.labels,
        vec!["no-ai-review".to_string(), "frontend".to_string()]
    );
    assert_eq!(mr.description, "");
    assert_eq!((mr.additions, mr.deletions), (12, 4));
    assert_eq!(mr.source_branch, "feat/9");
    assert_eq!(mr.target_branch, "main");
    assert_eq!(mr.web_url, "https://gl/x/-/merge_requests/9");
    assert_eq!(mr.updated_at, "2026-11-20T10:00:00Z");
    assert_eq!(
        warden_core::forge::skip_reason(&mr),
        Some("draft merge request")
    );
}

#[tokio::test]
async fn get_mr_404() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{PROJ}/merge_requests/404")))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "404 Not found"})))
        .mount(&server)
        .await;
    let err = provider(&server)
        .get_mr(&project(&server.uri()), 404)
        .await
        .unwrap_err();
    assert!(
        matches!(err, WardenError::Forge { status: 404, .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn server_error_with_html_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(ResponseTemplate::new(502).set_body_string("<html>Bad Gateway</html>"))
        .mount(&server)
        .await;
    let err = provider(&server).whoami().await.unwrap_err();
    match err {
        WardenError::Forge { status: 502, msg } => assert!(msg.contains("Bad Gateway")),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn unexpected_json_is_an_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;
    assert!(matches!(
        provider(&server).whoami().await,
        Err(WardenError::Http(_))
    ));
}

#[tokio::test]
async fn unreachable_server_is_an_http_error() {
    // Port 9 (discard) on localhost: connection refused, no real forge involved.
    let p = GitlabProvider::new("http://127.0.0.1:9", TOKEN);
    assert!(matches!(p.whoami().await, Err(WardenError::Http(_))));
}

#[tokio::test]
async fn list_discussions_paginates_and_maps_notes() {
    let server = MockServer::start().await;
    let disc = format!("{PROJ}/merge_requests/3/discussions");
    Mock::given(method("GET"))
        .and(path(disc.as_str()))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).insert_header("x-next-page", "2").set_body_json(json!([
            {"id": "d1", "individual_note": false, "notes": [
                {"id": 11, "body": "**Major** SQL injection", "author": {"username": "warden-bot"},
                 "system": false, "resolvable": true, "resolved": true,
                 "position": {"new_path": "app/api.py", "old_path": "app/api.py", "new_line": 42, "old_line": null}},
                {"id": 12, "body": "fixed", "author": {"username": "alice"}, "system": false, "resolvable": true, "resolved": true}
            ]},
            {"id": "sys", "individual_note": true, "notes": [
                {"id": 13, "body": "added 2 commits", "author": {"username": "alice"}, "system": true}
            ]}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(disc.as_str()))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).insert_header("x-next-page", "").set_body_json(json!([
            {"id": "d2", "notes": [
                {"id": 21, "body": "old side", "author": {"username": "bob"}, "resolvable": true, "resolved": false,
                 "position": {"new_path": null, "old_path": "gone.py", "new_line": null, "old_line": 7}}
            ]},
            {"id": "d3", "notes": [
                {"id": 31, "body": "general comment", "author": {"username": "bob"}, "resolvable": false}
            ]}
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let ds = provider(&server)
        .list_discussions(&project(&server.uri()), 3)
        .await
        .unwrap();
    assert_eq!(
        ds.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        vec!["d1", "d2", "d3"]
    );
    assert!(ds[0].resolved);
    assert_eq!(ds[0].notes.len(), 2);
    assert_eq!(ds[0].notes[0].id, "11");
    assert_eq!(ds[0].notes[0].author, "warden-bot");
    assert_eq!(ds[0].notes[0].path.as_deref(), Some("app/api.py"));
    assert_eq!(ds[0].notes[0].line, Some(42));
    assert_eq!(ds[0].notes[1].path, None);
    assert!(!ds[1].resolved);
    assert_eq!(ds[1].notes[0].path.as_deref(), Some("gone.py"));
    assert_eq!(ds[1].notes[0].line, Some(7));
    assert!(
        !ds[2].resolved,
        "non-resolvable discussion is never 'resolved'"
    );
}

#[tokio::test]
async fn post_line_note_sends_text_position_and_returns_discussion_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{PROJ}/merge_requests/3/discussions")))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(FormHas(vec![
            ("body", "**Blocker** f-string SQL & more"),
            ("position[position_type]", "text"),
            ("position[base_sha]", "b1"),
            ("position[start_sha]", "s1"),
            ("position[head_sha]", "h1"),
            ("position[old_path]", "app/api.py"),
            ("position[new_path]", "app/api.py"),
            ("position[new_line]", "42"),
        ]))
        .and(FormLacks("position[old_line]"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "6a9c1750b37d513a43987b574953fceb50b03ce7", "individual_note": false,
            "notes": [{"id": 1126, "body": "x"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let id = provider(&server)
        .post_line_note(
            &project(&server.uri()),
            3,
            "**Blocker** f-string SQL & more",
            &pos(),
        )
        .await
        .unwrap();
    assert_eq!(id, "6a9c1750b37d513a43987b574953fceb50b03ce7");
}

#[tokio::test]
async fn post_line_note_old_and_new_lines() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{PROJ}/merge_requests/3/discussions")))
        .and(FormHas(vec![
            ("position[old_line]", "40"),
            ("position[new_line]", "42"),
        ]))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "abc"})))
        .expect(1)
        .mount(&server)
        .await;
    let mut p = pos();
    p.old_line = Some(40);
    let id = provider(&server)
        .post_line_note(&project(&server.uri()), 3, "context line", &p)
        .await
        .unwrap();
    assert_eq!(id, "abc");
}

#[tokio::test]
async fn post_line_note_400_is_position_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{PROJ}/merge_requests/3/discussions")))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "message": "400 Bad request - Note {:line_code=>[\"can't be blank\", \"must be a valid line code\"]}"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let err = provider(&server)
        .post_line_note(&project(&server.uri()), 3, "body text here", &pos())
        .await
        .unwrap_err();
    match &err {
        WardenError::Forge { status: 400, msg } => assert!(msg.contains("line_code"), "{msg}"),
        other => panic!("unexpected {other:?}"),
    }
    assert!(is_position_error(&err));
}

#[tokio::test]
async fn post_note_returns_note_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{PROJ}/merge_requests/3/notes")))
        .and(FormHas(vec![("body", "## Warden summary\n| a | b |")]))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 302, "body": "x"})))
        .expect(1)
        .mount(&server)
        .await;
    let id = provider(&server)
        .post_note(&project(&server.uri()), 3, "## Warden summary\n| a | b |")
        .await
        .unwrap();
    assert_eq!(id, "302");
}

#[tokio::test]
async fn post_file_note_creates_resolvable_discussion() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{PROJ}/merge_requests/3/discussions")))
        .and(FormHas(vec![("body", "**a.py:9**\n\nnote")]))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"id": "6a9c1750", "notes": []})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let (id, resolvable) = provider(&server)
        .post_file_note(&project(&server.uri()), 3, "**a.py:9**\n\nnote")
        .await
        .unwrap();
    assert_eq!(id, "6a9c1750");
    assert!(resolvable);
}

#[tokio::test]
async fn resolve_discussion_puts_resolved_true() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!(
            "{PROJ}/merge_requests/3/discussions/6a9c1750"
        )))
        .and(query_param("resolved", "true"))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "6a9c1750"})))
        .expect(1)
        .mount(&server)
        .await;
    provider(&server)
        .resolve_discussion(&project(&server.uri()), 3, "6a9c1750")
        .await
        .unwrap();
}

#[tokio::test]
async fn resolve_discussion_403() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!("{PROJ}/merge_requests/3/discussions/d1")))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"message": "403 Forbidden"})))
        .mount(&server)
        .await;
    let err = provider(&server)
        .resolve_discussion(&project(&server.uri()), 3, "d1")
        .await
        .unwrap_err();
    assert!(is_auth_error(&err));
}

#[tokio::test]
async fn add_label_puts_add_labels() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path(format!("{PROJ}/merge_requests/3")))
        .and(body_string_contains("add_labels=ai-reviewed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(mr_json(3, true)))
        .expect(1)
        .mount(&server)
        .await;
    provider(&server)
        .add_label(&project(&server.uri()), 3, "ai-reviewed")
        .await
        .unwrap();
}

#[tokio::test]
async fn provider_for_routes_gitlab_projects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .and(header("PRIVATE-TOKEN", "tok2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"username": "via-factory"})))
        .expect(1)
        .mount(&server)
        .await;
    let p = provider_for(&project(&server.uri()), "tok2".into());
    assert_eq!(p.whoami().await.unwrap(), "via-factory");
}

#[tokio::test]
async fn project_details_and_protected_branches() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/projects"))
        .and(query_param("membership", "true"))
        .and(query_param("archived", "false"))
        .and(header("PRIVATE-TOKEN", TOKEN))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
            "path_with_namespace": "entries/entries_crm",
            "name": "entries_crm",
            "default_branch": "develop",
            "last_activity_at": "2026-11-25T08:00:00Z",
            "web_url": "https://gl/entries/entries_crm",
            "http_url_to_repo": "https://gl/entries/entries_crm.git"
        }])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{PROJ}/protected_branches")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{"name": "develop"}, {"name": "release/*"}])),
        )
        .mount(&server)
        .await;
    let p = provider(&server);
    let list = p.list_project_details("").await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].remote_id, "entries/entries_crm");
    assert_eq!(list[0].default_branch.as_deref(), Some("develop"));
    assert_eq!(
        list[0].clone_url.as_deref(),
        Some("https://gl/entries/entries_crm.git")
    );
    assert_eq!(
        p.protected_branches("entries/entries_crm").await.unwrap(),
        vec!["develop", "release/*"]
    );
    assert_eq!(
        p.git_auth_header().unwrap(),
        warden_core::discovery::basic_auth_header("oauth2", TOKEN)
    );
}

#[tokio::test]
async fn review_status_approves_once_and_unapproves() {
    use warden_core::MrReviewStatus;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v4/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"username": "warden-bot"})))
        .mount(&server)
        .await;
    // Not yet approved by the bot -> approve is called.
    Mock::given(method("GET"))
        .and(path(format!("{PROJ}/merge_requests/7/approvals")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"approved_by": []})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{PROJ}/merge_requests/7/approve")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    // Already approved by the bot -> unapprove on "changes requested".
    Mock::given(method("GET"))
        .and(path(format!("{PROJ}/merge_requests/8/approvals")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"approved_by": [{"user": {"username": "warden-bot"}}]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("{PROJ}/merge_requests/8/unapprove")))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("{PROJ}/merge_requests/8")))
        .and(body_string_contains("remove_labels=warden%3A%3Aapproved"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let p = provider(&server);
    let proj = project(&server.uri());
    p.set_review_status(&proj, 7, MrReviewStatus::Approved, "ok")
        .await
        .unwrap();
    p.set_review_status(&proj, 8, MrReviewStatus::ChangesRequested, "fix")
        .await
        .unwrap();
    p.remove_label(&proj, 8, "warden::approved").await.unwrap();
}
