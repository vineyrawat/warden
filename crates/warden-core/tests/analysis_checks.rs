//! checks: regex rules over added lines, semgrep / gitleaks (fake binaries), secret scanning and
//! redaction, tool detection.

#[path = "analysis_fixtures.rs"]
mod fixtures;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fixtures::*;
use pretty_assertions::assert_eq;
use warden_core::checks::rules::{
    parse_semgrep_json, regex_rule_findings, run_regex_rules, run_semgrep_with, REGEX_CONFIDENCE,
};
use warden_core::checks::secrets_scan::{
    parse_gitleaks_report, run_gitleaks, scan_added_lines, secret_hits,
};
use warden_core::checks::{
    detect_tool, detect_tools, parse_version, redact_secrets, run_checks, run_checks_with,
    tool_version, KNOWN_TOOLS,
};
use warden_core::config::{default_rules, RuleConfig, WardenConfig};
use warden_core::diff::{parse_unified, ChangedIndex, FileDiff};
use warden_core::types::{NewFinding, Severity, ToolStatus};

fn mr_files() -> Vec<FileDiff> {
    parse_unified(MR_DIFF)
}

fn rule(id: &str, pattern: &str, applies_to: Option<&str>) -> RuleConfig {
    RuleConfig {
        id: id.into(),
        title: format!("Rule {id}"),
        applies_to: applies_to.map(str::to_string),
        instruction: None,
        pattern: Some(pattern.into()),
        semgrep: None,
        severity: Severity::Minor,
    }
}

/// Writes an executable `sh` script into `dir`.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut perm = std::fs::metadata(&p).unwrap().permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&p, perm).unwrap();
    p
}

// ---------------------------------------------------------------------------------------------
// Regex rules
// ---------------------------------------------------------------------------------------------

#[test]
fn builtin_rules_hit_only_added_lines() {
    let findings = run_regex_rules(&mr_files(), &default_rules()).unwrap();
    let summary: Vec<(&str, u32, &str)> = findings
        .iter()
        .map(|f| (f.file.as_str(), f.line, f.rule_id.as_deref().unwrap()))
        .collect();
    // The deleted legacy file also has f-string SQL, and the TSX file has it in an added comment:
    // neither may be flagged (deleted lines / applies_to "**/*.py").
    assert_eq!(
        summary,
        vec![(IMPORT_LINKS, 16, "FR-002"), (IMPORT_LINKS, 17, "FR-007")]
    );

    let sql = &findings[0];
    assert_eq!(sql.severity, Severity::Blocker);
    assert_eq!(sql.side, "new");
    assert_eq!(sql.source, "rule");
    assert_eq!(sql.category, "security");
    assert_eq!(sql.title, "No string-formatted SQL");
    assert!(sql.confidence >= 0.95);
    assert_eq!(sql.confidence, REGEX_CONFIDENCE);
    assert!(sql.body.starts_with("Line 16 "), "{}", sql.body);
    assert!(sql.body.len() >= 20);
    let ev = sql.evidence.as_deref().unwrap();
    assert!(
        ev.contains("frappe.db.sql(f\"update `tabECRM Lead`"),
        "{ev}"
    );
    assert!(ev.contains("line 16"), "{ev}");

    let commit = &findings[1];
    assert_eq!(commit.severity, Severity::Major);
    assert_eq!(commit.category, "correctness");
    assert!(commit.body.contains("frappe.db.commit()"));
}

#[test]
fn context_lines_never_match() {
    // The pattern only appears on a context line and a deleted line.
    let d = "\
diff --git a/a.py b/a.py
--- a/a.py
+++ b/a.py
@@ -1,3 +1,3 @@
 frappe.db.commit()
-x = 1
+x = 2
 y = 3
";
    let files = parse_unified(d);
    assert!(run_regex_rules(&files, &default_rules())
        .unwrap()
        .is_empty());
}

