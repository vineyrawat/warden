// OWNER: poster
//! Db tests against `Db::open_memory()` (and one file-backed open) for every public method.

use pretty_assertions::assert_eq;
use serde_json::json;
use warden_core::config::AppSettings;
use warden_core::db::{normalize_ts, Db};
use warden_core::{
    CiJob, CiStatus, DiffPosition, FindingStatus, ForgeKind, MergeRequest, NewFinding, PostingMode,
    Project, RunStatus, Severity, WardenError,
};

fn project(remote: &str) -> Project {
    Project {
        id: 0,
        forge: ForgeKind::Gitlab,
        base_url: "https://gitlab.example".into(),
        remote_id: remote.into(),
        name: format!("name-{remote}"),
        local_path: "/tmp/repo".into(),
        target_branches: vec!["main".into(), "release/*".into()],
        mode: PostingMode::Assist,
        poll_interval_secs: 90,
        enabled: true,
    }
}

fn mr(project_id: i64, iid: u64) -> MergeRequest {
    MergeRequest {
        id: 0,
        project_id,
        iid,
        title: format!("MR {iid}"),
        description: "desc".into(),
        author: "alice".into(),
        source_branch: "feat".into(),
        target_branch: "main".into(),
        web_url: format!("https://gitlab.example/g/p/-/merge_requests/{iid}"),
        draft: false,
        labels: vec!["backend".into()],
        head_sha: "h1".into(),
        base_sha: "b1".into(),
        start_sha: "s1".into(),
        last_reviewed_sha: None,
        additions: 10,
        deletions: 2,
        state: "opened".into(),
        updated_at: format!("2026-11-25T10:00:0{iid}Z"),
    }
}

