//! config.rs: .warden.yml parsing, defaults, validation, overrides, durations, built-in rules.

use std::time::Duration;

use pretty_assertions::assert_eq;
use regex::Regex;
use serde_json::json;
use warden_core::config::{
    default_rules, load_repo_config, merge_overrides, parse_duration, parse_repo_config,
    AppSettings, RuleKind, WardenConfig,
};
use warden_core::error::WardenError;
use warden_core::types::{PostingMode, Severity};

/// The `.warden.yml` example from SPEC.md, verbatim.
const SPEC_YAML: &str = r#"
version: 1
review:
  focus: [security, correctness, frappe-permissions, performance]
  ignore_paths: ["yarn.lock", "**/*.min.js", "**/dist/**"]
  max_notes: 10
  min_confidence: 0.7
  severity_floor: minor        # blocker|major|minor|nit
  nits: false
  extra_instructions: ""       # free text appended to the prompt
rules:
  - id: FR-001
    title: Whitelisted methods must check permissions
    applies_to: "**/*.py"      # glob, optional
    instruction: "..."         # => Claude rule (goes into prompt)
    severity: major
  - id: FR-002
    title: No string-formatted SQL
    pattern: 'frappe\.db\.sql\(\s*f["'']'   # => deterministic regex rule over ADDED lines
    severity: blocker
  - id: FR-003
    title: ...
    semgrep: rules/x.yml       # => run semgrep if installed, else skip with notice
    severity: major
ci:
  recipes:
    - name: lint
      run: "ruff check {changed_py}"
      cwd: "."                 # relative to worktree
      timeout: "5m"
      env: {}
    - name: gitlab-ci
      use: gitlab-ci-local
      jobs: [build]
posting:
  mode: assist
  auto_post: { severities: [blocker, major], min_confidence: 0.85 }
  summary_note: true
  max_notes_per_hour: 30
"#;

fn config_err(r: Result<WardenConfig, WardenError>) -> String {
    match r {
        Err(WardenError::Config(m)) => m,
        Err(other) => panic!("expected a Config error, got {other:?}"),
        Ok(c) => panic!("expected an error, got {c:?}"),
    }
}

#[test]
fn parses_the_spec_example() {
    let cfg = parse_repo_config(SPEC_YAML).unwrap();
    assert_eq!(cfg.version, 1);
    assert_eq!(
        cfg.review.focus,
        vec![
            "security",
            "correctness",
            "frappe-permissions",
            "performance"
        ]
    );
    assert_eq!(
        cfg.review.ignore_paths,
        vec!["yarn.lock", "**/*.min.js", "**/dist/**"]
    );
    assert_eq!(cfg.review.max_notes, 10);
    assert!((cfg.review.min_confidence - 0.7).abs() < 1e-6);
    assert_eq!(cfg.review.severity_floor, Severity::Minor);
    assert!(!cfg.review.nits);

    assert_eq!(cfg.rules.len(), 3);
    let kinds: Vec<(&str, RuleKind)> = cfg
        .rules
        .iter()
        .map(|r| (r.id.as_str(), r.kind()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("FR-001", RuleKind::Claude),
            ("FR-002", RuleKind::Regex),
            ("FR-003", RuleKind::Semgrep)
        ]
    );
    // YAML '' escape inside a single-quoted scalar becomes a single quote
    assert_eq!(
        cfg.rules[1].pattern.as_deref(),
        Some(r#"frappe\.db\.sql\(\s*f["']"#)
    );
    assert_eq!(cfg.rules[1].severity, Severity::Blocker);
    assert_eq!(cfg.rules[0].applies_to.as_deref(), Some("**/*.py"));
    assert_eq!(cfg.rules[2].semgrep.as_deref(), Some("rules/x.yml"));
    assert_eq!(cfg.rules[2].applies_to, None);

    assert_eq!(cfg.ci.recipes.len(), 2);
    let lint = &cfg.ci.recipes[0];
    assert_eq!(lint.run.as_deref(), Some("ruff check {changed_py}"));
    assert_eq!(lint.cwd.as_deref(), Some("."));
    assert_eq!(lint.timeout_duration(), Some(Duration::from_secs(300)));
    let glci = &cfg.ci.recipes[1];
    assert_eq!(glci.use_.as_deref(), Some("gitlab-ci-local"));
    assert_eq!(glci.jobs, vec!["build"]);
    assert_eq!(glci.run, None);

    assert_eq!(cfg.posting.mode, PostingMode::Assist);
    assert_eq!(
        cfg.posting.auto_post.severities,
        vec![Severity::Blocker, Severity::Major]
    );
    assert!((cfg.posting.auto_post.min_confidence - 0.85).abs() < 1e-6);
    assert!(cfg.posting.summary_note);
    assert_eq!(cfg.posting.max_notes_per_hour, 30);

    // the regex from the spec example catches the mockup's f-string SQL
    let re = Regex::new(cfg.rules[1].pattern.as_deref().unwrap()).unwrap();
    assert!(re.is_match(r#"        frappe.db.sql(f"update `tabECRM Lead` set import_ref='{import_name}' where name='{r}'")"#));
}

#[test]
fn missing_file_gives_defaults_with_builtin_rules() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = load_repo_config(dir.path()).unwrap();
    assert_eq!(cfg, WardenConfig::default());
    let ids: Vec<&str> = cfg.rules.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["FR-001", "FR-002", "FR-004", "FR-007", "SEC-001"]);
    assert_eq!(cfg.posting.mode, PostingMode::Draft);
    assert!(cfg.ci.recipes.is_empty());
}

#[test]
fn loads_warden_yml_and_yaml_fallback() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".warden.yaml"), "review:\n  max_notes: 4\n").unwrap();
    assert_eq!(load_repo_config(dir.path()).unwrap().review.max_notes, 4);
    // .warden.yml wins over .warden.yaml
    std::fs::write(dir.path().join(".warden.yml"), "review:\n  max_notes: 7\n").unwrap();
    assert_eq!(load_repo_config(dir.path()).unwrap().review.max_notes, 7);
}