#[test]
fn applies_to_is_respected() {
    let files = mr_files();
    // TSX added comment contains frappe.db.sql(f" — only a TS-scoped rule sees it
    let ts_rule = RuleConfig {
        pattern: default_rules()[1].pattern.clone(),
        ..rule("TS-SQL", "", Some("**/*.tsx"))
    };
    let f = run_regex_rules(&files, &[ts_rule]).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!((f[0].file.as_str(), f[0].line), (TSX, 88));

    // no applies_to => every file (non-binary, non-deleted)
    let any = run_regex_rules(&files, &[rule("ANY", r"return", None)]).unwrap();
    let files_hit: Vec<&str> = any.iter().map(|f| f.file.as_str()).collect();
    assert_eq!(
        files_hit,
        vec![IMPORT_LINKS, NEW_NAME],
        "TSX's `return` is a context line"
    );

    // basename-style glob without a slash matches at any depth
    let any_py = run_regex_rules(&files, &[rule("PY", r"return", Some("*.py"))]).unwrap();
    assert_eq!(any_py.len(), 2);
    // scoped glob
    let scoped = run_regex_rules(
        &files,
        &[rule("UTIL", r"return", Some("entries_crm/utils/**"))],
    )
    .unwrap();
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].file, NEW_NAME);
}

#[test]
fn multiple_hits_in_one_file_are_grouped() {
    let d = "\
diff --git a/a.py b/a.py
--- a/a.py
+++ b/a.py
@@ -1,1 +1,4 @@
 import frappe
+frappe.db.commit()
+x = 1
+frappe.db.commit()
";
    let f = run_regex_rules(&parse_unified(d), &default_rules()).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].line, 2);
    assert!(f[0].body.contains("(also on line 4)"), "{}", f[0].body);
}

#[test]
fn custom_rule_body_uses_instruction_and_redacts_evidence() {
    let d = "\
diff --git a/a.py b/a.py
--- a/a.py
+++ b/a.py
@@ -0,0 +1 @@
+print(\"debug\", \"glpat-AbCdEf1234567890xYz_q\")
";
    let r = RuleConfig {
        instruction: Some("Use frappe.logger() instead of print.".into()),
        ..rule("LOG-1", r"\bprint\(", None)
    };
    let f = run_regex_rules(&parse_unified(d), &[r]).unwrap();
    assert_eq!(f.len(), 1);
    assert!(f[0].body.contains("matches rule LOG-1"), "{}", f[0].body);
    assert!(f[0].body.contains("frappe.logger()"));
    let ev = f[0].evidence.as_deref().unwrap();
    assert!(
        !ev.contains("glpat-AbCd"),
        "evidence must be redacted: {ev}"
    );
    assert!(ev.contains("[REDACTED:gitlab_token]"));
    assert_eq!(f[0].severity, Severity::Minor);
}

#[test]
fn claude_and_semgrep_rules_are_not_regex_rules() {
    let mut r = rule("S", r"x", None);
    r.semgrep = Some("rules/x.yml".into());
    assert!(run_regex_rules(&mr_files(), &[r]).unwrap().is_empty());
}

#[test]
fn invalid_pattern_is_an_error_or_a_notice() {
    let bad = rule("BAD", "(unclosed", None);
    assert!(run_regex_rules(&mr_files(), std::slice::from_ref(&bad)).is_err());
    let mut notices = Vec::new();
    let good = rule("OK", "frappe\\.db\\.commit", Some("**/*.py"));
    let f = regex_rule_findings(&mr_files(), &[bad, good], &mut notices);
    assert_eq!(f.len(), 1, "the valid rule still runs");
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("BAD"));
}