fn new_finding(title: &str, severity: Severity) -> NewFinding {
    NewFinding {
        file: "app/api.py".into(),
        line: 42,
        side: "new".into(),
        severity,
        category: "security".into(),
        rule_id: Some("FR-002".into()),
        title: title.into(),
        body: "The query is built with an f-string from request input.".into(),
        suggestion: Some("frappe.db.sql(q, values)".into()),
        evidence: Some("line 42".into()),
        confidence: 0.98,
        source: "claude".into(),
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

async fn seeded() -> (Db, i64, i64, i64) {
    let db = Db::open_memory().await.unwrap();
    let pid = db.upsert_project(&project("g/p")).await.unwrap();
    let mid = db.upsert_mr(&mr(pid, 1)).await.unwrap();
    let run = db.create_run(mid, "b1", "h1", None, "opus").await.unwrap();
    (db, pid, mid, run.id)
}

#[tokio::test]
async fn open_creates_file_and_parent_dirs_in_wal_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/deeper/warden.db");
    let db = Db::open(&path).await.unwrap();
    assert!(path.exists());
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");
    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(fk, 1);
    db.save_settings(&AppSettings {
        model: "sonnet".into(),
        ..Default::default()
    })
    .await
    .unwrap();
    db.pool.close().await;

    // Re-open: migrations are idempotent and data persists.
    let db2 = Db::open(&path).await.unwrap();
    assert_eq!(db2.get_settings().await.unwrap().model, "sonnet");
}

#[tokio::test]
async fn settings_default_and_round_trip() {
    let db = Db::open_memory().await.unwrap();
    assert_eq!(db.get_settings().await.unwrap(), AppSettings::default());
    let s = AppSettings {
        claude_cli_path: Some("/opt/claude".into()),
        model: "sonnet".into(),
        max_turns: 12,
        review_timeout_secs: 99,
        max_reviews_per_hour: 2,
        posting_paused: true,
        mcp_port: 9000,
        worktree_root: "/w".into(),
        data_dir: "/d".into(),
    };
    db.save_settings(&s).await.unwrap();
    assert_eq!(db.get_settings().await.unwrap(), s);
    let s2 = AppSettings {
        posting_paused: false,
        ..s.clone()
    };
    db.save_settings(&s2).await.unwrap();
    assert_eq!(db.get_settings().await.unwrap(), s2);

    // Corrupt row falls back to defaults instead of failing startup.
    sqlx::query("UPDATE settings SET value = 'not json' WHERE key = 'app'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.get_settings().await.unwrap(), AppSettings::default());
}

#[tokio::test]
async fn project_overrides_round_trip() {
    let (db, pid, _, _) = seeded().await;
    assert_eq!(db.get_project_overrides(pid).await.unwrap(), json!({}));
    let o = json!({"review": {"max_notes": 3}, "posting": {"summary_note": false}});
    db.save_project_overrides(pid, &o).await.unwrap();
    assert_eq!(db.get_project_overrides(pid).await.unwrap(), o);
    let o2 = json!({"review": {"nits": true}});
    db.save_project_overrides(pid, &o2).await.unwrap();
    assert_eq!(db.get_project_overrides(pid).await.unwrap(), o2);
    // Unknown project violates the foreign key.
    assert!(db.save_project_overrides(999, &o).await.is_err());
}

#[tokio::test]
async fn projects_crud() {
    let db = Db::open_memory().await.unwrap();
    assert!(db.list_projects().await.unwrap().is_empty());

    let id = db.upsert_project(&project("g/b")).await.unwrap();
    let id2 = db.upsert_project(&project("g/a")).await.unwrap();
    assert_ne!(id, id2);

    let mut p = db.get_project(id).await.unwrap();
    assert_eq!(p.id, id);
    assert_eq!(p.remote_id, "g/b");
    assert_eq!(p.target_branches, vec!["main", "release/*"]);
    assert_eq!(p.mode, PostingMode::Assist);
    assert_eq!(p.poll_interval_secs, 90);
    assert!(p.enabled);

    // Update by id.
    p.name = "renamed".into();
    p.enabled = false;
    p.mode = PostingMode::Auto;
    p.forge = ForgeKind::Github;
    assert_eq!(db.upsert_project(&p).await.unwrap(), id);
    assert_eq!(db.get_project(id).await.unwrap(), p);

    // Insert of an existing (forge, base_url, remote_id) updates that row.
    let mut dup = project("g/a");
    dup.name = "again".into();
    assert_eq!(db.upsert_project(&dup).await.unwrap(), id2);
    assert_eq!(db.get_project(id2).await.unwrap().name, "again");

    // Ordered by name.
    let names: Vec<String> = db
        .list_projects()
        .await
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, vec!["again", "renamed"]);

    // Update of a missing id.
    let mut ghost = project("g/z");
    ghost.id = 4242;
    assert!(matches!(
        db.upsert_project(&ghost).await,
        Err(WardenError::NotFound(_))
    ));

    db.delete_project(id).await.unwrap();
    assert!(matches!(
        db.get_project(id).await,
        Err(WardenError::NotFound(_))
    ));
    db.delete_project(id).await.unwrap(); // idempotent
}

#[tokio::test]
async fn delete_project_cascades() {
    let (db, pid, mid, run_id) = seeded().await;
    db.insert_finding(
        run_id,
        &new_finding("x", Severity::Major),
        FindingStatus::Draft,
        None,
        "fp",
        None,
    )
    .await
    .unwrap();
    db.insert_ci_job(&CiJob {
        run_id,
        name: "lint".into(),
        ..Default::default()
    })
    .await
    .unwrap();
    db.save_project_overrides(pid, &json!({"a": 1}))
        .await
        .unwrap();
    db.audit(Some(run_id), "post", json!({"x": 1}))
        .await
        .unwrap();

    db.delete_project(pid).await.unwrap();
    assert!(db.get_mr(mid).await.is_err());
    assert!(db.get_run(run_id).await.is_err());
    assert!(db.list_findings(run_id).await.unwrap().is_empty());
    assert!(db.list_ci_jobs(run_id).await.unwrap().is_empty());
    assert_eq!(db.get_project_overrides(pid).await.unwrap(), json!({}));
    // Audit rows survive with run_id set to NULL.
    let audit = db.list_audit(None, 10).await.unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].run_id, None);
}

