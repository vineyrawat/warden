//! get_stats / rule_stats / simulate_guardrails / list_pending_findings against `Db::open_memory()`,
//! plus the pure helpers in `warden_core::stats`.

use chrono::{Duration, NaiveDate, Utc};
use pretty_assertions::assert_eq;
use warden_core::config::{RuleConfig, WardenConfig};
use warden_core::db::{format_ts, Db};
use warden_core::poster::Poster;
use warden_core::stats::{self, compute_stats};
use warden_core::{
    DiffPosition, Finding, FindingStatus, ForgeKind, MergeRequest, NewFinding, PostingMode,
    Project, ReviewRun, RunStatus, Severity,
};

fn project(remote: &str) -> Project {
    Project {
        id: 0,
        forge: ForgeKind::Gitlab,
        base_url: "https://gitlab.example".into(),
        remote_id: remote.into(),
        name: format!("name-{remote}"),
        local_path: String::new(),
        target_branches: vec!["main".into()],
        mode: PostingMode::Assist,
        poll_interval_secs: 90,
        enabled: true,
    }
}

fn mr(project_id: i64, iid: u64, state: &str) -> MergeRequest {
    MergeRequest {
        id: 0,
        project_id,
        iid,
        title: format!("MR {iid}"),
        web_url: format!("https://gitlab.example/g/p/-/merge_requests/{iid}"),
        head_sha: "h1".into(),
        base_sha: "b1".into(),
        start_sha: "b1".into(),
        state: state.into(),
        ..Default::default()
    }
}

fn nf(rule: Option<&str>, category: &str, title: &str, sev: Severity, conf: f32) -> NewFinding {
    NewFinding {
        file: "app/api.py".into(),
        line: 10,
        side: "new".into(),
        severity: sev,
        category: category.into(),
        rule_id: rule.map(str::to_string),
        title: title.into(),
        body: "Body text for the finding.".into(),
        suggestion: None,
        evidence: Some("line 10".into()),
        confidence: conf,
        source: "claude".into(),
    }
}

fn pos() -> DiffPosition {
    DiffPosition {
        base_sha: "b1".into(),
        start_sha: "b1".into(),
        head_sha: "h1".into(),
        old_path: "app/api.py".into(),
        new_path: "app/api.py".into(),
        old_line: None,
        new_line: Some(10),
    }
}

/// Creates a finished run started `days_ago` days ago (noon UTC) and returns it.
async fn run_at(db: &Db, mr_id: i64, days_ago: i64, status: RunStatus, ms: u64) -> ReviewRun {
    let mut run = db
        .create_run(mr_id, "b1", "h1", None, "opus")
        .await
        .unwrap();
    let day = Utc::now().date_naive() - Duration::days(days_ago);
    let start = day.and_hms_opt(12, 0, 0).unwrap().and_utc();
    run.started_at = format_ts(start);
    run.ended_at = Some(format_ts(start + Duration::milliseconds(ms as i64)));
    run.duration_ms = ms;
    run.turns = 5;
    run.status = status;
    db.update_run(&run).await.unwrap();
    run
}