// ---------------------------------------------------------------------------------------------
// run_checks
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn run_checks_without_optional_tools() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = WardenConfig::default();
    cfg.rules.push(RuleConfig {
        id: "FR-003".into(),
        title: "Semgrep rule".into(),
        applies_to: None,
        instruction: None,
        pattern: None,
        semgrep: Some("rules/x.yml".into()),
        severity: Severity::Major,
    });
    let tools = vec![
        ToolStatus {
            name: "semgrep".into(),
            installed: false,
            version: None,
        },
        ToolStatus {
            name: "gitleaks".into(),
            installed: false,
            version: None,
        },
    ];
    let report = run_checks_with(dir.path(), &mr_files(), &cfg, None, None, tools.clone())
        .await
        .unwrap();
    assert_eq!(report.tools, tools);
    assert!(
        report
            .notices
            .iter()
            .any(|n| n == "semgrep not installed; skipped FR-003"),
        "{:?}",
        report.notices
    );
    assert!(report
        .notices
        .iter()
        .any(|n| n.contains("gitleaks not installed")));

    let summary: Vec<(&str, u32, &str)> = report
        .findings
        .iter()
        .map(|f| (f.file.as_str(), f.line, f.rule_id.as_deref().unwrap()))
        .collect();
    assert_eq!(
        summary,
        vec![
            (IMPORT_LINKS, 16, "FR-002"),
            (IMPORT_LINKS, 17, "FR-007"),
            (TEST_FILE, 3, "SEC-001")
        ]
    );
    assert!(report
        .findings
        .iter()
        .all(|f| f.source == "rule" && f.confidence >= 0.95));
    // the AWS key in the deleted legacy file is not reported
    assert!(!report.findings.iter().any(|f| f.file == LEGACY));
}

#[tokio::test]
async fn secret_severity_follows_the_sec001_rule() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = warden_core::config::merge_overrides(
        WardenConfig::default(),
        &serde_json::json!({"rules": {"SEC-001": {"severity": "major"}}}),
    )
    .unwrap();
    let r = run_checks_with(dir.path(), &mr_files(), &cfg, None, None, vec![])
        .await
        .unwrap();
    let sec = r
        .findings
        .iter()
        .find(|f| f.rule_id.as_deref() == Some("SEC-001"))
        .unwrap();
    assert_eq!(sec.severity, Severity::Major);
}

#[tokio::test]
async fn run_checks_with_real_tool_detection_never_fails() {
    let dir = tempfile::tempdir().unwrap();
    let report = run_checks(dir.path(), &mr_files(), &WardenConfig::default())
        .await
        .unwrap();
    let names: Vec<&str> = report.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["semgrep", "gitleaks"]);
    assert!(report
        .findings
        .iter()
        .any(|f| f.rule_id.as_deref() == Some("FR-002")));
    // no semgrep rules in the default set => no semgrep notice either way
    assert!(!report.notices.iter().any(|n| n.contains("semgrep")));
}

#[tokio::test]
async fn run_checks_on_empty_diff() {
    let dir = tempfile::tempdir().unwrap();
    let r = run_checks_with(
        dir.path(),
        &[],
        &WardenConfig::default(),
        None,
        None,
        vec![],
    )
    .await
    .unwrap();
    assert!(r.findings.is_empty());
}

// ---------------------------------------------------------------------------------------------
// semgrep (fake binary + parser)
// ---------------------------------------------------------------------------------------------

fn semgrep_rule() -> RuleConfig {
    RuleConfig {
        id: "FR-003".into(),
        title: "SQL built from request input".into(),
        applies_to: Some("**/*.py".into()),
        instruction: None,
        pattern: None,
        semgrep: Some("rules/sql.yml".into()),
        severity: Severity::Blocker,
    }
}

const SEMGREP_JSON: &str = r#"{
  "results": [
    {"check_id": "rules.sql", "path": "entries_crm/api/import_links.py",
     "start": {"line": 16, "col": 9}, "end": {"line": 16, "col": 80},
     "extra": {"message": "f-string passed to frappe.db.sql", "lines": "        frappe.db.sql(f\"update ...\")", "severity": "ERROR"}},
    {"check_id": "rules.sql", "path": "entries_crm/api/import_links.py",
     "start": {"line": 9, "col": 1}, "end": {"line": 9, "col": 5},
     "extra": {"message": "context line, must be dropped", "lines": "import frappe"}},
    {"check_id": "rules.sql", "path": "somewhere/else.py",
     "start": {"line": 1, "col": 1}, "end": {"line": 1, "col": 5},
     "extra": {"message": "file not in diff"}}
  ],
  "errors": [{"message": "Syntax error at line foo.py:3"}]
}"#;

