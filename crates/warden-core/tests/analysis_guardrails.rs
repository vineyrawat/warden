//! guardrails.rs: every rejection reason, auto_post policy, positions, budget, fingerprint, tone.

#[path = "analysis_fixtures.rs"]
mod fixtures;

use std::collections::HashSet;

use fixtures::*;
use pretty_assertions::assert_eq;
use sha2::{Digest, Sha256};
use warden_core::checks::run_checks_with;
use warden_core::config::WardenConfig;
use warden_core::diff::{parse_unified, ChangedIndex};
use warden_core::guardrails::{
    apply_budget, auto_post_for, fingerprint, fingerprint_for, is_near_duplicate, similarity,
    tokens, tone_check, tone_check_with, validate_new_finding, GuardrailInput, Verdict, DUPLICATE,
    OVER_BUDGET,
};
use warden_core::types::{DiffPosition, Finding, FindingStatus, NewFinding, PostingMode, Severity};

const SQL_BODY: &str = "import_name and each r come straight from the request, so this query is \
    injectable. Use frappe.db.set_value or a parameterised query instead.";

/// The FR-002 finding from the docs/plan.html mockup.
fn sql_finding() -> NewFinding {
    NewFinding {
        file: IMPORT_LINKS.into(),
        line: 16,
        side: "new".into(),
        severity: Severity::Blocker,
        category: "security".into(),
        rule_id: Some("FR-002".into()),
        title: "SQL built with an f-string from request input".into(),
        body: SQL_BODY.into(),
        suggestion: Some(
            "frappe.db.set_value(\"ECRM Lead\", r, \"import_ref\", import_name)".into(),
        ),
        evidence: Some("rows = json.loads(rows) on line 14; no sanitising before line 16".into()),
        confidence: 0.98,
        source: "claude".into(),
    }
}

struct Ctx {
    cfg: WardenConfig,
    index: ChangedIndex,
    fps: HashSet<String>,
    notes: Vec<String>,
}

impl Ctx {
    fn new() -> Self {
        Ctx {
            cfg: WardenConfig::default(),
            index: ChangedIndex::from_files(&parse_unified(MR_DIFF)),
            fps: HashSet::new(),
            notes: Vec::new(),
        }
    }

    fn check(&self, f: &NewFinding) -> Verdict {
        validate_new_finding(
            f,
            &GuardrailInput {
                cfg: &self.cfg,
                index: &self.index,
                existing_fingerprints: &self.fps,
                existing_note_bodies: &self.notes,
                shas: SHAS,
            },
        )
    }

    fn reason(&self, f: &NewFinding) -> String {
        let v = self.check(f);
        assert!(!v.keep, "expected a rejection for {f:?}");
        assert!(!v.auto_post);
        assert!(v.position.is_none());
        v.reason.expect("rejections carry a reason")
    }
}

fn with(f: impl FnOnce(&mut NewFinding)) -> NewFinding {
    let mut n = sql_finding();
    f(&mut n);
    n
}

// ---------------------------------------------------------------------------------------------
// Accepted findings
// ---------------------------------------------------------------------------------------------

#[test]
fn valid_finding_is_kept_with_position() {
    let ctx = Ctx::new();
    let v = ctx.check(&sql_finding());
    assert_eq!(
        v,
        Verdict {
            keep: true,
            reason: None,
            auto_post: false, // default mode is Draft
            position: Some(DiffPosition {
                base_sha: SHAS.0.into(),
                start_sha: SHAS.1.into(),
                head_sha: SHAS.2.into(),
                old_path: IMPORT_LINKS.into(),
                new_path: IMPORT_LINKS.into(),
                old_line: None,
                new_line: Some(16),
            }),
        }
    );
}

#[test]
fn deleted_line_on_old_side_is_kept_with_old_line_position() {
    let ctx = Ctx::new();
    let f = with(|f| {
        f.side = "old".into();
        f.line = 11;
        f.title = "Removed comment documented the legacy linker".into();
        f.severity = Severity::Minor;
    });
    let v = ctx.check(&f);
    assert!(v.keep, "{v:?}");
    let p = v.position.unwrap();
    assert_eq!((p.old_line, p.new_line), (Some(11), None));
}

