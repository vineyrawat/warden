// OWNER: poster
//! Poster tests with a hand-written fake ForgeProvider (no network, hard rule 3).

use std::collections::HashSet;
use std::sync::Mutex;

use async_trait::async_trait;
use pretty_assertions::assert_eq;
use serde_json::json;
use warden_core::config::{AppSettings, PostingConfig};
use warden_core::db::Db;
use warden_core::forge::{ForgeDiscussion, ForgeProvider};
use warden_core::poster::{PostReport, Poster};
use warden_core::{
    AppEvent, CiJob, CiStatus, DiffPosition, Finding, FindingStatus, ForgeKind, MergeRequest,
    NewFinding, PostingMode, Project, Result, ReviewRun, Severity, WardenError,
};

// ---------------------------------------------------------------------------------------------
// Fake forge
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Call {
    LineNote {
        iid: u64,
        body: String,
        new_line: Option<u32>,
    },
    Note {
        iid: u64,
        body: String,
    },
    Resolve {
        iid: u64,
        discussion: String,
    },
    Label {
        iid: u64,
        label: String,
    },
}

#[derive(Default)]
struct FakeForge {
    calls: Mutex<Vec<Call>>,
    /// new_line values whose line note returns Forge{status: 400}.
    reject_lines: HashSet<u32>,
    /// new_line values whose line note returns Forge{status: 422}.
    reject_lines_422: HashSet<u32>,
    /// new_line values whose line note fails with a non-position error (500).
    fail_lines: HashSet<u32>,
    fail_notes: bool,
    fail_label: bool,
    fail_resolve: HashSet<String>,
    counter: Mutex<u32>,
}

impl FakeForge {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    fn next_id(&self, prefix: &str) -> String {
        let mut c = self.counter.lock().unwrap();
        *c += 1;
        format!("{prefix}-{c}")
    }
    fn line_notes(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::LineNote { body, .. } => Some(body),
                _ => None,
            })
            .collect()
    }
    fn notes(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::Note { body, .. } => Some(body),
                _ => None,
            })
            .collect()
    }
    fn labels(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::Label { label, .. } => Some(label),
                _ => None,
            })
            .collect()
    }
}