#[test]
fn semgrep_json_keeps_only_added_lines() {
    let files = mr_files();
    let index = ChangedIndex::from_files(&files);
    let (f, notices) = parse_semgrep_json(SEMGREP_JSON, &semgrep_rule(), &index, &files).unwrap();
    assert_eq!(f.len(), 1);
    assert_eq!((f[0].file.as_str(), f[0].line), (IMPORT_LINKS, 16));
    assert_eq!(f[0].rule_id.as_deref(), Some("FR-003"));
    assert_eq!(f[0].severity, Severity::Blocker);
    assert!(f[0].body.contains("f-string passed to frappe.db.sql"));
    assert!(f[0]
        .evidence
        .as_deref()
        .unwrap()
        .contains("semgrep rule FR-003"));
    assert!(f[0].confidence >= 0.95);
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("Syntax error"));
    assert!(parse_semgrep_json("not json", &semgrep_rule(), &index, &files).is_err());
}

#[tokio::test]
async fn semgrep_missing_binary_gives_notice_and_fake_binary_gives_findings() {
    let files = mr_files();
    let wt = tempfile::tempdir().unwrap();
    // missing
    let (f, n) = run_semgrep_with(None, wt.path(), &files, &[semgrep_rule()]).await;
    assert!(f.is_empty());
    assert_eq!(n, vec!["semgrep not installed; skipped FR-003".to_string()]);

    // rule file missing in the worktree
    let bin_dir = tempfile::tempdir().unwrap();
    let fake = script(
        bin_dir.path(),
        "semgrep",
        &format!("cat <<'JSON'\n{SEMGREP_JSON}\nJSON"),
    );
    let (f, n) = run_semgrep_with(Some(&fake), wt.path(), &files, &[semgrep_rule()]).await;
    assert!(f.is_empty());
    assert!(n[0].contains("not found"), "{n:?}");

    // rule file + target file present => the fake output is parsed
    std::fs::create_dir_all(wt.path().join("rules")).unwrap();
    std::fs::write(wt.path().join("rules/sql.yml"), "rules: []\n").unwrap();
    std::fs::create_dir_all(wt.path().join("entries_crm/api")).unwrap();
    std::fs::write(wt.path().join(IMPORT_LINKS), "import frappe\n").unwrap();
    let (f, n) = run_semgrep_with(Some(&fake), wt.path(), &files, &[semgrep_rule()]).await;
    assert_eq!(f.len(), 1, "{n:?}");
    assert_eq!(f[0].line, 16);

    // garbage output => notice, not a crash
    let broken = script(
        bin_dir.path(),
        "semgrep-broken",
        "echo 'boom' >&2; echo 'not json'",
    );
    let (f, n) = run_semgrep_with(Some(&broken), wt.path(), &files, &[semgrep_rule()]).await;
    assert!(f.is_empty());
    assert!(n[0].contains("unreadable"), "{n:?}");

    // no semgrep rules => nothing at all
    let (f, n) = run_semgrep_with(None, wt.path(), &files, &default_rules()).await;
    assert!(f.is_empty() && n.is_empty());
}

// ---------------------------------------------------------------------------------------------
// gitleaks (fake binary + parser)
// ---------------------------------------------------------------------------------------------

const CONF_DIFF: &str = "\
diff --git a/app/conf.py b/app/conf.py
--- a/app/conf.py
+++ b/app/conf.py
@@ -1,1 +1,3 @@
 import client
+x = 1
+c = Client(key=\"Zm9vYmFyYmF6cXV4cXV1eA\")
";

#[tokio::test]
async fn gitleaks_fake_binary_scans_a_mirror_of_added_lines() {
    let files = parse_unified(CONF_DIFF);
    let bin_dir = tempfile::tempdir().unwrap();
    // The fake checks the mirror puts the added line at its real line number (3) and that the
    // untouched context line 1 is blank, then reports a leak like gitleaks would.
    let fake = script(
        bin_dir.path(),
        "gitleaks",
        r#"while [ $# -gt 0 ]; do
  case "$1" in
    --source) shift; src="$1" ;;
    --report-path) shift; out="$1" ;;
  esac
  shift
done
line3=$(sed -n 3p "$src/app/conf.py")
line1=$(sed -n 1p "$src/app/conf.py")
case "$line3" in
  *Client*) ;;
  *) echo "[]" > "$out"; exit 0 ;;