#[tokio::test]
async fn mrs_upsert_keeps_last_reviewed_sha() {
    let db = Db::open_memory().await.unwrap();
    let pid = db.upsert_project(&project("g/p")).await.unwrap();
    let pid2 = db.upsert_project(&project("g/q")).await.unwrap();

    let id = db.upsert_mr(&mr(pid, 1)).await.unwrap();
    let got = db.get_mr(id).await.unwrap();
    let mut expected = mr(pid, 1);
    expected.id = id;
    assert_eq!(got, expected);

    db.set_last_reviewed(id, "h1").await.unwrap();
    assert_eq!(
        db.get_mr(id).await.unwrap().last_reviewed_sha.as_deref(),
        Some("h1")
    );

    // Poller upsert with new head: same id, fields updated, last_reviewed_sha kept.
    let mut again = mr(pid, 1);
    again.head_sha = "h2".into();
    again.title = "new title".into();
    again.labels = vec![];
    again.draft = true;
    assert_eq!(db.upsert_mr(&again).await.unwrap(), id);
    let got = db.get_mr(id).await.unwrap();
    assert_eq!(got.head_sha, "h2");
    assert_eq!(got.title, "new title");
    assert!(got.labels.is_empty());
    assert!(got.draft);
    assert_eq!(got.last_reviewed_sha.as_deref(), Some("h1"));

    // Same iid in a different project is a different MR.
    let other = db.upsert_mr(&mr(pid2, 1)).await.unwrap();
    assert_ne!(other, id);
    let id3 = db.upsert_mr(&mr(pid, 3)).await.unwrap();

    assert_eq!(db.list_mrs(None).await.unwrap().len(), 3);
    let in_p: Vec<i64> = db
        .list_mrs(Some(pid))
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(in_p, vec![id3, id]); // updated_at desc
    assert!(db.list_mrs(Some(999)).await.unwrap().is_empty());

    assert!(matches!(
        db.get_mr(999).await,
        Err(WardenError::NotFound(_))
    ));
    assert!(matches!(
        db.set_last_reviewed(999, "x").await,
        Err(WardenError::NotFound(_))
    ));
    // MR for an unknown project violates the foreign key.
    assert!(db.upsert_mr(&mr(999, 1)).await.is_err());
}

#[tokio::test]
async fn runs_lifecycle() {
    let (db, _, mid, run_id) = seeded().await;
    let mut run = db.get_run(run_id).await.unwrap();
    assert_eq!(run.status, RunStatus::Queued);
    assert_eq!(run.mr_id, mid);
    assert_eq!(run.model, "opus");
    assert!(run.since_sha.is_none());
    assert!(run.started_at.ends_with('Z'));

    run.status = RunStatus::AwaitingApproval;
    run.turns = 7;
    run.duration_ms = 12_345;
    run.cli_session_id = Some("sess".into());
    run.summary = Some("two issues".into());
    run.verdict = Some("request_changes".into());
    run.error = None;
    run.ended_at = Some("2026-11-25T10:10:00.000Z".into());
    db.update_run(&run).await.unwrap();
    assert_eq!(db.get_run(run_id).await.unwrap(), run);

    let r2 = db
        .create_run(mid, "b1", "h2", Some("h1"), "sonnet")
        .await
        .unwrap();
    assert_eq!(r2.since_sha.as_deref(), Some("h1"));
    assert_eq!(db.get_run(r2.id).await.unwrap(), r2);

    let ids: Vec<i64> = db
        .list_runs(Some(mid), 10)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, vec![r2.id, run_id]);
    assert_eq!(db.list_runs(None, 1).await.unwrap().len(), 1);
    assert!(db.list_runs(Some(999), 10).await.unwrap().is_empty());
    assert_eq!(db.latest_run_for_mr(mid).await.unwrap().unwrap().id, r2.id);
    assert!(db.latest_run_for_mr(999).await.unwrap().is_none());

    let mut ghost = run.clone();
    ghost.id = 999;
    assert!(matches!(
        db.update_run(&ghost).await,
        Err(WardenError::NotFound(_))
    ));
    assert!(matches!(
        db.get_run(999).await,
        Err(WardenError::NotFound(_))
    ));
}

