// OWNER: analysis
//! Built-in regex rules (Frappe set) over ADDED lines, plus semgrep rules when semgrep is installed.
//!
//! Findings are grouped per (file, rule): one finding anchored at the first matching added line,
//! with any further lines listed in the body (the fingerprint excludes the line number, so two
//! findings for the same file and rule would collide anyway).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use regex::Regex;

use crate::config::{RuleConfig, RuleKind};
use crate::diff::{ChangedIndex, FileDiff, PathMatcher};
use crate::error::{Result, WardenError};
use crate::types::{NewFinding, RuleMatch, RuleTestResult};

use super::secrets_scan::{evidence_snippet, redact_secrets};

/// Confidence given to deterministic regex hits.
pub const REGEX_CONFIDENCE: f32 = 0.97;
/// Confidence given to semgrep hits.
pub const SEMGREP_CONFIDENCE: f32 = 0.95;

/// One rule hit on an added line.
#[derive(Debug, Clone)]
struct Hit {
    line: u32,
    text: String,
    message: Option<String>,
}

/// Applies every `RuleKind::Regex` rule to added lines (respecting `applies_to`).
/// An invalid pattern or glob is an error (see [`regex_rule_findings`] for the lenient variant).
pub fn run_regex_rules(files: &[FileDiff], rules: &[RuleConfig]) -> Result<Vec<NewFinding>> {
    let mut notices = Vec::new();
    let findings = regex_rule_findings(files, rules, &mut notices);
    if let Some(first) = notices.into_iter().next() {
        return Err(WardenError::Config(first));
    }
    Ok(findings)
}

/// Lenient variant used by `run_checks`: rules that fail to compile are skipped with a notice.
pub fn regex_rule_findings(
    files: &[FileDiff],
    rules: &[RuleConfig],
    notices: &mut Vec<String>,
) -> Vec<NewFinding> {
    let mut out = Vec::new();
    for rule in rules.iter().filter(|r| r.kind() == RuleKind::Regex) {
        let Some(pattern) = rule.pattern.as_deref() else {
            continue;
        };
        let re = match Regex::new(pattern) {
            Ok(re) => re,
            Err(e) => {
                notices.push(format!("rule {} skipped: invalid pattern: {e}", rule.id));
                continue;
            }
        };
        let matcher = match applies_matcher(rule) {
            Ok(m) => m,
            Err(e) => {
                notices.push(format!(
                    "rule {} skipped: invalid applies_to glob: {e}",
                    rule.id
                ));
                continue;
            }
        };
        for f in files.iter().filter(|f| !f.is_binary && !f.is_deleted) {
            if let Some(m) = &matcher {
                if !m.is_match(&f.new_path) {
                    continue;
                }
            }
            let hits: Vec<Hit> = f
                .added_lines()
                .filter(|(_, text)| re.is_match(text))
                .map(|(line, text)| Hit {
                    line,
                    text: text.to_string(),
                    message: None,
                })
                .collect();
            if let Some(finding) =
                hits_to_finding(rule, &f.new_path, &hits, "regex", REGEX_CONFIDENCE)
            {
                out.push(finding);
            }
        }
    }
    out
}

/// Lines of pasted code matched by `pattern` (1-based line numbers). Invalid regex is an error.
pub fn test_pattern_on_code(pattern: &str, code: &str) -> Result<Vec<RuleMatch>> {
    let re =
        Regex::new(pattern).map_err(|e| WardenError::Config(format!("invalid pattern: {e}")))?;
    Ok(code
        .lines()
        .enumerate()
        .filter(|(_, l)| re.is_match(l))
        .map(|(i, l)| RuleMatch {
            file: String::new(),
            line: i as u32 + 1,
            text: l.to_string(),
        })
        .take(500)
        .collect())
}