async fn add(db: &Db, run_id: i64, f: NewFinding, status: FindingStatus) -> Finding {
    let fp = format!("{}:{}:{}", f.file, f.title, run_id);
    let reason = (status == FindingStatus::Rejected).then_some("over note budget");
    db.insert_finding(run_id, &f, status, reason, &fp, Some(&pos()))
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------------------------
// pure helpers
// ---------------------------------------------------------------------------------------------

fn run_row(id: i64, started: &str, status: RunStatus, ms: u64) -> ReviewRun {
    ReviewRun {
        id,
        mr_id: 1,
        status,
        turns: 3,
        duration_ms: ms,
        started_at: started.into(),
        ended_at: Some(started.into()),
        ..Default::default()
    }
}

fn finding_row(id: i64, run_id: i64, status: FindingStatus) -> Finding {
    Finding {
        id,
        run_id,
        file: "a.py".into(),
        line: 1,
        side: "new".into(),
        severity: Severity::Major,
        category: "correctness".into(),
        rule_id: None,
        title: format!("t{id}"),
        body: "b".into(),
        suggestion: None,
        evidence: None,
        confidence: 0.9,
        source: "claude".into(),
        status,
        guardrail_reason: None,
        fingerprint: format!("fp{id}"),
        forge_note_id: None,
        position: Some(pos()),
    }
}

#[test]
fn compute_stats_fills_zero_days_and_medians() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
    let runs = vec![
        run_row(1, "2026-11-20T09:00:00.000Z", RunStatus::Posted, 100_000),
        run_row(2, "2026-11-20T15:00:00.000Z", RunStatus::Posted, 300_000),
        run_row(
            3,
            "2026-11-25T08:00:00Z",
            RunStatus::AwaitingApproval,
            200_000,
        ),
        run_row(4, "2026-11-24T08:00:00Z", RunStatus::Skipped, 0),
        // outside the 7-day window
        run_row(5, "2026-11-10T08:00:00Z", RunStatus::Posted, 999_000),
    ];
    let findings = vec![
        (
            finding_row(1, 1, FindingStatus::Posted),
            runs[0].started_at.clone(),
            Some("2026-11-21T10:00:00.000Z".into()),
        ),
        (
            finding_row(2, 1, FindingStatus::Resolved),
            runs[0].started_at.clone(),
            None,
        ),
        (
            finding_row(3, 2, FindingStatus::Dismissed),
            runs[1].started_at.clone(),
            None,
        ),
        (
            finding_row(4, 3, FindingStatus::Resolved),
            runs[2].started_at.clone(),
            Some("2026-11-25T09:00:00+00:00".into()),
        ),
    ];
    let s = compute_stats(&runs, &findings, 7, today);
    assert_eq!(s.per_day.len(), 7);
    assert_eq!(s.per_day[0].date, "2026-11-19");
    assert_eq!(s.per_day[6].date, "2026-11-25");
    assert_eq!(s.reviews, 3, "skipped and out-of-window runs do not count");
    let by: Vec<(u32, u32)> = s.per_day.iter().map(|d| (d.reviews, d.notes)).collect();
    assert_eq!(
        by,
        vec![(0, 0), (2, 1), (0, 1), (0, 0), (0, 0), (0, 0), (1, 1)]
    );
    assert_eq!(s.notes_posted, 3);
    assert_eq!(s.resolved, 2);
    assert!((s.resolved_rate - 2.0 / 3.0).abs() < 1e-9);
    assert_eq!(s.median_run_ms, 200_000);
}

#[test]
fn compute_stats_empty_is_zero() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 25).unwrap();
    let s = compute_stats(&[], &[], 14, today);
    assert_eq!(s.per_day.len(), 14);
    assert_eq!(
        (s.reviews, s.notes_posted, s.resolved, s.median_run_ms),
        (0, 0, 0, 0)
    );
    assert_eq!(s.resolved_rate, 0.0);
    assert!(s.per_day.iter().all(|d| d.reviews == 0 && d.notes == 0));
}

#[test]
fn rule_key_falls_back_to_category() {
    let mut f = finding_row(1, 1, FindingStatus::Draft);
    assert_eq!(stats::rule_key(&f), "correctness");
    f.rule_id = Some(" FR-007 ".into());
    assert_eq!(stats::rule_key(&f), "FR-007");
    f.rule_id = Some(String::new());
    f.category = String::new();
    assert_eq!(stats::rule_key(&f), "uncategorized");
}

// ---------------------------------------------------------------------------------------------
// against the database
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn get_stats_from_db() {
    let db = Db::open_memory().await.unwrap();
    let pid = db.upsert_project(&project("g/p")).await.unwrap();
    let mid = db.upsert_mr(&mr(pid, 1, "merged")).await.unwrap();
    let r1 = run_at(&db, mid, 3, RunStatus::Posted, 120_000).await;
    let r2 = run_at(&db, mid, 1, RunStatus::Posted, 240_000).await;
    let _old = run_at(&db, mid, 40, RunStatus::Posted, 1_000).await;
    add(
        &db,
        r1.id,
        nf(Some("FR-001"), "security", "a", Severity::Major, 0.9),
        FindingStatus::Resolved,
    )
    .await;
    add(
        &db,
        r1.id,
        nf(Some("FR-002"), "security", "b", Severity::Major, 0.9),
        FindingStatus::Dismissed,
    )
    .await;
    let p = add(
        &db,
        r2.id,
        nf(None, "tests", "c", Severity::Minor, 0.9),
        FindingStatus::Draft,
    )
    .await;
    // posting it stamps posted_at = now (today)
    db.update_finding_status(p.id, FindingStatus::Posted, Some("disc-1"))
        .await
        .unwrap();

    let s = stats::get_stats(&db, 14).await.unwrap();
    assert_eq!(s.per_day.len(), 14);
    assert_eq!(s.reviews, 2);
    assert_eq!(s.notes_posted, 2);
    assert_eq!(s.resolved, 1);
    assert!((s.resolved_rate - 0.5).abs() < 1e-9);
    assert_eq!(s.median_run_ms, 180_000);
    assert_eq!(
        s.per_day[13].date,
        Utc::now().date_naive().format("%Y-%m-%d").to_string()
    );
    assert_eq!(s.per_day[13].notes, 1, "posted today");
    assert_eq!(s.per_day[10].reviews, 1, "3 days ago");
    assert_eq!(
        s.per_day[10].notes, 1,
        "resolved finding without posted_at counts on its run day"
    );
}