#[test]
fn spec_example_loads_from_a_worktree() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".warden.yml"), SPEC_YAML).unwrap();
    let cfg = load_repo_config(dir.path()).unwrap();
    assert_eq!(cfg, parse_repo_config(SPEC_YAML).unwrap());
}

#[test]
fn empty_or_comment_only_file_gives_defaults() {
    for text in [
        "",
        "   \n\n",
        "# just a comment\n# another\n",
        "---\n",
        "---\n# c\n",
    ] {
        assert_eq!(
            parse_repo_config(text).unwrap(),
            WardenConfig::default(),
            "{text:?}"
        );
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".warden.yml"), "# nothing yet\n").unwrap();
    assert_eq!(
        load_repo_config(dir.path()).unwrap(),
        WardenConfig::default()
    );
}

#[test]
fn partial_file_keeps_defaults_for_everything_else() {
    let cfg = parse_repo_config("review:\n  max_notes: 3\n  nits: true\n").unwrap();
    let d = WardenConfig::default();
    assert_eq!(cfg.review.max_notes, 3);
    assert!(cfg.review.nits);
    assert_eq!(cfg.review.min_confidence, d.review.min_confidence);
    assert_eq!(cfg.review.ignore_paths, d.review.ignore_paths);
    assert_eq!(
        cfg.rules,
        default_rules(),
        "no `rules:` key keeps the built-in set"
    );
    assert_eq!(cfg.posting, d.posting);

    // an explicit empty list disables all rules
    assert!(parse_repo_config("rules: []\n").unwrap().rules.is_empty());
    // a rule without severity defaults to major; unknown keys are ignored
    let cfg = parse_repo_config(
        "rules:\n  - id: X-1\n    title: Something\n    future_field: 1\nwhatever: true\n",
    )
    .unwrap();
    assert_eq!(cfg.rules[0].severity, Severity::Major);
    assert_eq!(cfg.rules[0].kind(), RuleKind::Claude);
    assert_eq!(cfg.rules[0].instruction_text(), "Something");
}