/// What a rule would flag in a run's diff. Only pattern rules can be tested this way; Claude and
/// semgrep rules get a note instead.
pub fn test_rule_on_files(rule: &RuleConfig, files: &[FileDiff]) -> Result<RuleTestResult> {
    match rule.kind() {
        RuleKind::Claude => {
            return Ok(RuleTestResult {
                note: Some("Claude rules are checked by Claude during the next review.".into()),
                ..Default::default()
            })
        }
        RuleKind::Semgrep => {
            return Ok(RuleTestResult {
                note: Some(
                    "Semgrep rules run during the next review (if semgrep is installed).".into(),
                ),
                ..Default::default()
            })
        }
        RuleKind::Regex => {}
    }
    let pattern = rule.pattern.as_deref().unwrap_or_default();
    let re =
        Regex::new(pattern).map_err(|e| WardenError::Config(format!("invalid pattern: {e}")))?;
    let matcher = applies_matcher(rule)
        .map_err(|e| WardenError::Config(format!("invalid applies_to glob: {e}")))?;
    let mut out = RuleTestResult::default();
    for f in files.iter().filter(|f| !f.is_binary && !f.is_deleted) {
        if let Some(m) = &matcher {
            if !m.is_match(&f.new_path) {
                continue;
            }
        }
        out.files_checked += 1;
        out.matches.extend(
            f.added_lines()
                .filter(|(_, t)| re.is_match(t))
                .map(|(line, text)| RuleMatch {
                    file: f.new_path.clone(),
                    line,
                    text: text.to_string(),
                }),
        );
    }
    out.matches.truncate(500);
    Ok(out)
}

fn applies_matcher(rule: &RuleConfig) -> std::result::Result<Option<PathMatcher>, globset::Error> {
    match rule.applies_to.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(g) => PathMatcher::new(&[g]).map(Some),
    }
}

/// Built-in explanation for a rule id, used as the finding body (custom rules use their instruction).
fn builtin_explanation(id: &str) -> Option<&'static str> {
    match id {
        "FR-002" => Some(
            "builds SQL with string formatting before passing it to frappe.db.sql, so any request \
             value in the string is injectable. Pass values as parameters \
             (frappe.db.sql(query, values)) or use frappe.qb / frappe.db.set_value.",
        ),
        "FR-007" => Some(
            "calls frappe.db.commit() explicitly. Frappe commits at the end of each request, and an \
             explicit commit inside a whitelisted method makes partial writes survive later errors. \
             Remove it unless this is a background job that commits in batches on purpose.",
        ),
        _ => None,
    }
}

/// Category from the rule's id/title keywords.
pub fn category_for_rule(rule: &RuleConfig) -> String {
    let t = format!("{} {}", rule.id, rule.title).to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if t.starts_with("sec")
        || has(&[
            "sql",
            "secret",
            "permission",
            "inject",
            "xss",
            "csrf",
            "auth",
            "password",
            "token",
        ])
    {
        "security".into()
    } else if has(&["perf", "n+1", "slow", "loop", "cache"]) {
        "performance".into()
    } else {
        "correctness".into()
    }
}

fn hits_to_finding(
    rule: &RuleConfig,
    file: &str,
    hits: &[Hit],
    via: &str,
    confidence: f32,
) -> Option<NewFinding> {
    let first = hits.first()?;
    let mut lines: Vec<u32> = hits.iter().map(|h| h.line).collect();
    lines.sort_unstable();
    lines.dedup();
    let also = if lines.len() > 1 {
        let rest: Vec<String> = lines[1..].iter().map(|l| l.to_string()).collect();
        format!(
            " (also on line{} {})",
            if rest.len() > 1 { "s" } else { "" },
            rest.join(", ")
        )
    } else {
        String::new()
    };
    let (snippet, _) = redact_secrets(first.text.trim());
    let snippet = evidence_snippet(&snippet);
    let explanation = match (builtin_explanation(&rule.id), &first.message) {
        (_, Some(msg)) if !msg.trim().is_empty() => format!("matches {}: {}", rule.id, msg.trim()),
        (Some(e), _) => e.to_string(),
        (None, _) => {
            let detail = rule
                .instruction
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| format!(" {}", s.trim()))
                .unwrap_or_default();
            format!("matches rule {} ({}).{}", rule.id, rule.title, detail)
        }
    };
    let body = format!("Line {}{} {}", first.line, also, explanation);
    let evidence = match via {
        "semgrep" => format!(
            "semgrep rule {} matched `{}` (line {})",
            rule.id, snippet, first.line
        ),
        _ => format!(
            "added line {} matches pattern `{}`: `{}`",
            first.line,
            rule.pattern.as_deref().unwrap_or(""),
            snippet
        ),
    };
    Some(NewFinding {
        file: file.to_string(),
        line: first.line,
        side: "new".into(),
        severity: rule.severity,
        category: category_for_rule(rule),
        rule_id: Some(rule.id.clone()),
        title: rule.title.clone(),
        body,
        suggestion: None,
        evidence: Some(evidence),
        confidence,
        source: "rule".into(),
    })
}

