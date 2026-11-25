// OWNER: forge
//! GitHub provider (octocrab) against wiremock only (hard rule 3: never a real server).

use pretty_assertions::assert_eq;
use serde_json::json;
use warden_core::forge::{is_position_error, provider_for, ForgeProvider, GithubProvider};
use warden_core::{DiffPosition, ForgeKind, PostingMode, Project, WardenError};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "ghp_test_token";
/// `GithubProvider::new(<non-github host>)` targets GitHub Enterprise: `<base>/api/v3`.
const API: &str = "/api/v3";

fn project(base: &str) -> Project {
    Project {
        id: 3,
        forge: ForgeKind::Github,
        base_url: base.to_string(),
        remote_id: "entries/warden".into(),
        name: "warden".into(),
        local_path: "/tmp/none".into(),
        target_branches: vec![],
        mode: PostingMode::Assist,
        poll_interval_secs: 60,
        enabled: true,
    }
}

fn provider(server: &MockServer) -> GithubProvider {
    GithubProvider::new(server.uri(), TOKEN)
}

fn pr_json(n: u64) -> serde_json::Value {
    json!({
        "id": 9000 + n, "number": n, "state": "open", "title": format!("PR {n}"), "body": "Body",
        "user": {"login": "octo", "id": 1},
        "head": {"ref": format!("feat/{n}"), "sha": format!("head{n}"), "label": "x"},
        "base": {"ref": "main", "sha": format!("base{n}")},
        "html_url": format!("https://github.com/entries/warden/pull/{n}"),
        "draft": false, "labels": [{"id": 1, "name": "backend"}],
        "updated_at": "2026-11-21T08:00:00Z", "merged_at": null
    })
}

#[tokio::test]
async fn whoami_uses_bearer_token_and_enterprise_base() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{API}/user")))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"login": "octocat", "id": 1})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let p = provider(&server);
    assert_eq!(p.api_base(), format!("{}{API}", server.uri()));
    assert_eq!(p.whoami().await.unwrap(), "octocat");
}

#[tokio::test]
async fn with_api_base_uses_root_verbatim() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"login": "root"})))
        .expect(1)
        .mount(&server)
        .await;
    let p = GithubProvider::with_api_base(server.uri(), TOKEN);
    assert_eq!(p.whoami().await.unwrap(), "root");
}

#[tokio::test]
async fn unauthorized_maps_to_forge_401() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{API}/user")))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "message": "Bad credentials", "documentation_url": "https://docs.github.com/rest"
        })))
        .mount(&server)
        .await;
    match provider(&server).whoami().await.unwrap_err() {
        WardenError::Forge { status: 401, msg } => {
            assert!(msg.contains("Bad credentials"), "{msg}");
            assert!(!msg.contains(TOKEN));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn list_projects_filters_by_search() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{API}/user/repos")))
        .and(query_param("per_page", "100"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 1, "name": "warden", "full_name": "entries/warden"},
            {"id": 2, "name": "crm", "full_name": "entries/crm"},
            {"id": 3, "name": "Warden-UI", "full_name": "me/Warden-UI"}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let got = provider(&server).list_projects("WARDEN").await.unwrap();
    assert_eq!(
        got,
        vec![
            ("entries/warden".to_string(), "warden".to_string()),
            ("me/Warden-UI".to_string(), "Warden-UI".to_string()),
        ]
    );
}

#[tokio::test]
async fn list_open_prs_paginates_and_maps() {
    let server = MockServer::start().await;
    let route = format!("{API}/repos/entries/warden/pulls");
    let page1: Vec<_> = (1..=100).map(pr_json).collect();
    Mock::given(method("GET"))
        .and(path(route.as_str()))
        .and(query_param("state", "open"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page1))
        .expect(1)
        .mount(&server)
        .await;
    let mut draft = pr_json(101);
    draft["draft"] = json!(true);
    draft["body"] = json!(null);
    Mock::given(method("GET"))
        .and(path(route.as_str()))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([draft])))
        .expect(1)
        .mount(&server)
        .await;

    let mrs = provider(&server)
        .list_open_mrs(&project(&server.uri()))
        .await
        .unwrap();
    assert_eq!(mrs.len(), 101);
    let first = &mrs[0];
    assert_eq!(first.iid, 1);
    assert_eq!(first.project_id, 3);
    assert_eq!(first.title, "PR 1");
    assert_eq!(first.author, "octo");
    assert_eq!(first.source_branch, "feat/1");
    assert_eq!(first.target_branch, "main");
    assert_eq!(first.head_sha, "head1");
    assert_eq!(first.base_sha, "base1");
    assert_eq!(first.start_sha, "base1");
    assert_eq!(first.state, "opened");
    assert_eq!(first.labels, vec!["backend".to_string()]);
    let last = mrs.last().unwrap();
    assert_eq!(last.iid, 101);
    assert!(last.draft);
    assert_eq!(last.description, "");
}