esac
[ -z "$line1" ] || { echo "[]" > "$out"; exit 0; }
printf '[{"RuleID":"generic-api-key","Description":"Generic API Key","File":"%s/app/conf.py","StartLine":3,"Secret":"REDACTED"}]' "$src" > "$out""#,
    );
    let wt = tempfile::tempdir().unwrap();
    let cfg = WardenConfig::default();
    let r = run_checks_with(wt.path(), &files, &cfg, None, Some(&fake), vec![])
        .await
        .unwrap();
    assert!(r.notices.is_empty(), "{:?}", r.notices);
    assert_eq!(r.findings.len(), 1, "{:?}", r.findings);
    let f = &r.findings[0];
    assert_eq!((f.file.as_str(), f.line), ("app/conf.py", 3));
    assert_eq!(f.rule_id.as_deref(), Some("SEC-001"));
    assert_eq!(f.title, "Hardcoded Generic API Key");
    assert!(f.body.contains("gitleaks"));
}

#[tokio::test]
async fn gitleaks_hits_on_lines_the_regex_scanner_found_are_deduped() {
    let files = mr_files();
    let bin_dir = tempfile::tempdir().unwrap();
    let fake = script(
        bin_dir.path(),
        "gitleaks",
        &format!(
            r#"while [ $# -gt 0 ]; do case "$1" in --source) shift; src="$1" ;; --report-path) shift; out="$1" ;; esac; shift; done
printf '[{{"RuleID":"gitlab-pat","Description":"GitLab PAT","File":"%s/{TEST_FILE}","StartLine":3}}]' "$src" > "$out""#
        ),
    );
    let wt = tempfile::tempdir().unwrap();
    let r = run_checks_with(
        wt.path(),
        &files,
        &WardenConfig::default(),
        None,
        Some(&fake),
        vec![],
    )
    .await
    .unwrap();
    let secrets: Vec<&NewFinding> = r
        .findings
        .iter()
        .filter(|f| f.rule_id.as_deref() == Some("SEC-001"))
        .collect();
    assert_eq!(secrets.len(), 1);
    assert_eq!(secrets[0].title, "Hardcoded GitLab token");
}

#[tokio::test]
async fn gitleaks_failures_become_notices() {
    let files = parse_unified(CONF_DIFF);
    let bin_dir = tempfile::tempdir().unwrap();
    let no_report = script(
        bin_dir.path(),
        "gitleaks",
        "echo 'fatal: bad flag' >&2; exit 2",
    );
    let r = run_checks_with(
        Path::new("/"),
        &files,
        &WardenConfig::default(),
        None,
        Some(&no_report),
        vec![],
    )
    .await
    .unwrap();
    assert!(
        r.notices
            .iter()
            .any(|n| n.contains("gitleaks produced no report")),
        "{:?}",
        r.notices
    );

    let (f, n) = run_gitleaks(Path::new("/"), &files).await.unwrap();
    // real PATH: either not installed (notice) or installed (no crash)
    assert!(f.len() <= 1);
    let _ = n;
}

#[test]
fn gitleaks_report_parser() {
    let files = parse_unified(CONF_DIFF);
    let root = Path::new("/tmp/mirror/src");
    let json = r#"[
      {"RuleID":"generic-api-key","Description":"Generic API Key","File":"/tmp/mirror/src/app/conf.py","StartLine":3},
      {"RuleID":"x","Description":"","File":"app/conf.py","StartLine":2},
      {"RuleID":"y","Description":"Y","File":"/tmp/mirror/src/app/conf.py","StartLine":1},
      {"RuleID":"z","Description":"Z","File":"/tmp/mirror/src/unknown.py","StartLine":1}
    ]"#;
    let hits = parse_gitleaks_report(json, root, &files).unwrap();
    let got: Vec<(u32, &str, &str)> = hits
        .iter()
        .map(|h| (h.line, h.kind.as_str(), h.label.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (3, "gitleaks:generic-api-key", "Generic API Key"),
            (2, "gitleaks:x", "secret (x)")
        ]
    );
    assert!(parse_gitleaks_report("", root, &files).unwrap().is_empty());
    assert!(parse_gitleaks_report("null", root, &files)
        .unwrap()
        .is_empty());
    assert!(parse_gitleaks_report("{bad", root, &files).is_err());
}