#[test]
fn invalid_yaml_is_a_config_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".warden.yml"),
        "review:\n  max_notes: [unclosed\n",
    )
    .unwrap();
    let msg = config_err(load_repo_config(dir.path()));
    assert!(msg.contains(".warden.yml"), "{msg}");
    assert!(msg.contains("yaml"), "{msg}");

    let msg = config_err(parse_repo_config("review: {max_notes: 3\n"));
    assert!(msg.contains("yaml"), "{msg}");
    // wrong types / unknown enum values
    config_err(parse_repo_config("review:\n  max_notes: lots\n"));
    let msg = config_err(parse_repo_config("review:\n  severity_floor: critical\n"));
    assert!(msg.contains("critical") || msg.contains("variant"), "{msg}");
    config_err(parse_repo_config("posting:\n  mode: yolo\n"));
    config_err(parse_repo_config("- just\n- a list\n"));
    config_err(parse_repo_config("rules:\n  - title: no id\n"));
}

#[test]
fn validation_errors() {
    let cases: &[(&str, &str)] = &[
        ("version: 2\n", "version"),
        ("review:\n  min_confidence: 1.5\n", "min_confidence"),
        ("review:\n  ignore_paths: ['a/[b']\n", "ignore_paths"),
        (
            "rules:\n  - id: R1\n    title: t\n    pattern: '(unclosed'\n",
            "R1",
        ),
        (
            "rules:\n  - id: R1\n    title: t\n  - id: R1\n    title: u\n",
            "duplicate rule id R1",
        ),
        ("rules:\n  - id: ''\n    title: t\n", "id"),
        ("rules:\n  - id: R1\n    title: ''\n", "title"),
        (
            "rules:\n  - id: R1\n    title: t\n    applies_to: '[x'\n",
            "applies_to",
        ),
        (
            "rules:\n  - id: R1\n    title: t\n    semgrep: ''\n",
            "semgrep",
        ),
        (
            "ci:\n  recipes:\n    - name: a\n      run: x\n      use: gitlab-ci-local\n",
            "exactly one",
        ),
        ("ci:\n  recipes:\n    - name: a\n", "exactly one"),
        (
            "ci:\n  recipes:\n    - name: a\n      run: x\n      timeout: soon\n",
            "timeout",
        ),
        (
            "ci:\n  recipes:\n    - name: a\n      run: x\n    - name: a\n      run: y\n",
            "duplicate recipe",
        ),
        ("ci:\n  recipes:\n    - name: ''\n      run: x\n", "name"),
        (
            "posting:\n  auto_post:\n    min_confidence: -0.1\n",
            "auto_post",
        ),
    ];
    for (yaml, needle) in cases {
        let msg = config_err(parse_repo_config(yaml));
        assert!(
            msg.contains(needle),
            "yaml {yaml:?}: error {msg:?} should mention {needle:?}"
        );
    }
}

#[test]
fn unreadable_config_path_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    // a directory named .warden.yml cannot be read as a file
    std::fs::create_dir(dir.path().join(".warden.yml")).unwrap();
    config_err(load_repo_config(dir.path()));
}

// ---------------------------------------------------------------------------------------------
// merge_overrides
// ---------------------------------------------------------------------------------------------

#[test]
fn merge_null_and_empty_are_no_ops() {
    let base = parse_repo_config(SPEC_YAML).unwrap();
    assert_eq!(
        merge_overrides(base.clone(), &serde_json::Value::Null).unwrap(),
        base
    );
    assert_eq!(merge_overrides(base.clone(), &json!({})).unwrap(), base);
}

#[test]
fn merge_deep_merges_objects_and_replaces_arrays() {
    let base = parse_repo_config(SPEC_YAML).unwrap();
    let merged = merge_overrides(
        base.clone(),
        &json!({
            "review": {"max_notes": 3, "ignore_paths": ["*.lock"]},
            "posting": {"mode": "auto", "auto_post": {"min_confidence": 0.9}}
        }),
    )
    .unwrap();
    assert_eq!(merged.review.max_notes, 3);
    assert_eq!(merged.review.ignore_paths, vec!["*.lock"]);
    assert_eq!(
        merged.review.focus, base.review.focus,
        "untouched siblings survive"
    );
    assert_eq!(merged.review.severity_floor, base.review.severity_floor);
    assert_eq!(merged.posting.mode, PostingMode::Auto);
    assert!((merged.posting.auto_post.min_confidence - 0.9).abs() < 1e-6);
    assert_eq!(
        merged.posting.auto_post.severities,
        base.posting.auto_post.severities
    );
    assert_eq!(merged.rules, base.rules);
    assert_eq!(merged.ci, base.ci);
}

