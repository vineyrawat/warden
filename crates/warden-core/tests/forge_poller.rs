// OWNER: forge
//! Poller change detection with an in-memory MrStore and a fake ForgeProvider (no db.rs, no network),
//! plus one end-to-end run against a wiremock GitLab.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use pretty_assertions::assert_eq;
use warden_core::forge::{ForgeDiscussion, ForgeProvider, GitlabProvider};
use warden_core::poller::{detect_changes, MrChangeReason, MrStore, Poller};
use warden_core::{
    DiffPosition, ForgeKind, MergeRequest, PostingMode, Project, Result, WardenError,
};

// ---------------------------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct FakeStore {
    rows: Mutex<Vec<MergeRequest>>,
    next_id: Mutex<i64>,
}

impl FakeStore {
    fn with(rows: Vec<MergeRequest>) -> Self {
        let max = rows.iter().map(|r| r.id).max().unwrap_or(0);
        FakeStore {
            rows: Mutex::new(rows),
            next_id: Mutex::new(max + 1),
        }
    }
    fn get(&self, iid: u64) -> Option<MergeRequest> {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.iid == iid)
            .cloned()
    }
    fn set_reviewed(&self, iid: u64, sha: &str) {
        let mut rows = self.rows.lock().unwrap();
        let r = rows.iter_mut().find(|r| r.iid == iid).unwrap();
        r.last_reviewed_sha = Some(sha.into());
    }
}

#[async_trait]
impl MrStore for FakeStore {
    async fn stored_mrs(&self, project_id: i64) -> Result<Vec<MergeRequest>> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.project_id == project_id)
            .cloned()
            .collect())
    }
    async fn save_mr(&self, mr: &MergeRequest) -> Result<i64> {
        let mut rows = self.rows.lock().unwrap();
        if let Some(r) = rows
            .iter_mut()
            .find(|r| r.project_id == mr.project_id && r.iid == mr.iid)
        {
            let id = r.id;
            *r = mr.clone();
            r.id = id;
            return Ok(id);
        }
        let mut next = self.next_id.lock().unwrap();
        let id = *next;
        *next += 1;
        let mut m = mr.clone();
        m.id = id;
        rows.push(m);
        Ok(id)
    }
}

#[derive(Default)]
struct FakeForge {
    open: Mutex<Vec<MergeRequest>>,
    /// get_mr answers (for closed MRs); missing -> NotFound.
    detail: Mutex<HashMap<u64, MergeRequest>>,
    fail_list: Mutex<bool>,
}

impl FakeForge {
    fn set_open(&self, mrs: Vec<MergeRequest>) {
        *self.open.lock().unwrap() = mrs;
    }
}

#[async_trait]
impl ForgeProvider for FakeForge {
    async fn whoami(&self) -> Result<String> {
        Ok("fake".into())
    }
    async fn list_projects(&self, _search: &str) -> Result<Vec<(String, String)>> {
        Ok(vec![])
    }
    async fn list_open_mrs(&self, _project: &Project) -> Result<Vec<MergeRequest>> {
        if *self.fail_list.lock().unwrap() {
            return Err(WardenError::Forge {
                status: 401,
                msg: "unauthorized".into(),
            });
        }
        Ok(self.open.lock().unwrap().clone())
    }
    async fn get_mr(&self, _project: &Project, iid: u64) -> Result<MergeRequest> {
        self.detail
            .lock()
            .unwrap()
            .get(&iid)
            .cloned()
            .ok_or_else(|| WardenError::NotFound(format!("mr {iid}")))
    }
    async fn list_discussions(&self, _p: &Project, _iid: u64) -> Result<Vec<ForgeDiscussion>> {
        Ok(vec![])
    }
    async fn post_line_note(
        &self,
        _p: &Project,
        _iid: u64,
        _b: &str,
        _pos: &DiffPosition,
    ) -> Result<String> {
        panic!("poller must never write to the forge")
    }
    async fn post_note(&self, _p: &Project, _iid: u64, _b: &str) -> Result<String> {
        panic!("poller must never write to the forge")
    }
    async fn resolve_discussion(&self, _p: &Project, _iid: u64, _d: &str) -> Result<()> {
        panic!("poller must never write to the forge")
    }
    async fn add_label(&self, _p: &Project, _iid: u64, _l: &str) -> Result<()> {
        panic!("poller must never write to the forge")
    }
}