// ---------------------------------------------------------------------------------------------
// Secret scanning + redaction
// ---------------------------------------------------------------------------------------------

/// (input, kind expected in the marker, raw secret that must disappear)
fn secret_cases() -> Vec<(String, &'static str, String)> {
    let gh = format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8");
    let ghpat = format!(
        "github_pat_{}",
        "11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyzABCDEFGHIJ"
    );
    vec![
        (
            "aws_key = AKIAZ3MQRSTUVWXY1234".into(),
            "aws_access_key",
            "AKIAZ3MQRSTUVWXY1234".into(),
        ),
        (
            "aws_secret_access_key = \"wJalrXUtnFEMI/K7MDENG/bPxRfiCYzEXAMPLEKQ\"".into(),
            "",
            String::new(),
        ),
        (
            "AWS_SECRET_ACCESS_KEY=\"Zx9Yw8Vu7Ts6Rq5Po4Nm3Lk2Ji1Hg0FeDcBa9876\"".into(),
            "aws_secret_key",
            "Zx9Yw8Vu7Ts6Rq5Po4Nm3Lk2Ji1Hg0FeDcBa9876".into(),
        ),
        (
            "token: glpat-AbCdEf1234567890xYz_q".into(),
            "gitlab_token",
            "glpat-AbCdEf1234567890xYz_q".into(),
        ),
        (format!("GH = '{gh}'"), "github_token", gh.clone()),
        (format!("pat={ghpat}"), "github_token", ghpat.clone()),
        (
            "SLACK = 'xoxb-1234567890-abcdefghijkl'".into(),
            "slack_token",
            "xoxb-1234567890-abcdefghijkl".into(),
        ),
        (
            "hook = 'https://hooks.slack.com/services/T0001/B0002/XXyyZZ112233'".into(),
            "slack_webhook",
            "hooks.slack.com/services/T0001".into(),
        ),
        (
            "headers = {'Authorization': 'token 0123456789abcde:fedcba987654321'}".into(),
            "frappe_api_token",
            "fedcba987654321".into(),
        ),
        (
            "api_secret = \"s3cr3tValue99\"".into(),
            "frappe_api_secret",
            "s3cr3tValue99".into(),
        ),
        (
            "API_KEY = \"9f8e7d6c5b4a39281706\"".into(),
            "generic_secret",
            "9f8e7d6c5b4a39281706".into(),
        ),
        (
            "password: \"hunter2hunter2\"".into(),
            "generic_secret",
            "hunter2hunter2".into(),
        ),
        (
            "\"client_secret\": \"Qw3rTy-UiOp-AsDf\"".into(),
            "generic_secret",
            "Qw3rTy-UiOp-AsDf".into(),
        ),
        (
            "DB_PASSWORD='Pa55w0rd!xyz'".into(),
            "generic_secret",
            "Pa55w0rd!xyz".into(),
        ),
        (
            "url = 'postgres://admin:sup3rs3cret@db.internal:5432/crm'".into(),
            "url_credentials",
            "sup3rs3cret".into(),
        ),
        (
            "r = get(u, headers={'Authorization': 'Bearer abcdefghijklmnopqrstuvwxyz012345'})"
                .into(),
            "bearer_token",
            "abcdefghijklmnopqrstuvwxyz012345".into(),
        ),
        (
            "key = 'AIzaSyA1234567890abcdefghijklmnopqrstuv'".into(),
            "google_api_key",
            "AIzaSyA1234567890abcdefghijklmnopqrstuv".into(),
        ),
        (
            format!("stripe = '{}'", fake_stripe_key()),
            "stripe_key",
            fake_stripe_key(),
        ),
    ]
}

/// Built at runtime so the source has no key-shaped literal (GitHub push protection would flag it).
fn fake_stripe_key() -> String {
    ["sk", "live", "51Habcdefghijklmnopqrstu"].join("_")
}