#[test]
fn renamed_file_old_path_and_dot_slash_paths_resolve() {
    let ctx = Ctx::new();
    let f = with(|f| {
        f.file = format!("./{NEW_NAME}");
        f.line = 2;
    });
    let v = ctx.check(&f);
    assert!(v.keep, "{v:?}");
    let p = v.position.unwrap();
    assert_eq!(
        (p.old_path.as_str(), p.new_path.as_str()),
        (OLD_NAME, NEW_NAME)
    );
}

// ---------------------------------------------------------------------------------------------
// Rejections: schema
// ---------------------------------------------------------------------------------------------

#[test]
fn rejects_missing_required_fields() {
    let ctx = Ctx::new();
    assert_eq!(
        ctx.reason(&with(|f| f.file = "  ".into())),
        "missing required field: file"
    );
    assert_eq!(
        ctx.reason(&with(|f| f.title = String::new())),
        "missing required field: title"
    );
    assert_eq!(
        ctx.reason(&with(|f| f.body = "\n".into())),
        "missing required field: body"
    );
    assert_eq!(
        ctx.reason(&with(|f| f.category = String::new())),
        "missing required field: category"
    );
}

#[test]
fn rejects_bad_side_and_line() {
    let ctx = Ctx::new();
    assert!(ctx
        .reason(&with(|f| f.side = "left-ish".into()))
        .contains("invalid side"));
    assert!(ctx
        .reason(&with(|f| f.side = String::new()))
        .contains("invalid side"));
    assert_eq!(
        ctx.reason(&with(|f| f.line = 0)),
        "line must be 1 or greater"
    );
    // side is case-insensitive
    assert!(ctx.check(&with(|f| f.side = "NEW".into())).keep);
}

#[test]
fn rejects_confidence_out_of_range() {
    let ctx = Ctx::new();
    for c in [1.5, -0.1, f32::NAN, f32::INFINITY] {
        let r = ctx.reason(&with(|f| f.confidence = c));
        assert!(r.contains("confidence must be between 0 and 1"), "{c}: {r}");
    }
    assert!(ctx.check(&with(|f| f.confidence = 1.0)).keep);
}

#[test]
fn rejects_findings_without_rule_or_evidence() {
    let ctx = Ctx::new();
    let r = ctx.reason(&with(|f| {
        f.rule_id = None;
        f.evidence = None;
    }));
    assert!(r.contains("needs a rule_id or evidence"), "{r}");
    let r = ctx.reason(&with(|f| {
        f.rule_id = Some("  ".into());
        f.evidence = Some(String::new());
    }));
    assert!(r.contains("needs a rule_id or evidence"), "{r}");
    // either one is enough
    assert!(ctx.check(&with(|f| f.rule_id = None)).keep);
    assert!(ctx.check(&with(|f| f.evidence = None)).keep);
}

// ---------------------------------------------------------------------------------------------
// Rejections: tone
// ---------------------------------------------------------------------------------------------

#[test]
fn rejects_bad_tone() {
    let ctx = Ctx::new();
    let r = ctx.reason(&with(|f| {
        f.body = "LGTM, ship it whenever you are ready to go".into()
    }));
    assert!(r.contains("LGTM"), "{r}");
    let r = ctx.reason(&with(|f| f.body = "Fix this query.".into()));
    assert!(r.contains("too short"), "{r}");
    let r = ctx.reason(&with(|f| {
        f.body = "Nice work on this endpoint, looks good to me!".into()
    }));
    assert!(r.contains("praise"), "{r}");
    let r = ctx.reason(&with(|f| {
        f.body = "This might be a problem for some users of the endpoint.".into();
        f.evidence = None;
    }));
    assert!(r.contains("hedged"), "{r}");
}