fn project() -> Project {
    Project {
        id: 1,
        forge: ForgeKind::Gitlab,
        base_url: "https://gitlab.invalid".into(),
        remote_id: "g/r".into(),
        name: "r".into(),
        local_path: "/tmp/none".into(),
        target_branches: vec!["main".into(), "release/*".into()],
        mode: PostingMode::Draft,
        poll_interval_secs: 60,
        enabled: true,
    }
}

/// An MR as returned by the forge (id 0, no last_reviewed_sha).
fn remote(iid: u64, head: &str) -> MergeRequest {
    MergeRequest {
        id: 0,
        project_id: 1,
        iid,
        title: format!("MR {iid}"),
        target_branch: "main".into(),
        head_sha: head.into(),
        base_sha: "base".into(),
        start_sha: "start".into(),
        state: "opened".into(),
        ..Default::default()
    }
}

fn stored(id: i64, iid: u64, head: &str, reviewed: Option<&str>) -> MergeRequest {
    let mut m = remote(iid, head);
    m.id = id;
    m.last_reviewed_sha = reviewed.map(str::to_string);
    m
}

fn reasons(ch: &[warden_core::poller::MrChange]) -> Vec<(u64, MrChangeReason)> {
    ch.iter().map(|c| (c.mr.iid, c.reason.clone())).collect()
}

// ---------------------------------------------------------------------------------------------
// Pure detection
// ---------------------------------------------------------------------------------------------

#[test]
fn new_mr_is_new() {
    let ch = detect_changes(&[remote(1, "a")], &[]);
    assert_eq!(reasons(&ch), vec![(1, MrChangeReason::New)]);
}

#[test]
fn reviewed_head_is_unchanged() {
    let ch = detect_changes(&[remote(1, "a")], &[stored(10, 1, "a", Some("a"))]);
    assert!(ch.is_empty());
}

#[test]
fn moved_head_after_review_is_new_commits_since_reviewed() {
    let ch = detect_changes(&[remote(1, "c")], &[stored(10, 1, "b", Some("a"))]);
    assert_eq!(
        reasons(&ch),
        vec![(1, MrChangeReason::NewCommits { since: "a".into() })]
    );
    assert_eq!(ch[0].mr.id, 10, "carries the stored row id");
    assert_eq!(ch[0].mr.last_reviewed_sha.as_deref(), Some("a"));
}

#[test]
fn moved_head_never_reviewed_is_new() {
    let ch = detect_changes(&[remote(1, "b")], &[stored(10, 1, "a", None)]);
    assert_eq!(reasons(&ch), vec![(1, MrChangeReason::New)]);
}

#[test]
fn same_head_not_reviewed_is_not_repeated() {
    // Reported on an earlier poll (review queued/running/failed) -> no duplicate change.
    let ch = detect_changes(&[remote(1, "a")], &[stored(10, 1, "a", None)]);
    assert!(ch.is_empty());
}

#[test]
fn draft_becoming_ready_triggers_review() {
    let mut was = stored(10, 1, "a", None);
    was.draft = true;
    let ch = detect_changes(&[remote(1, "a")], &[was.clone()]);
    assert_eq!(reasons(&ch), vec![(1, MrChangeReason::New)]);
    assert!(ch[0].needs_review());

    // still draft -> nothing new
    let mut still = remote(1, "a");
    still.draft = true;
    assert!(detect_changes(&[still], &[was]).is_empty());
}

#[test]
fn removing_no_ai_review_label_triggers_review_since_last_reviewed() {
    let mut was = stored(10, 1, "b", Some("a"));
    was.labels = vec!["no-ai-review".into()];
    let ch = detect_changes(&[remote(1, "b")], &[was]);
    assert_eq!(
        reasons(&ch),
        vec![(1, MrChangeReason::NewCommits { since: "a".into() })]
    );
}

#[test]
fn reopened_mr_triggers_review() {
    let mut was = stored(10, 1, "a", None);
    was.state = "closed".into();
    let ch = detect_changes(&[remote(1, "a")], &[was]);
    assert_eq!(reasons(&ch), vec![(1, MrChangeReason::New)]);
}

#[test]
fn draft_and_label_are_flagged_for_skipping() {
    let mut d = remote(1, "a");
    d.draft = true;
    let mut l = remote(2, "b");
    l.labels = vec!["frontend".into(), "no-ai-review".into()];
    let ch = detect_changes(&[d, l, remote(3, "c")], &[]);
    assert_eq!(ch.len(), 3);
    assert_eq!(ch[0].skip_reason(), Some("draft merge request"));
    assert_eq!(ch[1].skip_reason(), Some("labelled no-ai-review"));
    assert_eq!(ch[2].skip_reason(), None);
    assert!(ch[2].needs_review());
}