#[tokio::test]
async fn get_pr_with_shas_and_stats() {
    let server = MockServer::start().await;
    let mut pr = pr_json(12);
    pr["additions"] = json!(30);
    pr["deletions"] = json!(5);
    Mock::given(method("GET"))
        .and(path(format!("{API}/repos/entries/warden/pulls/12")))
        .respond_with(ResponseTemplate::new(200).set_body_json(pr))
        .expect(1)
        .mount(&server)
        .await;
    let mr = provider(&server)
        .get_mr(&project(&server.uri()), 12)
        .await
        .unwrap();
    assert_eq!(
        (mr.head_sha.as_str(), mr.base_sha.as_str()),
        ("head12", "base12")
    );
    assert_eq!((mr.additions, mr.deletions), (30, 5));
    assert_eq!(mr.web_url, "https://github.com/entries/warden/pull/12");
}

#[tokio::test]
async fn invalid_remote_id_is_config_error() {
    let server = MockServer::start().await;
    let mut p = project(&server.uri());
    p.remote_id = "just-a-name".into();
    assert!(matches!(
        provider(&server).get_mr(&p, 1).await,
        Err(WardenError::Config(_))
    ));
}

#[tokio::test]
async fn review_comments_become_threads() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{API}/repos/entries/warden/pulls/5/comments")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 100, "body": "**Major** missing permission check", "user": {"login": "warden-bot"},
             "path": "api.py", "line": 10, "original_line": 10, "commit_id": "h"},
            {"id": 102, "body": "on it", "user": {"login": "dev"}, "path": "api.py", "line": 10, "in_reply_to_id": 100},
            {"id": 101, "body": "outdated", "user": {"login": "dev"}, "path": "old.py", "line": null, "original_line": 3}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "{API}/repos/entries/warden/issues/5/comments"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": 555, "body": "## Warden summary", "user": {"login": "warden-bot"}}
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let ds = provider(&server)
        .list_discussions(&project(&server.uri()), 5)
        .await
        .unwrap();
    assert_eq!(
        ds.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
        vec!["100", "101", "issuecomment-555"]
    );
    assert_eq!(ds[0].notes.len(), 2);
    assert_eq!(ds[0].notes[0].author, "warden-bot");
    assert_eq!(ds[0].notes[0].path.as_deref(), Some("api.py"));
    assert_eq!(ds[0].notes[0].line, Some(10));
    assert_eq!(ds[0].notes[1].body, "on it");
    assert_eq!(
        ds[1].notes[0].line,
        Some(3),
        "falls back to original_line for outdated comments"
    );
    assert!(ds.iter().all(|d| !d.resolved));
    assert_eq!(ds[2].notes[0].path, None);
}

fn pos() -> DiffPosition {
    DiffPosition {
        base_sha: "b".into(),
        start_sha: "b".into(),
        head_sha: "headsha".into(),
        old_path: "app/api.py".into(),
        new_path: "app/api.py".into(),
        old_line: None,
        new_line: Some(42),
    }
}

#[tokio::test]
async fn post_line_note_uses_review_comment_api() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{API}/repos/entries/warden/pulls/5/comments")))
        .and(body_json(json!({
            "body": "**Blocker** f-string SQL",
            "commit_id": "headsha",
            "path": "app/api.py",
            "line": 42,
            "side": "RIGHT"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 777, "body": "x"})))
        .expect(1)
        .mount(&server)
        .await;
    let id = provider(&server)
        .post_line_note(
            &project(&server.uri()),
            5,
            "**Blocker** f-string SQL",
            &pos(),
        )
        .await
        .unwrap();
    assert_eq!(id, "777");
}