#[test]
fn merge_null_resets_a_key_to_its_default() {
    let base = merge_overrides(
        WardenConfig::default(),
        &json!({"review": {"min_confidence": 0.95}}),
    )
    .unwrap();
    assert!((base.review.min_confidence - 0.95).abs() < 1e-6);
    let reset = merge_overrides(base, &json!({"review": {"min_confidence": null}})).unwrap();
    assert!((reset.review.min_confidence - 0.7).abs() < 1e-6);
}

#[test]
fn merge_rules_array_replaces_the_list() {
    let merged = merge_overrides(
        WardenConfig::default(),
        &json!({"rules": [{"id": "MY-1", "title": "No print()", "pattern": "\\bprint\\(", "severity": "minor"}]}),
    )
    .unwrap();
    assert_eq!(merged.rules.len(), 1);
    assert_eq!(merged.rules[0].kind(), RuleKind::Regex);
    assert_eq!(merged.rules[0].severity, Severity::Minor);
}

#[test]
fn merge_rules_object_patches_by_id() {
    let merged = merge_overrides(
        WardenConfig::default(),
        &json!({"rules": {
            "FR-007": {"severity": "minor"},
            "FR-004": null,
            "NEW-1": {"instruction": "Prefer frappe.qb over raw SQL", "severity": "nit"},
            "GHOST": null
        }}),
    )
    .unwrap();
    let ids: Vec<&str> = merged.rules.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["FR-001", "FR-002", "FR-007", "SEC-001", "NEW-1"]);
    let fr7 = merged.rule("FR-007").unwrap();
    assert_eq!(fr7.severity, Severity::Minor);
    assert_eq!(fr7.pattern, default_rules()[3].pattern, "other fields kept");
    let new = merged.rule("NEW-1").unwrap();
    assert_eq!(new.title, "NEW-1");
    assert_eq!(new.kind(), RuleKind::Claude);
    assert_eq!(new.severity, Severity::Nit);
}

#[test]
fn merge_rejects_bad_overrides() {
    for bad in [json!([1, 2]), json!("x"), json!(3), json!(true)] {
        let msg = config_err(merge_overrides(WardenConfig::default(), &bad));
        assert!(msg.contains("must be a JSON object"), "{msg}");
    }
    let msg = config_err(merge_overrides(
        WardenConfig::default(),
        &json!({"review": {"max_notes": "ten"}}),
    ));
    assert!(msg.contains("invalid overrides"), "{msg}");
    config_err(merge_overrides(
        WardenConfig::default(),
        &json!({"posting": {"mode": "yolo"}}),
    ));
    // merged result is validated too
    config_err(merge_overrides(
        WardenConfig::default(),
        &json!({"review": {"min_confidence": 7}}),
    ));
    config_err(merge_overrides(
        WardenConfig::default(),
        &json!({"rules": {"FR-002": {"pattern": "("}}}),
    ));
}

#[test]
fn config_round_trips_through_json_and_yaml() {
    let cfg = parse_repo_config(SPEC_YAML).unwrap();
    let json = serde_json::to_value(&cfg).unwrap();
    assert_eq!(json["ci"]["recipes"][1]["use"], "gitlab-ci-local");
    assert_eq!(json["posting"]["mode"], "assist");
    let back: WardenConfig = serde_json::from_value(json).unwrap();
    assert_eq!(back, cfg);
    let yaml = serde_yaml::to_string(&cfg).unwrap();
    assert_eq!(parse_repo_config(&yaml).unwrap(), cfg);
}

// ---------------------------------------------------------------------------------------------
// parse_duration
// ---------------------------------------------------------------------------------------------

#[test]
fn parse_duration_accepts_spec_forms() {
    let ok = |s: &str| parse_duration(s).unwrap_or_else(|e| panic!("{s:?}: {e}"));
    assert_eq!(ok("30s"), Duration::from_secs(30));
    assert_eq!(ok("5m"), Duration::from_secs(300));
    assert_eq!(ok("1h"), Duration::from_secs(3600));
    assert_eq!(ok("90"), Duration::from_secs(90));
    assert_eq!(ok("0"), Duration::ZERO);
    assert_eq!(ok("15m"), Duration::from_secs(900));
    assert_eq!(ok(" 2M "), Duration::from_secs(120));
    assert_eq!(ok("1h30m"), Duration::from_secs(5400));
    assert_eq!(ok("1h 30m 15s"), Duration::from_secs(5415));
    assert_eq!(ok("500ms"), Duration::from_millis(500));
    assert_eq!(ok("1.5h"), Duration::from_secs(5400));
    assert_eq!(ok("10 sec"), Duration::from_secs(10));
    assert_eq!(ok("5 minutes"), Duration::from_secs(300));
    assert_eq!(ok("2d"), Duration::from_secs(172_800));
    assert_eq!(ok("2.5"), Duration::from_millis(2500));
}