#[test]
fn missing_open_mr_is_closed_but_old_closed_ones_are_not_repeated() {
    let mut old = stored(11, 2, "x", Some("x"));
    old.state = "merged".into();
    let ch = detect_changes(&[], &[stored(10, 1, "a", Some("a")), old]);
    assert_eq!(reasons(&ch), vec![(1, MrChangeReason::Closed)]);
}

#[test]
fn ordering_is_by_iid_then_closed() {
    let ch = detect_changes(
        &[remote(5, "e"), remote(2, "b")],
        &[stored(1, 9, "z", None)],
    );
    assert_eq!(
        reasons(&ch),
        vec![
            (2, MrChangeReason::New),
            (5, MrChangeReason::New),
            (9, MrChangeReason::Closed)
        ]
    );
}

#[test]
fn pairs_api() {
    let mut pairs = HashMap::new();
    pairs.insert(1u64, ("a".to_string(), Some("a".to_string())));
    pairs.insert(2u64, ("b".to_string(), Some("a".to_string())));
    let ch = Poller::changes_from_pairs(&[remote(1, "a"), remote(2, "c")], &pairs);
    assert_eq!(
        reasons(&ch),
        vec![(2, MrChangeReason::NewCommits { since: "a".into() })]
    );
}

// ---------------------------------------------------------------------------------------------
// Full poll over the store seam
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn poll_lifecycle_new_then_unchanged_then_commits_then_closed() {
    let store = FakeStore::with(vec![]);
    let forge = FakeForge::default();
    let poller = Poller::new();
    let p = project();

    // 1. first poll: MR 1 targets main (watched), MR 2 targets develop (ignored)
    let mut other = remote(2, "zz");
    other.target_branch = "develop".into();
    let mut rel = remote(3, "r1");
    rel.target_branch = "release/2.0".into();
    forge.set_open(vec![remote(1, "a"), other, rel]);
    let ch = poller.poll_with_store(&store, &forge, &p).await.unwrap();
    assert_eq!(
        reasons(&ch),
        vec![(1, MrChangeReason::New), (3, MrChangeReason::New)]
    );
    assert!(ch.iter().all(|c| c.mr.id > 0), "ids assigned by the store");
    assert!(
        store.get(2).is_none(),
        "unwatched target branch is not stored"
    );
    assert_eq!(store.get(1).unwrap().head_sha, "a");

    // 2. same state -> nothing
    let ch = poller.poll_with_store(&store, &forge, &p).await.unwrap();
    assert!(ch.is_empty());

    // 3. pipeline reviewed MR 1 at "a"; author pushes "b" and renames the MR
    store.set_reviewed(1, "a");
    let mut pushed = remote(1, "b");
    pushed.title = "MR 1 (v2)".into();
    let mut rel = remote(3, "r1");
    rel.target_branch = "release/2.0".into();
    forge.set_open(vec![pushed, rel]);
    let ch = poller.poll_with_store(&store, &forge, &p).await.unwrap();
    assert_eq!(
        reasons(&ch),
        vec![(1, MrChangeReason::NewCommits { since: "a".into() })]
    );
    let row = store.get(1).unwrap();
    assert_eq!(row.head_sha, "b");
    assert_eq!(row.title, "MR 1 (v2)", "metadata refreshed");
    assert_eq!(
        row.last_reviewed_sha.as_deref(),
        Some("a"),
        "last_reviewed_sha preserved"
    );
    let id1 = row.id;
    assert_eq!(ch[0].mr.id, id1);

    // 4. MR 3 merged (get_mr knows), MR 1 still open
    let mut merged = remote(3, "r1");
    merged.state = "merged".into();
    merged.target_branch = "release/2.0".into();
    forge.detail.lock().unwrap().insert(3, merged);
    forge.set_open(vec![remote(1, "b")]);
    let ch = poller.poll_with_store(&store, &forge, &p).await.unwrap();
    assert_eq!(reasons(&ch), vec![(3, MrChangeReason::Closed)]);
    assert_eq!(ch[0].mr.state, "merged");
    assert_eq!(store.get(3).unwrap().state, "merged");

    // 5. MR 1 disappears and get_mr fails -> recorded as closed anyway
    forge.set_open(vec![]);
    let ch = poller.poll_with_store(&store, &forge, &p).await.unwrap();
    assert_eq!(reasons(&ch), vec![(1, MrChangeReason::Closed)]);
    assert_eq!(ch[0].mr.id, id1);
    assert_eq!(store.get(1).unwrap().state, "closed");
    assert_eq!(
        store.get(1).unwrap().last_reviewed_sha.as_deref(),
        Some("a")
    );

    // 6. nothing open, nothing stored as opened -> no changes
    assert!(poller
        .poll_with_store(&store, &forge, &p)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn poll_reports_draft_once_then_new_when_ready() {
    let store = FakeStore::with(vec![]);
    let forge = FakeForge::default();
    let poller = Poller::new();
    let p = project();

    let mut d = remote(4, "a");
    d.draft = true;
    forge.set_open(vec![d.clone()]);
    let ch = poller.poll_with_store(&store, &forge, &p).await.unwrap();
    assert_eq!(ch.len(), 1);
    assert_eq!(ch[0].skip_reason(), Some("draft merge request"));

    // still draft: not reported again
    assert!(poller
        .poll_with_store(&store, &forge, &p)
        .await
        .unwrap()
        .is_empty());

    // marked ready
    forge.set_open(vec![remote(4, "a")]);
    let ch = poller.poll_with_store(&store, &forge, &p).await.unwrap();
    assert_eq!(reasons(&ch), vec![(4, MrChangeReason::New)]);
    assert!(ch[0].needs_review());
}

#[tokio::test]
async fn poll_keeps_existing_rows_for_other_projects() {
    let mut foreign = stored(50, 1, "x", None);
    foreign.project_id = 99;
    let store = FakeStore::with(vec![foreign]);
    let forge = FakeForge::default();
    let ch = Poller::new()
        .poll_with_store(&store, &forge, &project())
        .await
        .unwrap();
    assert!(
        ch.is_empty(),
        "another project's open MR is not reported as closed"
    );
}

#[tokio::test]
async fn poll_propagates_forge_errors_without_touching_store() {
    let store = FakeStore::with(vec![stored(10, 1, "a", None)]);
    let forge = FakeForge::default();
    *forge.fail_list.lock().unwrap() = true;
    let err = Poller::new()
        .poll_with_store(&store, &forge, &project())
        .await
        .unwrap_err();
    assert!(matches!(err, WardenError::Forge { status: 401, .. }));
    assert_eq!(
        store.get(1).unwrap().state,
        "opened",
        "a failed poll must not mark MRs closed"
    );
}

#[tokio::test]
async fn poll_rejects_invalid_branch_glob() {
    let store = FakeStore::with(vec![]);
    let forge = FakeForge::default();
    let mut p = project();
    p.target_branches = vec!["[unclosed".into()];
    assert!(matches!(
        Poller::new().poll_with_store(&store, &forge, &p).await,
        Err(WardenError::Config(_))
    ));
}

// ---------------------------------------------------------------------------------------------
// End to end with the real GitLab client against wiremock
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn poll_against_wiremock_gitlab() {
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let mr = |iid: u64, head: &str, target: &str| {
        json!({
            "iid": iid, "title": format!("MR {iid}"), "author": {"username": "a"},
            "source_branch": "f", "target_branch": target, "web_url": "u", "draft": false,
            "labels": [], "sha": head, "state": "opened", "updated_at": "2026-11-25T00:00:00Z",
            "diff_refs": {"base_sha": "b", "start_sha": "s", "head_sha": head}
        })
    };
    Mock::given(method("GET"))
        .and(path("/api/v4/projects/g%2Fr/merge_requests"))
        .and(query_param("state", "opened"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-next-page", "")
                .set_body_json(json!([mr(1, "h1", "main"), mr(2, "h2", "develop")])),
        )
        .mount(&server)
        .await;

    let mut p = project();
    p.base_url = server.uri();
    let gl = GitlabProvider::new(server.uri(), "tok");
    let store = FakeStore::with(vec![stored(10, 1, "h0", Some("h0"))]);
    let ch = Poller::new()
        .poll_with_store(&store, &gl, &p)
        .await
        .unwrap();
    assert_eq!(
        reasons(&ch),
        vec![(1, MrChangeReason::NewCommits { since: "h0".into() })]
    );
    assert_eq!(ch[0].mr.base_sha, "b");
    assert_eq!(ch[0].mr.start_sha, "s");
    assert_eq!(ch[0].mr.head_sha, "h1");
    assert_eq!(ch[0].mr.id, 10);
}