#[test]
fn redacts_every_secret_kind() {
    for (input, kind, secret) in secret_cases() {
        let (out, n) = redact_secrets(&input);
        if kind.is_empty() {
            // AWS's documented example key is a placeholder and stays untouched
            assert_eq!((out.as_str(), n), (input.as_str(), 0), "{input}");
            continue;
        }
        assert!(n >= 1, "nothing redacted in {input:?}");
        assert!(
            out.contains(&format!("[REDACTED:{kind}]")),
            "{input:?} -> {out:?}"
        );
        assert!(
            !out.contains(&secret),
            "{input:?} still leaks {secret:?}: {out:?}"
        );
    }
}

#[test]
fn redaction_keeps_key_names_and_counts() {
    let (out, n) =
        redact_secrets("password = \"hunter2hunter2\"\napi_key: 'abcdef1234567890'\nplain = 1\n");
    assert_eq!(n, 2);
    assert_eq!(
        out,
        "password = \"[REDACTED:generic_secret]\"\napi_key: '[REDACTED:generic_secret]'\nplain = 1\n"
    );
    // a token inside an assignment is counted once, under its specific kind
    let (out, n) = redact_secrets("GITLAB_TOKEN = \"glpat-AbCdEf1234567890xYz_q\"");
    assert_eq!(n, 1);
    assert_eq!(out, "GITLAB_TOKEN = \"[REDACTED:gitlab_token]\"");
    // idempotent
    let (again, n2) = redact_secrets(&out);
    assert_eq!((again.as_str(), n2), (out.as_str(), 0));
}

#[test]
fn redacts_multiline_private_key_blocks_in_a_diff() {
    let diff = "\
+++ b/deploy/key.pem
@@ -0,0 +1,5 @@
+-----BEGIN RSA PRIVATE KEY-----
+MIIEowIBAAKCAQEAu1SU1LfVLPHCozMxH2Mo4lgOEePzNm0tRgeLezV6ffAt0gun
+VTLw7onLRnrq0/IzW7yWR7QkrmBL7jTKEn5u+qKhbwKfBstIs+bMY2Zkp18gnTxK
+-----END RSA PRIVATE KEY-----
+after = 1
";
    let (out, n) = redact_secrets(diff);
    assert_eq!(n, 1);
    assert!(out.contains("+[REDACTED:private_key]\n+after = 1"), "{out}");
    assert!(!out.contains("MIIEow"));
    // a truncated block (no END line) is redacted to the end rather than leaked
    let (out, _) = redact_secrets("x\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n");
    assert_eq!(out, "x\n[REDACTED:private_key]");
}

#[test]
fn placeholders_and_normal_code_are_left_alone() {
    let clean = [
        "password = \"changeme\"",
        "api_key = os.environ[\"API_KEY\"]",
        "token = frappe.conf.get(\"token\")",
        "secret = \"${SECRET}\"",
        "password: '{{ vault_password }}'",
        "API_KEY = \"<your-api-key>\"",
        "password = \"xxxxxxxxxx\"",
        "frappe.db.sql(\"select name from tabUser where api_key=%s\", key)",
        "tokenizer = \"bert-base-uncased-model\"",
        "fieldtype = \"Password\"",
        "url = 'https://example.com:8080/path'",
        "",
    ];
    for c in clean {
        let (out, n) = redact_secrets(c);
        assert_eq!((out.as_str(), n), (c, 0), "{c:?}");
    }
}

#[test]
fn scan_added_lines_finds_secrets_only_on_added_lines() {
    let findings = scan_added_lines(&mr_files());
    // test file adds a GitLab token (line 3) and a placeholder password (ignored);
    // the AWS key is on a deleted line of the legacy file => not reported.
    assert_eq!(findings.len(), 1, "{findings:#?}");
    let f = &findings[0];
    assert_eq!(
        (f.file.as_str(), f.line, f.side.as_str()),
        (TEST_FILE, 3, "new")
    );
    assert_eq!(f.rule_id.as_deref(), Some("SEC-001"));
    assert_eq!(f.severity, Severity::Blocker);
    assert_eq!(f.category, "security");
    assert_eq!(f.source, "rule");
    assert_eq!(f.title, "Hardcoded GitLab token");
    assert!(f.confidence >= 0.95);
    let ev = f.evidence.as_deref().unwrap();
    assert_eq!(ev, "GITLAB_TOKEN = \"[REDACTED:gitlab_token]\"");
    assert!(!f.body.contains("glpat-"));
}