#[test]
fn parse_duration_rejects_garbage() {
    for bad in [
        "", "   ", "abc", "5x", "-5s", "m", "5m3", "1h-2m", "s5", "1e400s", "..s",
    ] {
        match parse_duration(bad) {
            Err(WardenError::Config(m)) => assert!(m.contains("invalid duration"), "{m}"),
            other => panic!("{bad:?} should fail, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Built-in rules
// ---------------------------------------------------------------------------------------------

#[test]
fn default_rules_kinds_and_severities() {
    let rules = default_rules();
    let summary: Vec<(&str, RuleKind, Severity)> = rules
        .iter()
        .map(|r| (r.id.as_str(), r.kind(), r.severity))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("FR-001", RuleKind::Claude, Severity::Major),
            ("FR-002", RuleKind::Regex, Severity::Blocker),
            ("FR-004", RuleKind::Claude, Severity::Major),
            ("FR-007", RuleKind::Regex, Severity::Major),
            ("SEC-001", RuleKind::Claude, Severity::Blocker),
        ]
    );
    for r in &rules {
        assert!(!r.instruction_text().is_empty());
        if let Some(p) = &r.pattern {
            Regex::new(p).unwrap();
        }
    }
    WardenConfig::default().validate().unwrap();
    let default_cfg = WardenConfig::default();
    let claude: Vec<&str> = default_cfg
        .claude_rules()
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(claude, vec!["FR-001", "FR-004", "SEC-001"]);
}

#[test]
fn fr002_pattern_catches_formatted_sql_but_not_parameters() {
    let re = Regex::new(default_rules()[1].pattern.as_deref().unwrap()).unwrap();
    let hits = [
        r#"frappe.db.sql(f"update `tabECRM Lead` set import_ref='{import_name}' where name='{r}'")"#,
        r#"frappe.db.sql(f'select name from tabUser where email={email}')"#,
        r#"frappe.db.sql( f"select 1")"#,
        r#"frappe.db.sql(rf"select {x}")"#,
        r#"frappe.db.sql(fr"select {x}")"#,
        r#"frappe.db.sql(F"select {x}")"#,
        r#"frappe.db.sql(f"""select {x}""")"#,
        r#"frappe.db.sql("select * from tabLead where name='%s'" % name)"#,
        r#"frappe.db.sql("select * from tabLead where name='{}'".format(name))"#,
        r#"frappe.db.sql("""select %s""" % x)"#,
    ];
    for h in hits {
        assert!(re.is_match(h), "should match: {h}");
    }
    let misses = [
        r#"frappe.db.sql("select name from tabLead where owner=%s", (user,))"#,
        r#"frappe.db.sql("select name from tabLead where owner=%(user)s", {"user": user})"#,
        r#"frappe.db.sql(query, values)"#,
        r#"frappe.db.get_value("Lead", name, "status")"#,
        r#"frappe.db.sql("""select name from tabLead""")"#,
    ];
    for m in misses {
        assert!(!re.is_match(m), "should not match: {m}");
    }
}

#[test]
fn fr007_pattern_catches_commit() {
    let re = Regex::new(default_rules()[3].pattern.as_deref().unwrap()).unwrap();
    assert!(re.is_match("    frappe.db.commit()"));
    assert!(re.is_match("frappe.db.commit( )"));
    assert!(!re.is_match("frappe.db.rollback()"));
    assert!(!re.is_match("frappe.db.commit_later()"));
}

#[test]
fn app_settings_defaults_and_json() {
    let s = AppSettings::default();
    assert_eq!(s.worktree_root, "~/.warden/worktrees");
    assert_eq!(s.data_dir, "~/.warden");
    assert!(!s.posting_paused);
    assert_eq!(s.claude_cli_path, None);
    let back: AppSettings = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(back, s);
    let empty: AppSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(empty, s);
}