#[tokio::test]
async fn rule_stats_groups_by_rule_and_filters_by_project() {
    let db = Db::open_memory().await.unwrap();
    let p1 = db.upsert_project(&project("g/one")).await.unwrap();
    let p2 = db.upsert_project(&project("g/two")).await.unwrap();
    let m1 = db.upsert_mr(&mr(p1, 1, "opened")).await.unwrap();
    let m2 = db.upsert_mr(&mr(p2, 2, "opened")).await.unwrap();
    let ra = run_at(&db, m1, 5, RunStatus::Posted, 1).await;
    let rb = run_at(&db, m1, 2, RunStatus::Posted, 1).await;
    let rc = run_at(&db, m2, 1, RunStatus::Posted, 1).await;
    let fr7 = |t: &str| nf(Some("FR-007"), "correctness", t, Severity::Major, 0.9);
    add(&db, ra.id, fr7("commit old"), FindingStatus::Dismissed).await;
    add(&db, ra.id, fr7("commit 2"), FindingStatus::Posted).await;
    add(&db, rb.id, fr7("commit newest"), FindingStatus::Dismissed).await;
    add(&db, rb.id, fr7("commit 4"), FindingStatus::Rejected).await;
    add(
        &db,
        rb.id,
        nf(None, "tests", "flaky test", Severity::Minor, 0.8),
        FindingStatus::Resolved,
    )
    .await;
    add(&db, rc.id, fr7("other project"), FindingStatus::Draft).await;

    let rows = stats::rule_stats(&db, Some(p1), 30, &[]).await.unwrap();
    assert_eq!(rows.len(), 2);
    let r = &rows[0];
    assert_eq!(r.rule_id, "FR-007");
    assert_eq!((r.hits, r.posted, r.dismissed, r.rejected), (4, 1, 2, 1));
    assert_eq!(r.title, "commit 4", "most recent finding title");
    assert_eq!(r.last_hit_at.as_deref(), Some(rb.started_at.as_str()));
    assert_eq!(rows[1].rule_id, "tests");
    assert_eq!((rows[1].hits, rows[1].posted), (1, 1));

    // configured title wins; all projects
    let rules = vec![RuleConfig {
        id: "FR-007".into(),
        title: "No frappe.db.commit() in request handlers".into(),
        applies_to: None,
        instruction: None,
        pattern: Some("x".into()),
        semgrep: None,
        severity: Severity::Major,
    }];
    let all = stats::rule_stats(&db, None, 30, &rules).await.unwrap();
    assert_eq!(all[0].hits, 5);
    assert_eq!(all[0].title, "No frappe.db.commit() in request handlers");
    // window: 3 days excludes run ra (5 days ago)
    let recent = stats::rule_stats(&db, Some(p1), 3, &[]).await.unwrap();
    assert_eq!(recent[0].hits, 2);
}