#[tokio::test]
async fn runs_started_since_counts_by_time() {
    let (db, _, mid, run_id) = seeded().await;
    // Push the seeded run into the past.
    let mut old = db.get_run(run_id).await.unwrap();
    old.started_at = "2026-01-01T00:00:00.000Z".into();
    db.update_run(&old).await.unwrap();
    db.create_run(mid, "b", "h", None, "opus").await.unwrap();
    db.create_run(mid, "b", "h", None, "opus").await.unwrap();

    let hour_ago = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    assert_eq!(db.runs_started_since(&hour_ago).await.unwrap(), 2);
    assert_eq!(
        db.runs_started_since("2025-12-31T23:00:00+00:00")
            .await
            .unwrap(),
        3
    );
    // Offsets are normalised: 2026-01-01T01:00:00+01:00 == 2026-01-01T00:00:00Z.
    assert_eq!(
        db.runs_started_since("2026-01-01T01:00:00+01:00")
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        db.runs_started_since("2026-01-01T00:00:01Z").await.unwrap(),
        2
    );
    let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    assert_eq!(db.runs_started_since(&future).await.unwrap(), 0);
}

#[tokio::test]
async fn findings_insert_list_update() {
    let (db, _, _, run_id) = seeded().await;
    let nf = new_finding("SQL built with an f-string", Severity::Blocker);
    let f = db
        .insert_finding(
            run_id,
            &nf,
            FindingStatus::Draft,
            None,
            "fp-1",
            Some(&pos()),
        )
        .await
        .unwrap();
    assert!(f.id > 0);
    assert_eq!(f.fingerprint, "fp-1"); // stored as given, nothing computed
    assert_eq!(f.position, Some(pos()));
    assert_eq!(db.get_finding(f.id).await.unwrap(), f);

    let rejected = db
        .insert_finding(
            run_id,
            &new_finding("weak", Severity::Nit),
            FindingStatus::Rejected,
            Some("below severity floor"),
            "fp-2",
            None,
        )
        .await
        .unwrap();
    let got = db.get_finding(rejected.id).await.unwrap();
    assert_eq!(
        got.guardrail_reason.as_deref(),
        Some("below severity floor")
    );
    assert_eq!(got.position, None);
    assert_eq!(got.status, FindingStatus::Rejected);
    assert!((got.confidence - 0.98).abs() < f32::EPSILON);

    let list = db.list_findings(run_id).await.unwrap();
    assert_eq!(
        list.iter().map(|f| f.id).collect::<Vec<_>>(),
        vec![f.id, rejected.id]
    );
    assert!(db.list_findings(999).await.unwrap().is_empty());

    db.update_finding_text(f.id, "New title", "New body text here.", None)
        .await
        .unwrap();
    let got = db.get_finding(f.id).await.unwrap();
    assert_eq!(got.title, "New title");
    assert_eq!(got.body, "New body text here.");
    assert_eq!(got.suggestion, None);

    db.update_finding_status(f.id, FindingStatus::Approved, None)
        .await
        .unwrap();
    assert_eq!(
        db.get_finding(f.id).await.unwrap().status,
        FindingStatus::Approved
    );
    db.update_finding_status(f.id, FindingStatus::Posted, Some("disc-1"))
        .await
        .unwrap();
    let got = db.get_finding(f.id).await.unwrap();
    assert_eq!(got.status, FindingStatus::Posted);
    assert_eq!(got.forge_note_id.as_deref(), Some("disc-1"));
    // Resolving without a note id keeps the stored id.
    db.update_finding_status(f.id, FindingStatus::Resolved, None)
        .await
        .unwrap();
    let got = db.get_finding(f.id).await.unwrap();
    assert_eq!(got.status, FindingStatus::Resolved);
    assert_eq!(got.forge_note_id.as_deref(), Some("disc-1"));

    assert!(matches!(
        db.get_finding(999).await,
        Err(WardenError::NotFound(_))
    ));
    assert!(matches!(
        db.update_finding_status(999, FindingStatus::Posted, None)
            .await,
        Err(WardenError::NotFound(_))
    ));
    assert!(matches!(
        db.update_finding_text(999, "t", "b", None).await,
        Err(WardenError::NotFound(_))
    ));
    // Finding for an unknown run violates the foreign key.
    assert!(db
        .insert_finding(999, &nf, FindingStatus::Draft, None, "fp", None)
        .await
        .is_err());
}