// ---------------------------------------------------------------------------------------------
// semgrep
// ---------------------------------------------------------------------------------------------

/// Runs `RuleKind::Semgrep` rules if semgrep is installed; returns findings + notices.
pub async fn run_semgrep_rules(
    worktree: &Path,
    files: &[FileDiff],
    rules: &[RuleConfig],
) -> Result<(Vec<NewFinding>, Vec<String>)> {
    let bin = which::which("semgrep").ok();
    Ok(run_semgrep_with(bin.as_deref(), worktree, files, rules).await)
}

/// Same as [`run_semgrep_rules`] with an explicit binary (`None` = not installed).
pub async fn run_semgrep_with(
    bin: Option<&Path>,
    worktree: &Path,
    files: &[FileDiff],
    rules: &[RuleConfig],
) -> (Vec<NewFinding>, Vec<String>) {
    let semgrep_rules: Vec<&RuleConfig> = rules
        .iter()
        .filter(|r| r.kind() == RuleKind::Semgrep)
        .collect();
    if semgrep_rules.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let Some(bin) = bin else {
        let ids: Vec<&str> = semgrep_rules.iter().map(|r| r.id.as_str()).collect();
        return (
            Vec::new(),
            vec![format!("semgrep not installed; skipped {}", ids.join(", "))],
        );
    };
    let index = ChangedIndex::from_files(files);
    let mut findings = Vec::new();
    let mut notices = Vec::new();
    for rule in semgrep_rules {
        let Some(cfg_path) = rule.semgrep.as_deref() else {
            continue;
        };
        let cfg_path = resolve_rule_path(worktree, cfg_path);
        if !cfg_path.exists() {
            notices.push(format!(
                "semgrep rule file {} for {} not found; skipped",
                cfg_path.display(),
                rule.id
            ));
            continue;
        }
        let matcher = match applies_matcher(rule) {
            Ok(m) => m,
            Err(e) => {
                notices.push(format!(
                    "rule {} skipped: invalid applies_to glob: {e}",
                    rule.id
                ));
                continue;
            }
        };
        let targets: Vec<&str> = files
            .iter()
            .filter(|f| !f.is_binary && !f.is_deleted && f.added_lines().next().is_some())
            .filter(|f| matcher.as_ref().is_none_or(|m| m.is_match(&f.new_path)))
            .filter(|f| worktree.join(&f.new_path).is_file())
            .map(|f| f.new_path.as_str())
            .collect();
        if targets.is_empty() {
            continue;
        }
        let mut cmd = tokio::process::Command::new(bin);
        cmd.current_dir(worktree)
            .arg("scan")
            .arg("--config")
            .arg(&cfg_path)
            .arg("--json")
            .arg("--quiet")
            .arg("--disable-version-check")
            .arg("--metrics=off")
            .arg("--")
            .args(&targets)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let out = match tokio::time::timeout(Duration::from_secs(180), cmd.output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                notices.push(format!("semgrep failed to start for {}: {e}", rule.id));
                continue;
            }
            Err(_) => {
                notices.push(format!("semgrep timed out for {}; skipped", rule.id));
                continue;
            }
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        match parse_semgrep_json(&stdout, rule, &index, files) {
            Ok((mut f, errs)) => {
                findings.append(&mut f);
                notices.extend(errs);
            }
            Err(e) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                notices.push(format!(
                    "semgrep output unreadable for {} ({e}): {}",
                    rule.id,
                    stderr.lines().last().unwrap_or("").trim()
                ));
            }
        }
    }
    (findings, notices)
}

fn resolve_rule_path(worktree: &Path, p: &str) -> PathBuf {
    let path = Path::new(p.trim());
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        worktree.join(path)
    }
}