#[test]
fn hedging_is_fine_with_evidence() {
    let ctx = Ctx::new();
    let hedged =
        "This might let users relink imports they cannot read, since nothing checks access.";
    // evidence field present
    assert!(ctx.check(&with(|f| f.body = hedged.into())).keep);
    // evidence inside the body (inline code)
    let in_body =
        "Maybe guard this: `link_import_rows` never calls check_permission before writing.";
    assert!(
        ctx.check(&with(|f| {
            f.body = in_body.into();
            f.evidence = None;
        }))
        .keep
    );
}

#[test]
fn tone_check_directly() {
    assert_eq!(tone_check(""), Some("body is empty".into()));
    assert!(tone_check("lgtm").unwrap().contains("LGTM"));
    assert!(tone_check("Looks good to me").is_some());
    assert!(tone_check("short body").unwrap().contains("too short"));
    assert!(tone_check("Great job!! Awesome change here.")
        .unwrap()
        .contains("praise"));
    assert!(
        tone_check("Perhaps this is slow when there are many rows in the table.")
            .unwrap()
            .contains("hedged")
    );
    assert!(tone_check("Possibly wrong but I am not sure what this function does.").is_some());
    assert_eq!(
        tone_check("Perhaps this is slow: line 42 runs a query per row inside the loop."),
        None
    );
    assert_eq!(
        tone_check_with(
            "This might be slow for imports with thousands of rows.",
            true
        ),
        None
    );
    assert_eq!(tone_check(SQL_BODY), None);
    // long, concrete bodies that mention praise in passing are fine
    assert_eq!(
        tone_check("Looks good overall, but the f-string SQL on this line is injectable because import_name comes from the request and is never escaped."),
        None
    );
    // words that merely contain a hedge are not hedges
    assert_eq!(
        tone_check("The mighty loop recomputes totals for every row of the import table."),
        None
    );
}

// ---------------------------------------------------------------------------------------------
// Rejections: anchor
// ---------------------------------------------------------------------------------------------

#[test]
fn rejects_files_not_in_diff() {
    let ctx = Ctx::new();
    let r = ctx.reason(&with(|f| f.file = "entries_crm/api/other.py".into()));
    assert!(
        r.starts_with("file not in diff: entries_crm/api/other.py"),
        "{r}"
    );
}

#[test]
fn rejects_unchanged_lines_on_new_side() {
    let ctx = Ctx::new();
    let r = ctx.reason(&with(|f| f.line = 10));
    assert!(r.contains("unchanged context line"), "{r}");
    let r = ctx.reason(&with(|f| f.line = 30));
    assert!(r.contains("outside the diff"), "{r}");
    // binary file has no lines at all
    let r = ctx.reason(&with(|f| {
        f.file = "ui/public/logo.png".into();
        f.line = 1;
    }));
    assert!(r.contains("outside the diff"), "{r}");
}

#[test]
fn rejects_old_side_lines_that_were_not_deleted() {
    let ctx = Ctx::new();
    let r = ctx.reason(&with(|f| {
        f.side = "old".into();
        f.line = 12; // context
    }));
    assert!(r.contains("is not a deleted line"), "{r}");
    let r = ctx.reason(&with(|f| {
        f.side = "old".into();
        f.line = 16; // added on the new side, nothing deleted there
    }));
    assert!(r.contains("is not a deleted line"), "{r}");
}

// ---------------------------------------------------------------------------------------------
// Rejections: severity / nits / confidence
// ---------------------------------------------------------------------------------------------

#[test]
fn rejects_nits_when_disabled() {
    let mut ctx = Ctx::new();
    let nit = with(|f| f.severity = Severity::Nit);
    assert_eq!(ctx.reason(&nit), "nits are disabled for this project");
    ctx.cfg.review.nits = true;
    // nits on, but the default floor (minor) still drops them
    assert!(ctx.reason(&nit).contains("below the floor"));
    ctx.cfg.review.severity_floor = Severity::Nit;
    assert!(ctx.check(&nit).keep);
}