#[tokio::test]
async fn simulate_guardrails_classifies_findings() {
    let db = Db::open_memory().await.unwrap();
    let pid = db.upsert_project(&project("g/p")).await.unwrap();
    let mid = db.upsert_mr(&mr(pid, 7, "opened")).await.unwrap();
    let old = run_at(&db, mid, 4, RunStatus::Posted, 1).await;
    let run = run_at(&db, mid, 1, RunStatus::AwaitingApproval, 1).await;
    add(
        &db,
        old.id,
        nf(
            Some("FR-001"),
            "security",
            "older run",
            Severity::Major,
            0.9,
        ),
        FindingStatus::Posted,
    )
    .await;
    let blocker = add(
        &db,
        run.id,
        nf(Some("FR-002"), "security", "sql", Severity::Blocker, 0.95),
        FindingStatus::Draft,
    )
    .await;
    let noisy = add(
        &db,
        run.id,
        nf(
            Some("FR-007"),
            "correctness",
            "commit",
            Severity::Major,
            0.9,
        ),
        FindingStatus::Draft,
    )
    .await;
    let low = add(
        &db,
        run.id,
        nf(None, "ux", "low conf", Severity::Minor, 0.6),
        FindingStatus::Draft,
    )
    .await;
    let minor = add(
        &db,
        run.id,
        nf(None, "tests", "minor ok", Severity::Minor, 0.8),
        FindingStatus::Draft,
    )
    .await;
    let nit = add(
        &db,
        run.id,
        nf(None, "style", "nit", Severity::Nit, 0.99),
        FindingStatus::Draft,
    )
    .await;
    // Rejected for a non-policy reason stays dropped with that reason.
    let anchored = db
        .insert_finding(
            run.id,
            &nf(None, "correctness", "bad anchor", Severity::Major, 0.99),
            FindingStatus::Rejected,
            Some("line 3 of app/api.py is outside the diff"),
            "fp-anchor",
            None,
        )
        .await
        .unwrap();

    let mut cfg = WardenConfig::default();
    cfg.posting.mode = PostingMode::Assist;
    let disabled = vec!["FR-007".to_string()];
    let rows = stats::simulate_guardrails(&db, pid, &cfg, &disabled, 1)
        .await
        .unwrap();
    assert_eq!(rows.len(), 6, "only the newest run");
    let get = |id: i64| rows.iter().find(|r| r.finding.id == id).unwrap();
    assert!(rows
        .iter()
        .all(|r| r.mr_label == "!7 MR 7" && !r.reason.is_empty()));
    assert_eq!(get(blocker.id).outcome, "post");
    assert_eq!(get(noisy.id).outcome, "drop");
    assert_eq!(get(noisy.id).reason, "rule FR-007 is disabled");
    assert_eq!(get(low.id).outcome, "drop");
    assert!(get(low.id).reason.contains("below the threshold"));
    assert_eq!(get(minor.id).outcome, "draft");
    assert_eq!(get(nit.id).outcome, "drop");
    assert_eq!(
        get(anchored.id).reason,
        "line 3 of app/api.py is outside the diff"
    );

    // Budget of 1 keeps the blocker; Draft mode posts nothing; both runs included.
    cfg.review.max_notes = 1;
    cfg.posting.mode = PostingMode::Draft;
    let rows = stats::simulate_guardrails(&db, pid, &cfg, &[], 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 7);
    let get = |id: i64| rows.iter().find(|r| r.finding.id == id).unwrap();
    assert_eq!(get(blocker.id).outcome, "draft");
    assert_eq!(get(blocker.id).reason, "draft mode: waits for approval");
    assert_eq!(get(noisy.id).outcome, "drop");
    assert!(get(noisy.id).reason.starts_with("over note budget"));
    assert!(rows.iter().all(|r| r.outcome != "post"));

    // Auto mode, nits on, low threshold: everything not structurally rejected posts.
    cfg.review.max_notes = 10;
    cfg.review.nits = true;
    cfg.review.severity_floor = Severity::Nit;
    cfg.review.min_confidence = 0.0;
    cfg.posting.mode = PostingMode::Auto;
    let rows = stats::simulate_guardrails(&db, pid, &cfg, &[], 1)
        .await
        .unwrap();
    let drops: Vec<i64> = rows
        .iter()
        .filter(|r| r.outcome == "drop")
        .map(|r| r.finding.id)
        .collect();
    assert_eq!(drops, vec![anchored.id]);
}