/// Parses `semgrep --json` output for one Warden rule; keeps only results on ADDED lines.
/// Returns (findings, notices from semgrep's `errors` array).
pub fn parse_semgrep_json(
    json: &str,
    rule: &RuleConfig,
    index: &ChangedIndex,
    files: &[FileDiff],
) -> std::result::Result<(Vec<NewFinding>, Vec<String>), serde_json::Error> {
    #[derive(serde::Deserialize, Default)]
    struct Out {
        #[serde(default)]
        results: Vec<Res>,
        #[serde(default)]
        errors: Vec<serde_json::Value>,
    }
    #[derive(serde::Deserialize)]
    struct Res {
        path: String,
        start: Pos,
        #[serde(default)]
        extra: Extra,
    }
    #[derive(serde::Deserialize)]
    struct Pos {
        line: u32,
    }
    #[derive(serde::Deserialize, Default)]
    struct Extra {
        #[serde(default)]
        message: String,
        #[serde(default)]
        lines: String,
    }
    let out: Out = serde_json::from_str(json.trim())?;
    let mut per_file: BTreeMap<String, Vec<Hit>> = BTreeMap::new();
    let mut seen = HashSet::new();
    for r in out.results {
        let path = r.path.trim_start_matches("./").to_string();
        let Some(key) = index.resolve(&path).map(str::to_string) else {
            continue;
        };
        if !index.is_changed(&key, r.start.line) || !seen.insert((key.clone(), r.start.line)) {
            continue;
        }
        let text = if r.extra.lines.trim().is_empty() || r.extra.lines.trim() == "requires login" {
            files
                .iter()
                .find(|f| f.new_path == key)
                .and_then(|f| f.added_lines().find(|(n, _)| *n == r.start.line))
                .map(|(_, t)| t.to_string())
                .unwrap_or_default()
        } else {
            r.extra.lines.lines().next().unwrap_or("").to_string()
        };
        per_file.entry(key).or_default().push(Hit {
            line: r.start.line,
            text,
            message: Some(r.extra.message),
        });
    }
    let findings = per_file
        .into_iter()
        .filter_map(|(file, hits)| {
            hits_to_finding(rule, &file, &hits, "semgrep", SEMGREP_CONFIDENCE)
        })
        .collect();
    let notices = out
        .errors
        .iter()
        .take(3)
        .map(|e| {
            let msg = e
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            format!(
                "semgrep reported an error for {}: {}",
                rule.id,
                msg.lines().next().unwrap_or("")
            )
        })
        .collect();
    Ok((findings, notices))
}

#[cfg(test)]
mod rule_test_tests {
    use super::*;

    fn rule(pattern: Option<&str>, applies_to: Option<&str>) -> RuleConfig {
        RuleConfig {
            id: "CR-001".into(),
            title: "No print".into(),
            applies_to: applies_to.map(Into::into),
            instruction: None,
            pattern: pattern.map(Into::into),
            semgrep: None,
            severity: crate::types::Severity::Minor,
        }
    }

    #[test]
    fn pasted_code_matches_by_line() {
        let m = test_pattern_on_code(
            r"^\s*print\(",
            "def f(x):\n    print(x)\n    return x\nprint(1)",
        )
        .unwrap();
        assert_eq!(m.iter().map(|m| m.line).collect::<Vec<_>>(), vec![2, 4]);
        assert!(test_pattern_on_code("(", "x").is_err());
    }

    #[test]
    fn claude_rules_get_a_note() {
        let r = test_rule_on_files(&rule(None, None), &[]).unwrap();
        assert!(r.note.is_some() && r.matches.is_empty());
    }

    #[test]
    fn diff_matches_respect_applies_to() {
        let diff = "diff --git a/a.py b/a.py\n--- a/a.py\n+++ b/a.py\n@@ -1,1 +1,3 @@\n x = 1\n+print(x)\n+y = 2\ndiff --git a/b.js b/b.js\n--- a/b.js\n+++ b/b.js\n@@ -1,0 +1,1 @@\n+print(x)\n";
        let files = crate::diff::parse_unified(diff);
        let r = test_rule_on_files(&rule(Some(r"print\("), Some("**/*.py")), &files).unwrap();
        assert_eq!(r.files_checked, 1);
        assert_eq!(r.matches.len(), 1);
        assert_eq!((r.matches[0].file.as_str(), r.matches[0].line), ("a.py", 2));
    }
}