#[test]
fn rejects_below_severity_floor() {
    let mut ctx = Ctx::new();
    ctx.cfg.review.severity_floor = Severity::Major;
    let r = ctx.reason(&with(|f| f.severity = Severity::Minor));
    assert_eq!(r, "severity minor is below the floor (major)");
    assert!(ctx.check(&with(|f| f.severity = Severity::Major)).keep);
    assert!(ctx.check(&with(|f| f.severity = Severity::Blocker)).keep);
}

#[test]
fn rejects_low_confidence_except_rule_findings() {
    let ctx = Ctx::new();
    let r = ctx.reason(&with(|f| f.confidence = 0.62));
    assert_eq!(r, "confidence 0.62 is below the threshold 0.70");
    assert!(
        ctx.check(&with(|f| f.confidence = 0.7)).keep,
        "threshold is inclusive"
    );
    // deterministic rule findings are exempt from min_confidence
    assert!(
        ctx.check(&with(|f| {
            f.confidence = 0.3;
            f.source = "rule".into();
        }))
        .keep
    );
    // ...but CI findings are not
    assert!(
        !ctx.check(&with(|f| {
            f.confidence = 0.3;
            f.source = "ci".into();
        }))
        .keep
    );
}

// ---------------------------------------------------------------------------------------------
// Rejections: duplicates
// ---------------------------------------------------------------------------------------------

#[test]
fn rejects_exact_duplicate_fingerprints() {
    let mut ctx = Ctx::new();
    let f = sql_finding();
    ctx.fps
        .insert(fingerprint(&f.file, &f.title, f.rule_id.as_deref()));
    assert_eq!(ctx.reason(&f), DUPLICATE);
    assert_eq!(DUPLICATE, "duplicate of existing thread");
    // title normalisation: case/whitespace differences are still duplicates; line is irrelevant
    let g = with(|g| {
        g.title = "  sql BUILT with an   f-string from request input ".into();
        g.line = 17;
    });
    assert_eq!(ctx.reason(&g), DUPLICATE);
    // a different rule id is a different thread
    assert!(ctx.check(&with(|g| g.rule_id = Some("FR-009".into()))).keep);
    // fingerprint of a ./-prefixed path matches the stored one
    assert_eq!(
        ctx.reason(&with(|g| g.file = format!("./{IMPORT_LINKS}"))),
        DUPLICATE
    );
}

#[test]
fn rejects_near_duplicate_bodies() {
    let mut ctx = Ctx::new();
    ctx.notes = vec![
        "Please add tests for the importer.".into(),
        // a human already said the same thing, with slightly different punctuation/case
        "import_name and each r come straight from the request so this query is injectable. \
         Use frappe.db.set_value or a parameterised query."
            .into(),
    ];
    let r = ctx.reason(&sql_finding());
    assert_eq!(r, "near-duplicate of an existing discussion note");
}

#[test]
fn near_duplicate_catches_previously_posted_warden_notes() {
    let mut ctx = Ctx::new();
    // A Warden note wraps the body in a badge, title and footer: Jaccard drops, containment does not.
    ctx.notes = vec![format!(
        "**🔴 Blocker** · **SQL built with an f-string from request input**\n\n{SQL_BODY}\n\n\
         ```suggestion\nfrappe.db.set_value(\"ECRM Lead\", r, \"import_ref\", import_name)\n```\n\n\
         — Warden · rule FR-002 · confidence 0.98"
    )];
    assert_eq!(
        ctx.reason(&sql_finding()),
        "near-duplicate of an existing discussion note"
    );
}

#[test]
fn unrelated_notes_do_not_block() {
    let mut ctx = Ctx::new();
    ctx.notes = vec![
        "Can we rename this endpoint to something clearer?".into(),
        "".into(),
        "The frontend pagination resets the row state when changing pages.".into(),
    ];
    assert!(ctx.check(&sql_finding()).keep);
}