#[tokio::test]
async fn fingerprints_and_posted_findings_span_runs_of_the_mr() {
    let (db, pid, mid, run1) = seeded().await;
    let run2 = db
        .create_run(mid, "b", "h2", Some("h1"), "opus")
        .await
        .unwrap()
        .id;
    let other_mr = db.upsert_mr(&mr(pid, 2)).await.unwrap();
    let other_run = db
        .create_run(other_mr, "b", "h", None, "opus")
        .await
        .unwrap()
        .id;

    let a = db
        .insert_finding(
            run1,
            &new_finding("a", Severity::Major),
            FindingStatus::Posted,
            None,
            "fp-a",
            None,
        )
        .await
        .unwrap();
    db.insert_finding(
        run1,
        &new_finding("b", Severity::Major),
        FindingStatus::Rejected,
        Some("dup"),
        "fp-b",
        None,
    )
    .await
    .unwrap();
    db.insert_finding(
        run2,
        &new_finding("c", Severity::Minor),
        FindingStatus::Draft,
        None,
        "fp-c",
        None,
    )
    .await
    .unwrap();
    let d = db
        .insert_finding(
            run2,
            &new_finding("d", Severity::Minor),
            FindingStatus::Approved,
            None,
            "fp-d",
            None,
        )
        .await
        .unwrap();
    db.update_finding_status(d.id, FindingStatus::Posted, Some("n-d"))
        .await
        .unwrap();
    db.insert_finding(
        other_run,
        &new_finding("e", Severity::Minor),
        FindingStatus::Posted,
        None,
        "fp-e",
        None,
    )
    .await
    .unwrap();

    let fps = db.fingerprints_for_mr(mid).await.unwrap();
    let mut v: Vec<_> = fps.into_iter().collect();
    v.sort();
    // Only posted / resolved / dismissed count; the pending Draft "fp-c" is not an existing thread.
    assert_eq!(v, vec!["fp-a", "fp-d"]);

    let posted: Vec<i64> = db
        .posted_findings_for_mr(mid)
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.id)
        .collect();
    assert_eq!(posted, vec![a.id, d.id]);
    assert!(db.fingerprints_for_mr(999).await.unwrap().is_empty());
}