#[tokio::test]
async fn pending_findings_come_from_latest_run_of_open_mrs() {
    let db = Db::open_memory().await.unwrap();
    let pid = db.upsert_project(&project("g/p")).await.unwrap();
    let open = db.upsert_mr(&mr(pid, 3, "opened")).await.unwrap();
    let open2 = db.upsert_mr(&mr(pid, 9, "opened")).await.unwrap();
    let merged = db.upsert_mr(&mr(pid, 5, "merged")).await.unwrap();
    let stale = run_at(&db, open, 3, RunStatus::AwaitingApproval, 1).await;
    let latest = run_at(&db, open, 1, RunStatus::AwaitingApproval, 1).await;
    let r9 = run_at(&db, open2, 1, RunStatus::AwaitingApproval, 1).await;
    let rm = run_at(&db, merged, 1, RunStatus::AwaitingApproval, 1).await;
    add(
        &db,
        stale.id,
        nf(None, "x", "stale draft", Severity::Major, 0.9),
        FindingStatus::Draft,
    )
    .await;
    let a = add(
        &db,
        latest.id,
        nf(None, "x", "draft", Severity::Major, 0.9),
        FindingStatus::Draft,
    )
    .await;
    let b = add(
        &db,
        latest.id,
        nf(None, "x", "approved", Severity::Major, 0.9),
        FindingStatus::Approved,
    )
    .await;
    add(
        &db,
        latest.id,
        nf(None, "x", "posted", Severity::Major, 0.9),
        FindingStatus::Posted,
    )
    .await;
    add(
        &db,
        latest.id,
        nf(None, "x", "dismissed", Severity::Major, 0.9),
        FindingStatus::Dismissed,
    )
    .await;
    let c = add(
        &db,
        r9.id,
        nf(None, "x", "nine", Severity::Minor, 0.9),
        FindingStatus::Draft,
    )
    .await;
    add(
        &db,
        rm.id,
        nf(None, "x", "merged mr", Severity::Major, 0.9),
        FindingStatus::Draft,
    )
    .await;

    let rows = stats::list_pending_findings(&db).await.unwrap();
    let ids: Vec<i64> = rows.iter().map(|r| r.finding.id).collect();
    assert_eq!(
        ids,
        vec![c.id, a.id, b.id],
        "iid desc, then insertion order"
    );
    assert_eq!(rows[0].mr_iid, 9);
    assert_eq!(rows[1].run_id, latest.id);
    assert_eq!(rows[1].mr_id, open);
    assert_eq!(rows[1].project_name, "name-g/p");
    assert_eq!(
        rows[1].web_url,
        "https://gitlab.example/g/p/-/merge_requests/3"
    );

    let groups = stats::group_by_run(&db, &[c.id, a.id, b.id, a.id])
        .await
        .unwrap();
    assert_eq!(
        groups,
        vec![(latest.id, vec![a.id, b.id]), (r9.id, vec![c.id])]
    );
    assert!(stats::group_by_run(&db, &[999_999]).await.is_err());
}

#[tokio::test]
async fn preview_note_matches_poster_format() {
    let db = Db::open_memory().await.unwrap();
    let pid = db.upsert_project(&project("g/p")).await.unwrap();
    let mid = db.upsert_mr(&mr(pid, 1, "opened")).await.unwrap();
    let run = run_at(&db, mid, 0, RunStatus::AwaitingApproval, 1).await;
    let f = add(
        &db,
        run.id,
        nf(
            Some("FR-002"),
            "security",
            "SQL from input",
            Severity::Blocker,
            0.98,
        ),
        FindingStatus::Draft,
    )
    .await;
    let md = Poster::preview_note(&f);
    assert_eq!(md, Poster::format_note(&f));
    assert!(md.starts_with("🔴 Blocker · **SQL from input**"));
    assert!(md.ends_with("— Warden · rule FR-002 · confidence 0.98"));
    let mut nopos = f.clone();
    nopos.position = None;
    assert!(Poster::preview_note(&nopos).starts_with("**app/api.py:10**"));
}