#[test]
fn similarity_helpers() {
    assert!(similarity("alpha beta gamma", "Alpha, beta; GAMMA!") > 0.99);
    assert_eq!(
        similarity("a b c", "a b c"),
        0.0,
        "single-character tokens are ignored"
    );
    assert!(similarity("alpha beta", "gamma delta") < 0.01);
    let a = tokens("one two three four five six seven eight nine");
    let b = tokens("one two three four five six seven eight nine ten eleven twelve thirteen");
    assert!(is_near_duplicate(&a, &b), "containment");
    let short_a = tokens("fix this");
    let short_b = tokens("fix this please now and also that other thing there");
    assert!(
        !is_near_duplicate(&short_a, &short_b),
        "tiny sets need jaccard"
    );
    assert!(!is_near_duplicate(&tokens(""), &tokens("x y")));
}

// ---------------------------------------------------------------------------------------------
// auto_post policy
// ---------------------------------------------------------------------------------------------

#[test]
fn auto_post_follows_mode_and_policy() {
    let mut ctx = Ctx::new();
    assert!(
        !ctx.check(&sql_finding()).auto_post,
        "draft never auto-posts"
    );

    ctx.cfg.posting.mode = PostingMode::Assist;
    assert!(ctx.check(&sql_finding()).auto_post, "blocker 0.98");
    assert!(
        !ctx.check(&with(|f| f.confidence = 0.8)).auto_post,
        "below auto_post.min_confidence 0.85"
    );
    assert!(
        ctx.check(&with(|f| f.confidence = 0.85)).auto_post,
        "inclusive"
    );
    assert!(ctx.check(&with(|f| f.severity = Severity::Major)).auto_post);
    let minor = ctx.check(&with(|f| f.severity = Severity::Minor));
    assert!(
        minor.keep && !minor.auto_post,
        "minor is not in auto_post.severities"
    );

    ctx.cfg.posting.mode = PostingMode::Auto;
    let v = ctx.check(&with(|f| {
        f.severity = Severity::Minor;
        f.confidence = 0.71;
    }));
    assert!(v.keep && v.auto_post, "auto posts everything that is kept");

    assert!(!auto_post_for(
        &WardenConfig::default(),
        Severity::Blocker,
        1.0
    ));
}

// ---------------------------------------------------------------------------------------------
// Check findings flow through guardrails
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn deterministic_check_findings_pass_the_guardrails() {
    let ctx = Ctx::new();
    let dir = tempfile::tempdir().unwrap();
    let files = parse_unified(MR_DIFF);
    let report = run_checks_with(dir.path(), &files, &ctx.cfg, None, None, vec![])
        .await
        .unwrap();
    assert_eq!(report.findings.len(), 3);
    for f in &report.findings {
        let v = ctx.check(f);
        assert!(v.keep, "{f:?} -> {v:?}");
        assert_eq!(v.position.as_ref().unwrap().new_line, Some(f.line));
    }
}

// ---------------------------------------------------------------------------------------------
// apply_budget
// ---------------------------------------------------------------------------------------------

fn finding(id: i64, severity: Severity, confidence: f32, status: FindingStatus) -> Finding {
    Finding {
        id,
        run_id: 1,
        file: "a.py".into(),
        line: id as u32,
        side: "new".into(),
        severity,
        category: "correctness".into(),
        rule_id: None,
        title: format!("finding {id}"),
        body: "body text long enough".into(),
        suggestion: None,
        evidence: Some("e".into()),
        confidence,
        source: "claude".into(),
        status,
        guardrail_reason: None,
        fingerprint: format!("fp{id}"),
        forge_note_id: None,
        position: None,
    }
}

fn statuses(v: &[Finding]) -> Vec<(i64, FindingStatus)> {
    v.iter().map(|f| (f.id, f.status)).collect()
}