#[tokio::test]
async fn posts_since_counts_posted_at() {
    let (db, _, _, run_id) = seeded().await;
    let hour_ago = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    assert_eq!(db.posts_since(&hour_ago).await.unwrap(), 0);

    let f1 = db
        .insert_finding(
            run_id,
            &new_finding("a", Severity::Major),
            FindingStatus::Approved,
            None,
            "a",
            None,
        )
        .await
        .unwrap();
    db.insert_finding(
        run_id,
        &new_finding("b", Severity::Major),
        FindingStatus::Draft,
        None,
        "b",
        None,
    )
    .await
    .unwrap();
    db.insert_finding(
        run_id,
        &new_finding("c", Severity::Major),
        FindingStatus::Posted,
        None,
        "c",
        None,
    )
    .await
    .unwrap();
    assert_eq!(db.posts_since(&hour_ago).await.unwrap(), 1);
    db.update_finding_status(f1.id, FindingStatus::Posted, Some("n1"))
        .await
        .unwrap();
    assert_eq!(db.posts_since(&hour_ago).await.unwrap(), 2);
    // A later resolve still counts as a post made in that hour.
    db.update_finding_status(f1.id, FindingStatus::Resolved, None)
        .await
        .unwrap();
    assert_eq!(db.posts_since(&hour_ago).await.unwrap(), 2);

    // Old posts are outside the window.
    sqlx::query("UPDATE findings SET posted_at = '2026-01-01T00:00:00.000Z' WHERE id = ?")
        .bind(f1.id)
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(db.posts_since(&hour_ago).await.unwrap(), 1);
    assert_eq!(
        db.posts_since(&normalize_ts("2025-01-01T00:00:00Z"))
            .await
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn ci_jobs_crud() {
    let (db, _, _, run_id) = seeded().await;
    let mut job = CiJob {
        id: 0,
        run_id,
        name: "lint".into(),
        kind: "recipe".into(),
        command: "ruff check app/api.py".into(),
        status: CiStatus::Running,
        duration_ms: 0,
        log_path: "/logs/1.log".into(),
        failure_excerpt: None,
    };
    let id = db.insert_ci_job(&job).await.unwrap();
    job.id = id;
    assert_eq!(db.get_ci_job(id).await.unwrap(), job);

    job.status = CiStatus::Failed;
    job.duration_ms = 3_210;
    job.failure_excerpt = Some("E501 line too long".into());
    db.update_ci_job(&job).await.unwrap();
    assert_eq!(db.get_ci_job(id).await.unwrap(), job);

    let id2 = db
        .insert_ci_job(&CiJob {
            run_id,
            name: "tests".into(),
            kind: "gitlab_ci_local".into(),
            status: CiStatus::NotInstalled,
            ..Default::default()
        })
        .await
        .unwrap();
    let jobs = db.list_ci_jobs(run_id).await.unwrap();
    assert_eq!(jobs.iter().map(|j| j.id).collect::<Vec<_>>(), vec![id, id2]);
    assert_eq!(jobs[1].status, CiStatus::NotInstalled);
    assert!(db.list_ci_jobs(999).await.unwrap().is_empty());

    let mut ghost = job.clone();
    ghost.id = 999;
    assert!(matches!(
        db.update_ci_job(&ghost).await,
        Err(WardenError::NotFound(_))
    ));
    assert!(matches!(
        db.get_ci_job(999).await,
        Err(WardenError::NotFound(_))
    ));
}

#[tokio::test]
async fn audit_append_and_list() {
    let (db, _, mid, run_id) = seeded().await;
    let run2 = db.create_run(mid, "b", "h", None, "opus").await.unwrap().id;
    db.audit(Some(run_id), "tool_call", json!({"tool": "mr_get_diff"}))
        .await
        .unwrap();
    db.audit(Some(run2), "post", json!({"finding_id": 1}))
        .await
        .unwrap();
    db.audit(None, "engine", json!({"msg": "status"}))
        .await
        .unwrap();
    db.audit(Some(run_id), "guardrail", json!({"reason": "dup"}))
        .await
        .unwrap();

    let all = db.list_audit(None, 100).await.unwrap();
    assert_eq!(
        all.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        vec!["guardrail", "engine", "post", "tool_call"]
    );
    let r1 = db.list_audit(Some(run_id), 100).await.unwrap();
    assert_eq!(r1.len(), 2);
    assert_eq!(r1[0].payload, json!({"reason": "dup"}));
    assert_eq!(r1[1].payload["tool"], "mr_get_diff");
    assert_eq!(r1[0].run_id, Some(run_id));
    assert!(r1[0].created_at.ends_with('Z'));
    assert_eq!(db.list_audit(None, 2).await.unwrap().len(), 2);
}