#[tokio::test]
async fn config_with_candidate_merges_over_stored_overrides() {
    use std::sync::Arc;
    use warden_core::mcp::{McpServer, RunRegistry};
    use warden_core::pipeline::Pipeline;
    use warden_core::EventBus;

    let db = Db::open_memory().await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let events = EventBus::new();
    let registry = Arc::new(RunRegistry::new());
    let mcp = McpServer::start(0, registry.clone()).await.unwrap();
    let ci = Arc::new(Pipeline::ci_runner(
        &db,
        events.clone(),
        tmp.path().join("logs"),
    ));
    let p = Pipeline::new(db.clone(), events, registry, ci, mcp.url());

    let pid = db.upsert_project(&project("g/p")).await.unwrap();
    let proj = db.get_project(pid).await.unwrap();
    db.save_project_overrides(
        pid,
        &serde_json::json!({"review": {"max_notes": 4, "nits": true}, "disabled_rules": ["FR-001"]}),
    )
    .await
    .unwrap();

    // Empty candidate = stored overrides; mode from the project.
    let (cfg, disabled) = p
        .config_with_candidate(&proj, &serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(cfg.review.max_notes, 4);
    assert!(cfg.review.nits);
    assert_eq!(disabled, vec!["FR-001".to_string()]);
    assert_eq!(cfg.posting.mode, PostingMode::Assist);
    assert!(
        cfg.rules.iter().any(|r| r.id == "FR-001"),
        "rules kept for titles"
    );

    // Candidate wins field by field; its disabled_rules and posting.mode replace the stored ones.
    let cand = serde_json::json!({
        "review": {"max_notes": 2},
        "posting": {"mode": "auto"},
        "disabled_rules": ["FR-007"],
    });
    let (cfg, disabled) = p.config_with_candidate(&proj, &cand).await.unwrap();
    assert_eq!(cfg.review.max_notes, 2);
    assert!(cfg.review.nits, "stored value kept");
    assert_eq!(cfg.posting.mode, PostingMode::Auto);
    assert_eq!(disabled, vec!["FR-007".to_string()]);

    assert!(p
        .config_with_candidate(&proj, &serde_json::json!([1]))
        .await
        .is_err());
    assert!(p
        .config_with_candidate(&proj, &serde_json::json!({"disabled_rules": "FR-1"}))
        .await
        .is_err());
    mcp.stop().await;
}

#[tokio::test]
async fn list_run_rows_joins_mr_project_and_counts() {
    use warden_core::{CiJob, CiStatus};
    let db = Db::open_memory().await.unwrap();
    let pid = db.upsert_project(&project("g/p")).await.unwrap();
    let mut local = project("local/repo");
    local.base_url = warden_core::pipeline::LOCAL_BASE_URL.into();
    local.name = "repo".into();
    let lid = db.upsert_project(&local).await.unwrap();
    let merged = db.upsert_mr(&mr(pid, 4, "merged")).await.unwrap();
    let lmr = db.upsert_mr(&mr(lid, 1, "opened")).await.unwrap();
    let r1 = run_at(&db, merged, 2, RunStatus::Posted, 1).await;
    let r2 = run_at(&db, lmr, 1, RunStatus::AwaitingApproval, 1).await;
    for (st, t) in [
        (FindingStatus::Draft, "a"),
        (FindingStatus::Approved, "b"),
        (FindingStatus::Posted, "c"),
        (FindingStatus::Resolved, "d"),
        (FindingStatus::Rejected, "e"),
        (FindingStatus::Dismissed, "f"),
    ] {
        add(&db, r1.id, nf(None, "x", t, Severity::Major, 0.9), st).await;
    }
    for (name, status) in [
        ("lint", CiStatus::Passed),
        ("unit", CiStatus::Failed),
        ("e2e", CiStatus::TimedOut),
        ("build", CiStatus::Running),
        ("sem", CiStatus::NotInstalled),
    ] {
        db.insert_ci_job(&CiJob {
            run_id: r1.id,
            name: name.into(),
            status,
            ..Default::default()
        })
        .await
        .unwrap();
    }

    let rows = db
        .list_run_rows(10, warden_core::pipeline::LOCAL_BASE_URL)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].run.id, r2.id, "newest first");
    assert_eq!(rows[0].project_id, 0, "local marker");
    assert_eq!(rows[0].project_name, "repo");
    let r = &rows[1];
    assert_eq!(r.run, db.get_run(r1.id).await.unwrap());
    assert_eq!((r.mr_iid, r.mr_title.as_str()), (4, "MR 4"));
    assert_eq!(r.project_id, pid);
    assert_eq!(r.project_name, "name-g/p");
    assert_eq!(r.web_url, "https://gitlab.example/g/p/-/merge_requests/4");
    assert!(!r.draft);
    assert_eq!(
        (
            r.finding_counts.draft,
            r.finding_counts.posted,
            r.finding_counts.rejected
        ),
        (2, 2, 1)
    );
    let ci = &r.ci_summary;
    assert_eq!((ci.passed, ci.failed, ci.running, ci.total), (1, 2, 1, 5));
    assert_eq!(db.list_run_rows(1, "local://").await.unwrap().len(), 1);
}