#[test]
fn budget_keeps_top_n_by_severity_then_confidence() {
    use FindingStatus::*;
    let mut v = vec![
        finding(1, Severity::Minor, 0.99, Draft),
        finding(2, Severity::Blocker, 0.80, Draft),
        finding(3, Severity::Major, 0.95, Draft),
        finding(4, Severity::Major, 0.75, Draft),
        finding(5, Severity::Blocker, 0.90, Draft),
    ];
    apply_budget(&mut v, 3);
    assert_eq!(
        statuses(&v),
        vec![
            (1, Rejected),
            (2, Draft),
            (3, Draft),
            (4, Rejected),
            (5, Draft)
        ],
        "order of the vec is unchanged"
    );
    assert_eq!(v[0].guardrail_reason.as_deref(), Some(OVER_BUDGET));
    assert_eq!(v[3].guardrail_reason.as_deref(), Some("over note budget"));
    assert_eq!(v[1].guardrail_reason, None);
}

#[test]
fn budget_ignores_rejected_and_dismissed_and_counts_posted() {
    use FindingStatus::*;
    let mut v = vec![
        finding(1, Severity::Blocker, 0.99, Rejected),
        finding(2, Severity::Blocker, 0.99, Dismissed),
        finding(3, Severity::Minor, 0.80, Posted),
        finding(4, Severity::Major, 0.90, Approved),
        finding(5, Severity::Major, 0.85, Draft),
    ];
    v[0].guardrail_reason = Some("nits are disabled".into());
    apply_budget(&mut v, 2);
    assert_eq!(
        statuses(&v),
        vec![
            (1, Rejected),
            (2, Dismissed),
            (3, Posted),
            (4, Approved),
            (5, Rejected)
        ]
    );
    assert_eq!(
        v[0].guardrail_reason.as_deref(),
        Some("nits are disabled"),
        "existing reason untouched"
    );
    assert_eq!(v[4].guardrail_reason.as_deref(), Some(OVER_BUDGET));
}

#[test]
fn budget_ties_keep_original_order_and_edge_cases() {
    use FindingStatus::*;
    let mut v = vec![
        finding(1, Severity::Major, 0.9, Draft),
        finding(2, Severity::Major, 0.9, Draft),
        finding(3, Severity::Major, 0.9, Draft),
    ];
    apply_budget(&mut v, 2);
    assert_eq!(statuses(&v), vec![(1, Draft), (2, Draft), (3, Rejected)]);

    let mut v = vec![finding(1, Severity::Major, 0.9, Draft)];
    apply_budget(&mut v, 10);
    assert_eq!(statuses(&v), vec![(1, Draft)]);

    let mut v = vec![finding(1, Severity::Major, 0.9, Draft)];
    apply_budget(&mut v, 0);
    assert_eq!(statuses(&v), vec![(1, Rejected)]);

    let mut empty: Vec<Finding> = vec![];
    apply_budget(&mut empty, 3);
    assert!(empty.is_empty());
}

// ---------------------------------------------------------------------------------------------
// fingerprint
// ---------------------------------------------------------------------------------------------

#[test]
fn fingerprint_matches_spec_formula() {
    let expected = hex_sha256("entries_crm/api/import_links.py:sql built with an f-string:FR-002");
    assert_eq!(
        fingerprint(
            IMPORT_LINKS,
            "  SQL built with\tan  F-string ",
            Some("FR-002")
        ),
        expected
    );
    assert_eq!(
        fingerprint("a.py", "Title", None),
        hex_sha256("a.py:title:")
    );
    assert_eq!(fingerprint("a.py", "Title", None).len(), 64);
    assert_ne!(
        fingerprint("a.py", "Title", None),
        fingerprint("b.py", "Title", None)
    );
    assert_ne!(
        fingerprint("a.py", "Title", Some("X")),
        fingerprint("a.py", "Title", None)
    );

    let idx = ChangedIndex::from_files(&parse_unified(MR_DIFF));
    let f = with(|f| f.file = format!("./{IMPORT_LINKS}"));
    assert_eq!(
        fingerprint_for(&f, &idx),
        fingerprint(IMPORT_LINKS, &f.title, f.rule_id.as_deref())
    );
}

fn hex_sha256(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex::encode(h.finalize())
}