#[tokio::test]
async fn post_line_note_old_side_uses_left() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{API}/repos/entries/warden/pulls/5/comments")))
        .and(body_json(json!({
            "body": "removed guard", "commit_id": "headsha", "path": "gone.py", "line": 7, "side": "LEFT"
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 778})))
        .expect(1)
        .mount(&server)
        .await;
    let mut p = pos();
    p.old_path = "gone.py".into();
    p.new_line = None;
    p.old_line = Some(7);
    assert_eq!(
        provider(&server)
            .post_line_note(&project(&server.uri()), 5, "removed guard", &p)
            .await
            .unwrap(),
        "778"
    );
}

#[tokio::test]
async fn post_line_note_422_maps_to_position_error_400() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{API}/repos/entries/warden/pulls/5/comments")))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Validation Failed",
            "errors": [{"resource": "PullRequestReviewComment", "code": "custom", "field": "pull_request_review_thread.line", "message": "could not be resolved"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let err = provider(&server)
        .post_line_note(&project(&server.uri()), 5, "body", &pos())
        .await
        .unwrap_err();
    match &err {
        WardenError::Forge { status: 400, msg } => {
            assert!(msg.contains("Validation Failed"), "{msg}");
            assert!(msg.contains("pull_request_review_thread.line"), "{msg}");
        }
        other => panic!("{other:?}"),
    }
    assert!(is_position_error(&err));
}

#[tokio::test]
async fn post_note_uses_issue_comment() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!(
            "{API}/repos/entries/warden/issues/5/comments"
        )))
        .and(body_json(json!({"body": "## Warden summary"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": 4242})))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        provider(&server)
            .post_note(&project(&server.uri()), 5, "## Warden summary")
            .await
            .unwrap(),
        "4242"
    );
}

#[tokio::test]
async fn add_label_posts_labels() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("{API}/repos/entries/warden/issues/5/labels")))
        .and(body_json(json!({"labels": ["ai-reviewed"]})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([{"id": 1, "name": "ai-reviewed"}])),
        )
        .expect(1)
        .mount(&server)
        .await;
    provider(&server)
        .add_label(&project(&server.uri()), 5, "ai-reviewed")
        .await
        .unwrap();
}

#[tokio::test]
async fn resolve_discussion_is_a_noop() {
    let server = MockServer::start().await;
    // Any request would be unexpected.
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    provider(&server)
        .resolve_discussion(&project(&server.uri()), 5, "100")
        .await
        .unwrap();
}

#[tokio::test]
async fn post_is_not_retried_on_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!(
            "{API}/repos/entries/warden/issues/5/comments"
        )))
        .respond_with(ResponseTemplate::new(502).set_body_string("bad gateway"))
        .expect(1)
        .mount(&server)
        .await;
    let err = provider(&server)
        .post_note(&project(&server.uri()), 5, "x")
        .await
        .unwrap_err();
    assert!(
        matches!(err, WardenError::Forge { status: 502, .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn provider_for_routes_github_projects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{API}/user")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"login": "factory"})))
        .expect(1)
        .mount(&server)
        .await;
    let p = provider_for(&project(&server.uri()), TOKEN.into());
    assert_eq!(p.whoami().await.unwrap(), "factory");
}

#[tokio::test]
async fn review_status_submits_review_event() {
    use warden_core::MrReviewStatus;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/repos/acme/app/pulls/12/reviews"))
        .and(body_json(
            json!({"event": "REQUEST_CHANGES", "body": "fix the blocker"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 1})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v3/repos/acme/app/issues/12/labels/warden%3A%3Aapproved",
        ))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"message": "Label does not exist"})),
        )
        .mount(&server)
        .await;
    let p = provider(&server);
    let mut proj = project(&server.uri());
    proj.remote_id = "acme/app".into();
    p.set_review_status(
        &proj,
        12,
        MrReviewStatus::ChangesRequested,
        "fix the blocker",
    )
    .await
    .unwrap();
    // A label that is not on the PR is not an error.
    p.remove_label(&proj, 12, "warden::approved").await.unwrap();
}