#[test]
fn scan_groups_per_file_and_kind() {
    let d = "\
diff --git a/s.py b/s.py
--- a/s.py
+++ b/s.py
@@ -0,0 +1,4 @@
+A = \"AKIAZ3MQRSTUVWXY1234\"
+B = \"AKIAZ3MQRSTUVWXY5678\"
+password = \"hunter2hunter2\"
+C = \"AKIAZ3MQRSTUVWXY9999\"
";
    let files = parse_unified(d);
    let hits = secret_hits(&files);
    assert_eq!(hits.len(), 4);
    let f = scan_added_lines(&files);
    let got: Vec<(u32, &str)> = f.iter().map(|f| (f.line, f.title.as_str())).collect();
    assert_eq!(
        got,
        vec![(1, "Hardcoded AWS access key"), (3, "Hardcoded credential")]
    );
    assert!(f[0].body.contains("(also on lines 2, 4)"), "{}", f[0].body);
}

#[test]
fn binary_files_are_not_scanned() {
    let mut files = parse_unified(CONF_DIFF);
    files[0].is_binary = true;
    assert!(scan_added_lines(&files).is_empty());
    assert!(run_regex_rules(&files, &[rule("ANY", ".", None)])
        .unwrap()
        .is_empty());
}

// ---------------------------------------------------------------------------------------------
// Tool detection
// ---------------------------------------------------------------------------------------------

#[test]
fn detect_tools_reports_every_known_tool() {
    let tools = detect_tools();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, KNOWN_TOOLS.to_vec());
    let git = tools.iter().find(|t| t.name == "git").unwrap();
    assert!(git.installed, "git is required on the dev machine");
    let v = git.version.as_deref().unwrap();
    assert!(v.chars().next().unwrap().is_ascii_digit(), "{v}");
    for t in &tools {
        if !t.installed {
            assert_eq!(t.version, None);
        }
    }
}

#[test]
fn missing_tool_is_not_installed() {
    let t = detect_tool("warden-definitely-not-a-real-tool-xyz");
    assert_eq!(
        t,
        ToolStatus {
            name: "warden-definitely-not-a-real-tool-xyz".into(),
            installed: false,
            version: None
        }
    );
}

#[test]
fn version_parsing() {
    assert_eq!(
        parse_version("git version 2.50.1 (Apple Git-155)\n").as_deref(),
        Some("2.50.1")
    );
    assert_eq!(
        parse_version("Docker version 27.3.1, build ce12230").as_deref(),
        Some("27.3.1")
    );
    assert_eq!(
        parse_version("2.1.330 (Claude Code)").as_deref(),
        Some("2.1.330")
    );
    assert_eq!(
        parse_version("ripgrep 14.1.1\n\nfeatures:+pcre2").as_deref(),
        Some("14.1.1")
    );
    assert_eq!(parse_version("v8.18.4").as_deref(), Some("8.18.4"));
    assert_eq!(parse_version("1.97.0").as_deref(), Some("1.97.0"));
    assert_eq!(
        parse_version("act version 0.2.68").as_deref(),
        Some("0.2.68")
    );
    assert_eq!(
        parse_version("\n  weird tool\n").as_deref(),
        Some("weird tool")
    );
    assert_eq!(parse_version("   \n"), None);
}

#[test]
fn version_probe_times_out_quickly() {
    let dir = tempfile::tempdir().unwrap();
    let slow = script(dir.path(), "slowtool", "sleep 5; echo 1.0.0");
    let start = Instant::now();
    assert_eq!(tool_version(&slow, Duration::from_millis(300)), None);
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "{:?}",
        start.elapsed()
    );

    let stderr_only = script(dir.path(), "errtool", "echo 'errtool 3.4.5' >&2");
    assert_eq!(
        tool_version(&stderr_only, Duration::from_secs(3)).as_deref(),
        Some("3.4.5")
    );
    assert_eq!(
        tool_version(Path::new("/nonexistent/bin/x"), Duration::from_secs(1)),
        None
    );
}