#[async_trait]
impl ForgeProvider for FakeForge {
    async fn whoami(&self) -> Result<String> {
        Ok("warden-bot".into())
    }
    async fn list_projects(&self, _search: &str) -> Result<Vec<(String, String)>> {
        Ok(vec![])
    }
    async fn list_open_mrs(&self, _project: &Project) -> Result<Vec<MergeRequest>> {
        Ok(vec![])
    }
    async fn get_mr(&self, _project: &Project, _iid: u64) -> Result<MergeRequest> {
        Err(WardenError::NotFound("fake".into()))
    }
    async fn list_discussions(&self, _p: &Project, _iid: u64) -> Result<Vec<ForgeDiscussion>> {
        Ok(vec![])
    }
    async fn post_line_note(
        &self,
        _project: &Project,
        iid: u64,
        body: &str,
        pos: &DiffPosition,
    ) -> Result<String> {
        self.calls.lock().unwrap().push(Call::LineNote {
            iid,
            body: body.into(),
            new_line: pos.new_line,
        });
        let line = pos.new_line.unwrap_or(0);
        if self.reject_lines.contains(&line) {
            return Err(WardenError::Forge {
                status: 400,
                msg: "line_code can't be blank".into(),
            });
        }
        if self.reject_lines_422.contains(&line) {
            return Err(WardenError::Forge {
                status: 422,
                msg: "line must be part of the diff".into(),
            });
        }
        if self.fail_lines.contains(&line) {
            return Err(WardenError::Forge {
                status: 500,
                msg: "boom".into(),
            });
        }
        Ok(self.next_id("disc"))
    }
    async fn post_note(&self, _project: &Project, iid: u64, body: &str) -> Result<String> {
        self.calls.lock().unwrap().push(Call::Note {
            iid,
            body: body.into(),
        });
        if self.fail_notes {
            return Err(WardenError::Http("connection reset".into()));
        }
        Ok(self.next_id("note"))
    }
    async fn resolve_discussion(&self, _p: &Project, iid: u64, discussion_id: &str) -> Result<()> {
        self.calls.lock().unwrap().push(Call::Resolve {
            iid,
            discussion: discussion_id.into(),
        });
        if self.fail_resolve.contains(discussion_id) {
            return Err(WardenError::Forge {
                status: 404,
                msg: "no discussion".into(),
            });
        }
        Ok(())
    }
    async fn add_label(&self, _p: &Project, iid: u64, label: &str) -> Result<()> {
        self.calls.lock().unwrap().push(Call::Label {
            iid,
            label: label.into(),
        });
        if self.fail_label {
            return Err(WardenError::Forge {
                status: 403,
                msg: "forbidden".into(),
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

struct Fx {
    db: Db,
    project: Project,
    mr: MergeRequest,
    run: ReviewRun,
}

async fn fixture() -> Fx {
    let db = Db::open_memory().await.unwrap();
    let mut project = Project {
        id: 0,
        forge: ForgeKind::Gitlab,
        base_url: "https://gitlab.example".into(),
        remote_id: "entries/entries_crm".into(),
        name: "entries_crm".into(),
        local_path: "/tmp/repo".into(),
        target_branches: vec!["main".into()],
        mode: PostingMode::Assist,
        poll_interval_secs: 120,
        enabled: true,
    };
    project.id = db.upsert_project(&project).await.unwrap();
    let mut mr = MergeRequest {
        project_id: project.id,
        iid: 77,
        title: "Linker endpoint".into(),
        head_sha: "0123456789abcdef0123".into(),
        base_sha: "base".into(),
        start_sha: "start".into(),
        state: "opened".into(),
        ..Default::default()
    };
    mr.id = db.upsert_mr(&mr).await.unwrap();
    let mut run = db
        .create_run(mr.id, "base", &mr.head_sha, None, "opus")
        .await
        .unwrap();
    run.summary = Some("Two blocking issues in the new linker endpoint.".into());
    run.verdict = Some("request_changes".into());
    db.update_run(&run).await.unwrap();
    Fx {
        db,
        project,
        mr,
        run,
    }
}

fn pos(line: u32) -> DiffPosition {
    DiffPosition {
        base_sha: "base".into(),
        start_sha: "start".into(),
        head_sha: "0123456789abcdef0123".into(),
        old_path: "app/api.py".into(),
        new_path: "app/api.py".into(),
        old_line: None,
        new_line: Some(line),
    }
}

fn nf(title: &str, line: u32, severity: Severity, confidence: f32) -> NewFinding {
    NewFinding {
        file: "app/api.py".into(),
        line,
        side: "new".into(),
        severity,
        category: "security".into(),
        rule_id: Some("FR-002".into()),
        title: title.into(),
        body: "The SQL is built with an f-string from request input; use parameters.".into(),
        suggestion: Some("frappe.db.sql(q, values)".into()),
        evidence: None,
        confidence,
        source: "claude".into(),
    }
}

async fn add(fx: &Fx, title: &str, line: u32, sev: Severity, status: FindingStatus) -> Finding {
    fx.db
        .insert_finding(
            fx.run.id,
            &nf(title, line, sev, 0.9),
            status,
            None,
            &format!("fp-{title}"),
            Some(&pos(line)),
        )
        .await
        .unwrap()
}

async fn status_of(fx: &Fx, id: i64) -> Finding {
    fx.db.get_finding(id).await.unwrap()
}

async fn post_audits(fx: &Fx) -> Vec<serde_json::Value> {
    fx.db
        .list_audit(Some(fx.run.id), 1000)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "post")
        .map(|e| e.payload)
        .collect()
}

fn finding_for_format() -> Finding {
    Finding {
        id: 1,
        run_id: 1,
        file: "entries_crm/api/linker.py".into(),
        line: 42,
        side: "new".into(),
        severity: Severity::Blocker,
        category: "security".into(),
        rule_id: Some("FR-002".into()),
        title: "SQL built with an f-string from request input".into(),
        body: "`name` comes straight from the request and is interpolated into the query.".into(),
        suggestion: Some(
            "frappe.db.sql(\"select name from tabLead where name=%s\", (name,))".into(),
        ),
        evidence: None,
        confidence: 0.98,
        source: "claude".into(),
        status: FindingStatus::Approved,
        guardrail_reason: None,
        fingerprint: "fp".into(),
        forge_note_id: None,
        position: None,
    }
}

// ---------------------------------------------------------------------------------------------
// post_run
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn posts_approved_findings_as_line_notes() {
    let fx = fixture().await;
    let a = add(&fx, "a", 10, Severity::Major, FindingStatus::Approved).await;
    let b = add(&fx, "b", 20, Severity::Blocker, FindingStatus::Approved).await;
    let draft = add(&fx, "c", 30, Severity::Major, FindingStatus::Draft).await;
    let rejected = add(&fx, "d", 40, Severity::Major, FindingStatus::Rejected).await;
    let dismissed = add(&fx, "e", 50, Severity::Major, FindingStatus::Dismissed).await;

    let forge = FakeForge::default();
    let events = warden_core::EventBus::new();
    let mut rx = events.subscribe();
    let poster = Poster::new(events);
    let report = poster
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(
        report,
        PostReport {
            posted: 2,
            fallback: 0,
            failed: 0,
            skipped: 0
        }
    );

    let calls = forge.calls();
    // Blocker first (severity order), then the major; then summary, then label.
    assert!(matches!(
        &calls[0],
        Call::LineNote {
            new_line: Some(20),
            iid: 77,
            ..
        }
    ));
    assert!(matches!(
        &calls[1],
        Call::LineNote {
            new_line: Some(10),
            ..
        }
    ));
    assert!(matches!(&calls[2], Call::Note { .. }));
    assert_eq!(
        calls[3],
        Call::Label {
            iid: 77,
            label: "ai-reviewed".into()
        }
    );
    assert_eq!(calls.len(), 4);
    assert_eq!(forge.line_notes()[0], Poster::format_note(&b));

    for f in [&a, &b] {
        let got = status_of(&fx, f.id).await;
        assert_eq!(got.status, FindingStatus::Posted);
        assert!(got.forge_note_id.as_deref().unwrap().starts_with("disc-"));
    }
    for f in [&draft, &rejected, &dismissed] {
        assert_eq!(status_of(&fx, f.id).await.status, f.status);
    }

    let audits = post_audits(&fx).await;
    let posted: Vec<_> = audits
        .iter()
        .filter(|p| p["kind"] == "finding" && p["outcome"] == "posted")
        .collect();
    assert_eq!(posted.len(), 2);
    assert!(posted
        .iter()
        .any(|p| p["finding_id"] == a.id && p["note_id"].is_string()));
    assert!(audits
        .iter()
        .any(|p| p["kind"] == "summary" && p["outcome"] == "posted"));
    assert!(audits
        .iter()
        .any(|p| p["kind"] == "label" && p["outcome"] == "labeled"));

    match rx.try_recv().unwrap() {
        AppEvent::FindingsUpdated { run_id } => assert_eq!(run_id, fx.run.id),
        other => panic!("unexpected {other:?}"),
    }

    // Second call: nothing approved left, nothing posted, no extra summary / label.
    let report = poster
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(report, PostReport::default());
    assert_eq!(forge.calls().len(), 4);
}

#[tokio::test]
async fn only_ids_post_selected_and_skip_ineligible() {
    let fx = fixture().await;
    let draft = add(&fx, "a", 10, Severity::Major, FindingStatus::Draft).await;
    let approved_not_selected = add(&fx, "b", 20, Severity::Major, FindingStatus::Approved).await;
    let rejected = add(&fx, "c", 30, Severity::Major, FindingStatus::Rejected).await;
    let dismissed = add(&fx, "d", 40, Severity::Major, FindingStatus::Dismissed).await;

    let forge = FakeForge::default();
    let report = Poster::default()
        .post_run(
            &fx.db,
            &forge,
            &fx.project,
            &fx.mr,
            &fx.run,
            Some(&[draft.id, rejected.id, dismissed.id, 9999, draft.id]),
        )
        .await
        .unwrap();
    assert_eq!(report.posted, 1);
    assert_eq!(report.skipped, 3); // rejected, dismissed, unknown id (duplicate id ignored)
    assert_eq!(forge.line_notes().len(), 1);
    assert_eq!(status_of(&fx, draft.id).await.status, FindingStatus::Posted);
    assert_eq!(
        status_of(&fx, approved_not_selected.id).await.status,
        FindingStatus::Approved
    );

    // Already posted id is skipped on a second call.
    let report = Poster::default()
        .post_run(
            &fx.db,
            &forge,
            &fx.project,
            &fx.mr,
            &fx.run,
            Some(&[draft.id]),
        )
        .await
        .unwrap();
    assert_eq!(
        report,
        PostReport {
            skipped: 1,
            ..Default::default()
        }
    );
    let audits = post_audits(&fx).await;
    assert!(audits.iter().any(|p| p["outcome"] == "skipped_status"));
    assert!(audits.iter().any(|p| p["outcome"] == "skipped_unknown"));
}

#[tokio::test]
async fn position_400_falls_back_to_file_level_note() {
    let fx = fixture().await;
    let ok = add(&fx, "ok", 10, Severity::Major, FindingStatus::Approved).await;
    let bad = add(&fx, "bad", 99, Severity::Blocker, FindingStatus::Approved).await;
    let forge = FakeForge {
        reject_lines: [99].into(),
        ..Default::default()
    };
    let report = Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(
        report,
        PostReport {
            posted: 2,
            fallback: 1,
            failed: 0,
            skipped: 0
        }
    );
    let notes = forge.notes();
    // First note is the fallback, last is the summary.
    assert_eq!(notes.len(), 2);
    assert!(notes[0].starts_with("**app/api.py:99**\n\n🔴 Blocker · **bad**"));
    assert!(
        !notes[0].contains("suggestion:-0+0"),
        "fallback must not use a suggestion block"
    );
    assert!(notes[0].contains("```\nfrappe.db.sql(q, values)\n```"));
    assert!(notes[1].starts_with("### Warden review"));

    let got = status_of(&fx, bad.id).await;
    assert_eq!(got.status, FindingStatus::Posted);
    // Plain notes are not resolvable discussions: stored with the "note:" marker.
    assert!(got
        .forge_note_id
        .as_deref()
        .unwrap()
        .starts_with("note:note-"));
    assert_eq!(status_of(&fx, ok.id).await.status, FindingStatus::Posted);

    let audits = post_audits(&fx).await;
    let fb = audits.iter().find(|p| p["outcome"] == "fallback").unwrap();
    assert_eq!(fb["finding_id"], bad.id);
    assert!(fb["error"].as_str().unwrap().contains("400"));
}

#[tokio::test]
async fn position_422_and_missing_position_fall_back_too() {
    let fx = fixture().await;
    let a = add(&fx, "a", 5, Severity::Major, FindingStatus::Approved).await;
    let no_pos = fx
        .db
        .insert_finding(
            fx.run.id,
            &nf("np", 6, Severity::Minor, 0.9),
            FindingStatus::Approved,
            None,
            "fp-np",
            None,
        )
        .await
        .unwrap();
    let forge = FakeForge {
        reject_lines_422: [5].into(),
        ..Default::default()
    };
    let report = Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(report.posted, 2);
    assert_eq!(report.fallback, 2);
    assert_eq!(forge.line_notes().len(), 1); // no line-note attempt without a position
    assert!(forge
        .notes()
        .iter()
        .any(|n| n.starts_with("**app/api.py:6**")));
    assert_eq!(status_of(&fx, a.id).await.status, FindingStatus::Posted);
    assert_eq!(
        status_of(&fx, no_pos.id).await.status,
        FindingStatus::Posted
    );
}

#[tokio::test]
async fn other_errors_count_as_failed_and_keep_status() {
    let fx = fixture().await;
    let boom = add(&fx, "boom", 13, Severity::Major, FindingStatus::Approved).await;
    let fine = add(&fx, "fine", 14, Severity::Minor, FindingStatus::Approved).await;
    let forge = FakeForge {
        fail_lines: [13].into(),
        ..Default::default()
    };
    let report = Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(
        report,
        PostReport {
            posted: 1,
            fallback: 0,
            failed: 1,
            skipped: 0
        }
    );
    // No file-level fallback for a 500.
    assert_eq!(forge.notes().len(), 1); // summary only
    assert_eq!(
        status_of(&fx, boom.id).await.status,
        FindingStatus::Approved
    );
    assert_eq!(status_of(&fx, fine.id).await.status, FindingStatus::Posted);
    let audits = post_audits(&fx).await;
    let failed = audits.iter().find(|p| p["outcome"] == "failed").unwrap();
    assert_eq!(failed["finding_id"], boom.id);
    assert!(failed["error"].as_str().unwrap().contains("500"));
}

#[tokio::test]
async fn paused_posts_nothing() {
    let fx = fixture().await;
    let a = add(&fx, "a", 10, Severity::Blocker, FindingStatus::Approved).await;
    add(&fx, "b", 11, Severity::Major, FindingStatus::Approved).await;
    fx.db
        .save_settings(&AppSettings {
            posting_paused: true,
            ..Default::default()
        })
        .await
        .unwrap();
    let forge = FakeForge::default();
    let report = Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(
        report,
        PostReport {
            skipped: 2,
            ..Default::default()
        }
    );
    assert!(forge.calls().is_empty());
    assert_eq!(status_of(&fx, a.id).await.status, FindingStatus::Approved);
    let audits = post_audits(&fx).await;
    assert_eq!(audits.len(), 1);
    assert_eq!(audits[0]["outcome"], "paused");
    assert_eq!(audits[0]["finding_ids"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn hourly_budget_keeps_most_severe() {
    let fx = fixture().await;
    // 2 notes already posted this hour (another run of another MR counts too).
    let mut other_mr = fx.mr.clone();
    other_mr.iid = 78;
    let other_mr_id = fx.db.upsert_mr(&other_mr).await.unwrap();
    let other_run = fx
        .db
        .create_run(other_mr_id, "b", "h", None, "opus")
        .await
        .unwrap();
    for i in 0..2 {
        fx.db
            .insert_finding(
                other_run.id,
                &nf(&format!("old{i}"), 1, Severity::Minor, 0.9),
                FindingStatus::Posted,
                None,
                &format!("old{i}"),
                None,
            )
            .await
            .unwrap();
    }

    let nit = add(&fx, "nit", 1, Severity::Nit, FindingStatus::Approved).await;
    let blocker = add(
        &fx,
        "blocker",
        2,
        Severity::Blocker,
        FindingStatus::Approved,
    )
    .await;
    let minor = add(&fx, "minor", 3, Severity::Minor, FindingStatus::Approved).await;
    let major = add(&fx, "major", 4, Severity::Major, FindingStatus::Approved).await;

    let poster = Poster::default().with_posting(PostingConfig {
        max_notes_per_hour: 4,
        ..Default::default()
    });
    let mut rx = poster.events.subscribe();
    let forge = FakeForge::default();
    let report = poster
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(report.posted, 2);
    assert_eq!(report.skipped, 2);
    assert_eq!(
        status_of(&fx, blocker.id).await.status,
        FindingStatus::Posted
    );
    assert_eq!(status_of(&fx, major.id).await.status, FindingStatus::Posted);
    assert_eq!(
        status_of(&fx, minor.id).await.status,
        FindingStatus::Approved
    );
    assert_eq!(status_of(&fx, nit.id).await.status, FindingStatus::Approved);
    assert_eq!(
        post_audits(&fx)
            .await
            .iter()
            .filter(|p| p["outcome"] == "skipped_budget")
            .count(),
        2
    );
    assert!(matches!(rx.try_recv().unwrap(), AppEvent::Notice { level, .. } if level == "warn"));

    // Budget exhausted now: everything skipped, nothing written to the forge.
    let before = forge.calls().len();
    let report = poster
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(
        report,
        PostReport {
            skipped: 2,
            ..Default::default()
        }
    );
    assert_eq!(forge.calls().len(), before);
}

#[tokio::test]
async fn budget_comes_from_project_overrides_when_not_set() {
    let fx = fixture().await;
    fx.db
        .save_project_overrides(
            fx.project.id,
            &json!({"posting": {"max_notes_per_hour": 1, "summary_note": false}}),
        )
        .await
        .unwrap();
    add(&fx, "a", 1, Severity::Major, FindingStatus::Approved).await;
    add(&fx, "b", 2, Severity::Major, FindingStatus::Approved).await;
    let forge = FakeForge::default();
    let report = Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(report.posted, 1);
    assert_eq!(report.skipped, 1);
    assert!(forge.notes().is_empty(), "summary disabled by overrides");
    assert_eq!(forge.labels(), vec!["ai-reviewed"]);
}

#[tokio::test]
async fn duplicate_fingerprint_already_posted_on_mr_is_skipped() {
    let fx = fixture().await;
    let first = add(&fx, "same", 10, Severity::Major, FindingStatus::Approved).await;
    let forge = FakeForge::default();
    Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(status_of(&fx, first.id).await.status, FindingStatus::Posted);

    // New run on the same MR re-finds the same issue.
    let run2 = fx
        .db
        .create_run(fx.mr.id, "base", "h2", Some("h1"), "opus")
        .await
        .unwrap();
    let again = fx
        .db
        .insert_finding(
            run2.id,
            &nf("same", 12, Severity::Major, 0.9),
            FindingStatus::Approved,
            None,
            "fp-same",
            Some(&pos(12)),
        )
        .await
        .unwrap();
    let before = forge.line_notes().len();
    let report = Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &run2, None)
        .await
        .unwrap();
    assert_eq!(
        report,
        PostReport {
            skipped: 1,
            ..Default::default()
        }
    );
    assert_eq!(forge.line_notes().len(), before);
    assert_eq!(
        status_of(&fx, again.id).await.status,
        FindingStatus::Approved
    );
}

#[tokio::test]
async fn summary_note_content() {
    let fx = fixture().await;
    add(
        &fx,
        "minor thing",
        30,
        Severity::Minor,
        FindingStatus::Approved,
    )
    .await;
    add(
        &fx,
        "SQL | injection",
        12,
        Severity::Blocker,
        FindingStatus::Approved,
    )
    .await;
    for (name, status, ms) in [
        ("lint", CiStatus::Passed, 3_200u64),
        ("tests", CiStatus::Failed, 125_000),
    ] {
        fx.db
            .insert_ci_job(&CiJob {
                run_id: fx.run.id,
                name: name.into(),
                kind: "recipe".into(),
                status,
                duration_ms: ms,
                ..Default::default()
            })
            .await
            .unwrap();
    }
    let forge = FakeForge::default();
    Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    let notes = forge.notes();
    assert_eq!(notes.len(), 1);
    let expected = "\
### Warden review: changes requested

Two blocking issues in the new linker endpoint.

**Findings (2)**

| Severity | File | Title |
|---|---|---|
| 🔴 Blocker | `app/api.py:12` | SQL \\| injection |
| 🟡 Minor | `app/api.py:30` | minor thing |

**Local CI**

| Job | Status | Duration |
|---|---|---|
| lint | ✅ passed | 3.2s |
| tests | ❌ failed | 2m 05s |

---
Reviewed by Warden with Claude Code · head 01234567";
    assert_eq!(notes[0], expected);
}

#[tokio::test]
async fn summary_posted_for_failed_ci_without_findings_once() {
    let fx = fixture().await;
    fx.db
        .insert_ci_job(&CiJob {
            run_id: fx.run.id,
            name: "tests".into(),
            status: CiStatus::Failed,
            ..Default::default()
        })
        .await
        .unwrap();
    let forge = FakeForge::default();
    let report = Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(report, PostReport::default());
    let notes = forge.notes();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("No findings posted."));
    assert!(notes[0].contains("| tests | ❌ failed | 0ms |"));
    assert_eq!(forge.labels(), vec!["ai-reviewed"]);

    // Calling again does not repeat the CI-only summary.
    Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(forge.notes().len(), 1);
}

#[tokio::test]
async fn no_summary_when_nothing_posted_and_ci_green() {
    let fx = fixture().await;
    fx.db
        .insert_ci_job(&CiJob {
            run_id: fx.run.id,
            name: "lint".into(),
            status: CiStatus::Passed,
            ..Default::default()
        })
        .await
        .unwrap();
    let forge = FakeForge::default();
    let report = Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(report, PostReport::default());
    assert!(forge.calls().is_empty());
}

#[tokio::test]
async fn summary_and_label_errors_do_not_fail_the_post() {
    let fx = fixture().await;
    let a = add(&fx, "a", 10, Severity::Major, FindingStatus::Approved).await;
    let forge = FakeForge {
        fail_notes: true,
        fail_label: true,
        ..Default::default()
    };
    let report = Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    assert_eq!(report.posted, 1);
    assert_eq!(report.failed, 0);
    assert_eq!(status_of(&fx, a.id).await.status, FindingStatus::Posted);
    let audits = post_audits(&fx).await;
    assert!(audits
        .iter()
        .any(|p| p["kind"] == "summary" && p["outcome"] == "failed"));
    assert!(audits
        .iter()
        .any(|p| p["kind"] == "label" && p["outcome"] == "label_failed"));
}

// ---------------------------------------------------------------------------------------------
// auto_resolve
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn auto_resolve_resolves_fixed_posted_findings() {
    let fx = fixture().await;
    let fixed = add(&fx, "fixed", 10, Severity::Major, FindingStatus::Approved).await;
    let still = add(&fx, "still", 11, Severity::Major, FindingStatus::Approved).await;
    let broken = add(&fx, "broken", 12, Severity::Major, FindingStatus::Approved).await;
    let draft = add(&fx, "draftfix", 13, Severity::Major, FindingStatus::Draft).await;
    let forge = FakeForge::default();
    Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    let fixed_note = status_of(&fx, fixed.id).await.forge_note_id.unwrap();
    let broken_note = status_of(&fx, broken.id).await.forge_note_id.unwrap();

    let forge2 = FakeForge {
        fail_resolve: [broken_note.clone()].into(),
        ..Default::default()
    };
    let n = Poster::default()
        .auto_resolve(
            &fx.db,
            &forge2,
            &fx.project,
            &fx.mr,
            &[
                fixed.fingerprint.clone(),
                broken.fingerprint.clone(),
                draft.fingerprint.clone(),
                "fp-unknown".into(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert_eq!(
        forge2.calls(),
        vec![
            Call::Resolve {
                iid: 77,
                discussion: fixed_note.clone()
            },
            Call::Resolve {
                iid: 77,
                discussion: broken_note
            },
        ]
    );
    let got = status_of(&fx, fixed.id).await;
    assert_eq!(got.status, FindingStatus::Resolved);
    assert_eq!(got.forge_note_id.as_deref(), Some(fixed_note.as_str()));
    assert_eq!(status_of(&fx, still.id).await.status, FindingStatus::Posted);
    assert_eq!(
        status_of(&fx, broken.id).await.status,
        FindingStatus::Posted
    );
    assert_eq!(status_of(&fx, draft.id).await.status, FindingStatus::Draft);

    let audits = post_audits(&fx).await;
    assert!(audits.iter().any(|p| p["kind"] == "resolve"
        && p["outcome"] == "resolved"
        && p["finding_id"] == fixed.id));
    assert!(audits
        .iter()
        .any(|p| p["kind"] == "resolve" && p["outcome"] == "failed"));

    // Already resolved: nothing to do.
    let forge3 = FakeForge::default();
    let n = Poster::default()
        .auto_resolve(
            &fx.db,
            &forge3,
            &fx.project,
            &fx.mr,
            std::slice::from_ref(&fixed.fingerprint),
        )
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert!(forge3.calls().is_empty());
}

#[tokio::test]
async fn auto_resolve_respects_pause_and_empty_input() {
    let fx = fixture().await;
    let f = add(&fx, "a", 10, Severity::Major, FindingStatus::Approved).await;
    let forge = FakeForge::default();
    Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    let forge2 = FakeForge::default();
    assert_eq!(
        Poster::default()
            .auto_resolve(&fx.db, &forge2, &fx.project, &fx.mr, &[])
            .await
            .unwrap(),
        0
    );
    fx.db
        .save_settings(&AppSettings {
            posting_paused: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        Poster::default()
            .auto_resolve(
                &fx.db,
                &forge2,
                &fx.project,
                &fx.mr,
                std::slice::from_ref(&f.fingerprint)
            )
            .await
            .unwrap(),
        0
    );
    assert!(forge2.calls().is_empty());
    assert_eq!(status_of(&fx, f.id).await.status, FindingStatus::Posted);
}

// ---------------------------------------------------------------------------------------------
// format_note / format_summary snapshots
// ---------------------------------------------------------------------------------------------

#[test]
fn format_note_single_line_suggestion_snapshot() {
    let f = finding_for_format();
    let expected = "\
🔴 Blocker · **SQL built with an f-string from request input**

`name` comes straight from the request and is interpolated into the query.

```suggestion:-0+0
frappe.db.sql(\"select name from tabLead where name=%s\", (name,))
```

— Warden · rule FR-002 · confidence 0.98";
    assert_eq!(Poster::format_note(&f), expected);
}

#[test]
fn format_note_multi_line_suggestion_uses_plain_block() {
    let mut f = finding_for_format();
    f.severity = Severity::Major;
    f.rule_id = None;
    f.confidence = 0.912;
    f.suggestion = Some(
        "if not frappe.has_permission(\"Lead\", \"write\"):\n    frappe.throw(\"Not permitted\")\n"
            .into(),
    );
    let expected = "\
🟠 Major · **SQL built with an f-string from request input**

`name` comes straight from the request and is interpolated into the query.

```
if not frappe.has_permission(\"Lead\", \"write\"):
    frappe.throw(\"Not permitted\")
```

— Warden · confidence 0.91";
    assert_eq!(Poster::format_note(&f), expected);
}

#[test]
fn format_note_badges_and_no_suggestion() {
    let mut f = finding_for_format();
    f.suggestion = None;
    for (sev, badge) in [
        (Severity::Blocker, "🔴 Blocker"),
        (Severity::Major, "🟠 Major"),
        (Severity::Minor, "🟡 Minor"),
        (Severity::Nit, "⚪ Nit"),
    ] {
        f.severity = sev;
        let note = Poster::format_note(&f);
        assert!(note.starts_with(&format!("{badge} · **")), "{note}");
        assert!(!note.contains("```"));
    }
    f.severity = Severity::Minor;
    f.suggestion = Some("   \n".into());
    assert!(!Poster::format_note(&f).contains("```"));
}

#[test]
fn format_note_old_side_and_backticks() {
    let mut f = finding_for_format();
    f.side = "old".into();
    let note = Poster::format_note(&f);
    assert!(!note.contains("suggestion:-0+0"));
    f.side = "new".into();
    f.suggestion = Some("x = \"```\"".into());
    let note = Poster::format_note(&f);
    assert!(
        note.contains("````suggestion:-0+0\nx = \"```\"\n````"),
        "{note}"
    );
}

#[test]
fn format_summary_without_findings_or_ci() {
    let run = ReviewRun {
        head_sha: "abc".into(),
        verdict: Some("approve".into()),
        ..Default::default()
    };
    assert_eq!(
        Poster::format_summary(&run, &[], &[]),
        "### Warden review: no blocking issues\n\nNo findings posted.\n\n---\nReviewed by Warden with Claude Code · head abc"
    );
    let mut f = finding_for_format();
    f.severity = Severity::Minor;
    let run = ReviewRun {
        head_sha: "0123456789".into(),
        ..Default::default()
    };
    let s = Poster::format_summary(&run, &[f], &[]);
    assert!(s.starts_with("### Warden review: comments\n\n**Findings (1)**"));
    assert!(s.ends_with("head 01234567"));
    assert!(!s.contains("Local CI"));
}

// ---------------------------------------------------------------------------------------------
// Security regressions (fix pass)
// ---------------------------------------------------------------------------------------------

const LEAKED: &str = "glpat-AbCdEf1234567890xYz_q";

#[tokio::test]
async fn posted_notes_and_summary_are_redacted() {
    let fx = fixture().await;
    // A prompt-injected review tries to exfiltrate a token through the finding and the summary.
    let mut leak = nf("leak", 10, Severity::Blocker, 0.99);
    leak.title = format!("Token {LEAKED} found");
    leak.body = format!("The glab config contains token: {LEAKED} which grants api scope.");
    leak.suggestion = Some(format!("GITLAB_TOKEN = \"{LEAKED}\""));
    fx.db
        .insert_finding(
            fx.run.id,
            &leak,
            FindingStatus::Approved,
            None,
            "fp-leak",
            Some(&pos(10)),
        )
        .await
        .unwrap();
    let mut fallback = nf("leak2", 99, Severity::Major, 0.99);
    fallback.body = format!("Also see {LEAKED} in ~/.git-credentials, please rotate it.");
    fx.db
        .insert_finding(
            fx.run.id,
            &fallback,
            FindingStatus::Approved,
            None,
            "fp-leak2",
            Some(&pos(99)),
        )
        .await
        .unwrap();
    let mut run = fx.run.clone();
    run.summary = Some(format!("Summary quoting {LEAKED}"));
    fx.db.update_run(&run).await.unwrap();
    let forge = FakeForge {
        reject_lines: [99].into(),
        ..Default::default()
    };
    let report = Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &run, None)
        .await
        .unwrap();
    assert_eq!(report.posted, 2);
    let mut bodies = forge.line_notes();
    bodies.extend(forge.notes());
    assert!(bodies.len() >= 3, "{bodies:?}");
    for b in &bodies {
        assert!(!b.contains(LEAKED), "secret reached the forge: {b}");
    }
}

#[tokio::test]
async fn plain_note_fallback_is_resolved_locally_without_forge_call() {
    let fx = fixture().await;
    let bad = add(&fx, "bad", 99, Severity::Major, FindingStatus::Approved).await;
    let forge = FakeForge {
        reject_lines: [99].into(),
        ..Default::default()
    };
    Poster::default()
        .post_run(&fx.db, &forge, &fx.project, &fx.mr, &fx.run, None)
        .await
        .unwrap();
    let note = status_of(&fx, bad.id).await.forge_note_id.unwrap();
    assert!(note.starts_with("note:"), "{note}");

    let forge2 = FakeForge::default();
    let n = Poster::default()
        .auto_resolve(
            &fx.db,
            &forge2,
            &fx.project,
            &fx.mr,
            std::slice::from_ref(&bad.fingerprint),
        )
        .await
        .unwrap();
    assert_eq!(n, 1);
    assert!(forge2.calls().is_empty(), "{:?}", forge2.calls());
    assert_eq!(status_of(&fx, bad.id).await.status, FindingStatus::Resolved);
}
