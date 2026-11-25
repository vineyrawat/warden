// OWNER: analysis
//! Regex secret scanner (rule SEC-001), optional gitleaks, and redaction of diff text before it is
//! shown to Claude.
//!
//! Redaction and scanning share one ordered pattern table: specific token formats first (AWS,
//! GitLab, GitHub, Slack, private keys, Frappe …), the generic `api_key/secret/password = "..."`
//! assignment last, so a token is counted once under its most specific kind. A match becomes
//! `[REDACTED:<kind>]`; for assignment-style patterns only the value is replaced, so the key name
//! stays readable (`password = "[REDACTED:generic_secret]"`).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;

use crate::diff::FileDiff;
use crate::error::Result;
use crate::types::{NewFinding, Severity};

/// Rule id used for every secret finding.
pub const SECRET_RULE_ID: &str = "SEC-001";

struct SecretPattern {
    kind: &'static str,
    label: &'static str,
    re: Regex,
    /// Capture group holding the secret value; `None` = the whole match.
    value_group: Option<usize>,
    /// Apply the placeholder filter (skip `changeme`, `${VAR}`, `…EXAMPLE` and friends).
    filter: bool,
    confidence: f32,
}

fn patterns() -> &'static [SecretPattern] {
    static P: OnceLock<Vec<SecretPattern>> = OnceLock::new();
    P.get_or_init(|| {
        let p = |kind, label, re: &str, value_group, filter, confidence| SecretPattern {
            kind,
            label,
            re: Regex::new(re).unwrap_or_else(|e| panic!("secret pattern {kind}: {e}")),
            value_group,
            filter,
            confidence,
        };
        const ASSIGN: &str = r#"["']?\s*(?::=|=>|=|:)\s*"#;
        vec![
            p(
                "private_key",
                "private key",
                r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|\z)",
                None,
                false,
                0.99,
            ),
            p(
                "aws_access_key",
                "AWS access key",
                r"\b(?:AKIA|ASIA|ABIA|ACCA|AGPA|AIDA|AROA|ANPA|ANVA|AIPA)[0-9A-Z]{16}\b",
                None,
                true,
                0.98,
            ),
            p(
                "aws_secret_key",
                "AWS secret key",
                &format!(
                    r#"(?i)\baws_?secret_?(?:access_?)?key\b{ASSIGN}["']?([A-Za-z0-9/+=]{{40}})(?:[^A-Za-z0-9/+=]|$)"#
                ),
                Some(1),
                true,
                0.98,
            ),
            p(
                "gitlab_token",
                "GitLab token",
                r"\b(?:glpat|gldt|glrt|glptt|glsoat|glcbt|gloas|glft|glimt|glagent)-[0-9A-Za-z_\-]{20,}",
                None,
                true,
                0.98,
            ),
            p(
                "github_token",
                "GitHub token",
                r"\b(?:gh[pousr]_[0-9A-Za-z]{36,}|github_pat_[0-9A-Za-z_]{22,})\b",
                None,
                true,
                0.98,
            ),
            p(
                "slack_token",
                "Slack token",
                r"\bxox[abposr]-[0-9A-Za-z\-]{10,}",
                None,
                true,
                0.98,
            ),
            p(
                "slack_webhook",
                "Slack webhook URL",
                r"https://hooks\.slack\.com/services/T[0-9A-Za-z_]+/B[0-9A-Za-z_]+/[0-9A-Za-z_]+",
                None,
                true,
                0.98,
            ),
            p(
                "google_api_key",
                "Google API key",
                r"\bAIza[0-9A-Za-z_\-]{35}\b",
                None,
                true,
                0.97,
            ),
            p(
                "stripe_key",
                "Stripe live key",
                r"\b(?:sk|rk)_live_[0-9A-Za-z]{20,}\b",
                None,
                true,
                0.98,
            ),
            p(
                "jwt",
                "JSON Web Token",
                r"\beyJ[A-Za-z0-9_\-]{10,}\.eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
                None,
                true,
                0.96,
            ),
            // Frappe REST auth header: "token <api_key>:<api_secret>" (15-char hex each).
            p(
                "frappe_api_token",
                "Frappe API key/secret pair",
                r"(?i)\btoken\s+[0-9a-f]{15}:[0-9a-f]{15}\b",
                None,
                true,
                0.97,
            ),
            p(
                "frappe_api_secret",
                "Frappe API secret",
                &format!(r#"(?i)\bapi_secret\b{ASSIGN}["']([^"'\s]{{6,}})["']"#),
                Some(1),
                true,
                0.97,
            ),
            p(
                "url_credentials",
                "password in URL",
                r#"(?i)\b[a-z][a-z0-9+.\-]*://[^/\s:@'"]+:([^/\s:@'"]{3,})@"#,
                Some(1),
                true,
                0.95,
            ),
            p(
                "bearer_token",
                "bearer token",
                r"(?i)\bbearer\s+([A-Za-z0-9\-._~+/]{20,}=*)",
                Some(1),
                true,
                0.95,
            ),
            p(
                "generic_secret",
                "credential",
                &format!(
                    r#"(?i)\b[a-z0-9_.\-]*?(?:api[_-]?key|apikey|api[_-]?secret|secret[_-]?key|client[_-]?secret|auth[_-]?token|access[_-]?token|access[_-]?key|private[_-]?key|secret|passwd|password|pwd|token){ASSIGN}["']([^"'\s]{{8,}})["']"#
                ),
                Some(1),
                true,
                0.95,
            ),
        ]
    })
}

/// True if a matched value is obviously not a real secret.
fn is_placeholder(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() || v.starts_with("[REDACTED") {
        return true;
    }
    let lower = v.to_ascii_lowercase();
    const EXACT: &[&str] = &[
        "password",
        "secret",
        "token",
        "changeme",
        "null",
        "none",
        "undefined",
    ];
    if EXACT.contains(&lower.as_str()) {
        return true;
    }
    const CONTAINS: &[&str] = &[
        "example",
        "changeme",
        "change_me",
        "change-me",
        "placeholder",
        "your_",
        "your-",
        "yourpass",
        "xxxx",
        "****",
        "....",
        "dummy",
        "redacted",
        "fake",
        "sample",
    ];
    if CONTAINS.iter().any(|c| lower.contains(c)) {
        return true;
    }
    // Template / env references: ${VAR}, {{ var }}, <token>, %(name)s, process.env…
    if v.contains(['{', '}', '$', '<', '>']) || v.contains("%(") {
        return true;
    }
    // aaaaaaaa / 00000000
    let mut chars = v.chars();
    let first = chars.next().unwrap_or(' ');
    chars.all(|c| c == first)
}

/// Redacts `text` and returns the kinds of everything replaced (one entry per replacement).
fn redact_collect(text: &str) -> (String, Vec<&'static SecretPattern>) {
    let mut cur = text.to_string();
    let mut hits: Vec<&'static SecretPattern> = Vec::new();
    for pat in patterns() {
        if !pat.re.is_match(&cur) {
            continue;
        }
        let mut out = String::with_capacity(cur.len());
        let mut last = 0usize;
        for caps in pat.re.captures_iter(&cur) {
            let whole = caps.get(0).expect("match");
            let target = match pat.value_group {
                Some(g) => match caps.get(g) {
                    Some(m) => m,
                    None => continue,
                },
                None => whole,
            };
            if pat.filter && is_placeholder(target.as_str()) {
                continue;
            }
            out.push_str(&cur[last..target.start()]);
            out.push_str("[REDACTED:");
            out.push_str(pat.kind);
            out.push(']');
            last = target.end();
            hits.push(pat);
        }
        out.push_str(&cur[last..]);
        cur = out;
    }
    (cur, hits)
}

/// Replaces anything that looks like a secret with a redaction marker; returns (text, count).
pub fn redact_secrets(text: &str) -> (String, usize) {
    let (out, hits) = redact_collect(text);
    (out, hits.len())
}

/// One secret occurrence on an added line.
#[derive(Debug, Clone, PartialEq)]
pub struct SecretHit {
    pub file: String,
    pub line: u32,
    pub kind: String,
    pub label: String,
    /// The line with every secret redacted.
    pub redacted_line: String,
    pub confidence: f32,
    /// "regex" or "gitleaks"
    pub detector: &'static str,
}

/// Regex detection over ADDED lines of non-binary files.
pub fn secret_hits(files: &[FileDiff]) -> Vec<SecretHit> {
    let mut hits = Vec::new();
    for f in files.iter().filter(|f| !f.is_binary && !f.is_deleted) {
        for (line, text) in f.added_lines() {
            let (redacted, found) = redact_collect(text);
            let mut seen = HashSet::new();
            for pat in found {
                if seen.insert(pat.kind) {
                    hits.push(SecretHit {
                        file: f.new_path.clone(),
                        line,
                        kind: pat.kind.to_string(),
                        label: pat.label.to_string(),
                        redacted_line: redacted.clone(),
                        confidence: pat.confidence,
                        detector: "regex",
                    });
                }
            }
        }
    }
    hits
}

/// Groups hits per (file, kind) into SEC-001 findings anchored at the first line; other lines are
/// listed in the body (the fingerprint excludes the line, so one note per file and kind).
pub fn hits_to_findings(hits: &[SecretHit], severity: Severity) -> Vec<NewFinding> {
    let mut groups: BTreeMap<(String, String), Vec<&SecretHit>> = BTreeMap::new();
    let mut order: Vec<(String, String)> = Vec::new();
    for h in hits {
        let key = (h.file.clone(), h.kind.clone());
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(h);
    }
    order
        .into_iter()
        .map(|key| {
            let mut group = groups.remove(&key).unwrap_or_default();
            group.sort_by_key(|h| h.line);
            group.dedup_by_key(|h| h.line);
            let first = group[0];
            let also = if group.len() > 1 {
                let rest: Vec<String> = group[1..].iter().map(|h| h.line.to_string()).collect();
                format!(" (also on line{} {})", if rest.len() > 1 { "s" } else { "" }, rest.join(", "))
            } else {
                String::new()
            };
            let via = if first.detector == "gitleaks" { " (reported by gitleaks)" } else { "" };
            NewFinding {
                file: first.file.clone(),
                line: first.line,
                side: "new".into(),
                severity,
                category: "security".into(),
                rule_id: Some(SECRET_RULE_ID.into()),
                title: format!("Hardcoded {}", first.label),
                body: format!(
                    "Line {} of `{}` adds what looks like a {}{}{}. Anything committed stays in git \
                     history even after it is deleted, so move it to site_config.json, an environment \
                     variable or a secrets store, and rotate the credential.",
                    first.line, first.file, first.label, also, via
                ),
                suggestion: None,
                evidence: Some(evidence_snippet(&first.redacted_line)),
                confidence: group
                    .iter()
                    .map(|h| h.confidence)
                    .fold(0.0_f32, f32::max)
                    .max(0.95),
                source: "rule".into(),
            }
        })
        .collect()
}

/// Trimmed, length-capped line for `evidence` (callers pass already-redacted text).
pub(crate) fn evidence_snippet(text: &str) -> String {
    let t = text.trim();
    const MAX: usize = 240;
    if t.chars().count() > MAX {
        let cut: String = t.chars().take(MAX).collect();
        format!("{cut}…")
    } else {
        t.to_string()
    }
}

/// Scans ADDED lines for hardcoded secrets (findings use rule_id "SEC-001", evidence redacted).
pub fn scan_added_lines(files: &[FileDiff]) -> Vec<NewFinding> {
    hits_to_findings(&secret_hits(files), Severity::Blocker)
}

// ---------------------------------------------------------------------------------------------
// gitleaks
// ---------------------------------------------------------------------------------------------

pub const GITLEAKS_NOT_INSTALLED: &str =
    "gitleaks not installed; secret scan used Warden's built-in patterns only";

/// Runs gitleaks if installed; returns findings + notices ("gitleaks not installed" otherwise).
pub async fn run_gitleaks(
    worktree: &Path,
    files: &[FileDiff],
) -> Result<(Vec<NewFinding>, Vec<String>)> {
    let _ = worktree; // gitleaks scans a mirror of the added lines, not the worktree itself
    let bin = which::which("gitleaks").ok();
    let (hits, notices) = gitleaks_hits(bin.as_deref(), files).await;
    Ok((hits_to_findings(&hits, Severity::Blocker), notices))
}

/// gitleaks over a temp mirror holding only the ADDED lines at their real line numbers (other lines
/// blank), so reported line numbers are new-side line numbers and untouched code is never flagged.
pub(crate) async fn gitleaks_hits(
    bin: Option<&Path>,
    files: &[FileDiff],
) -> (Vec<SecretHit>, Vec<String>) {
    let Some(bin) = bin else {
        return (Vec::new(), vec![GITLEAKS_NOT_INSTALLED.to_string()]);
    };
    let targets: Vec<&FileDiff> = files
        .iter()
        .filter(|f| !f.is_binary && !f.is_deleted && f.added_lines().next().is_some())
        .collect();
    if targets.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let tmp = match tempfile::tempdir() {
        Ok(t) => t,
        Err(e) => return (Vec::new(), vec![format!("gitleaks skipped: temp dir: {e}")]),
    };
    let src = tmp.path().join("src");
    for f in &targets {
        let Some(rel) = safe_relative(&f.new_path) else {
            continue;
        };
        let path = src.join(rel);
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                continue;
            }
        }
        let max = f.added_lines().map(|(n, _)| n).max().unwrap_or(0) as usize;
        let mut lines = vec![String::new(); max];
        for (n, text) in f.added_lines() {
            if n >= 1 {
                lines[n as usize - 1] = text.to_string();
            }
        }
        let _ = std::fs::write(&path, lines.join("\n") + "\n");
    }
    let report = tmp.path().join("report.json");
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("detect")
        .arg("--no-git")
        .arg("--source")
        .arg(&src)
        .arg("--report-format")
        .arg("json")
        .arg("--report-path")
        .arg(&report)
        .arg("--exit-code")
        .arg("0")
        .arg("--no-banner")
        .arg("--redact")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let out = match tokio::time::timeout(Duration::from_secs(90), cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return (Vec::new(), vec![format!("gitleaks failed to start: {e}")]),
        Err(_) => {
            return (
                Vec::new(),
                vec!["gitleaks timed out after 90s; skipped".to_string()],
            )
        }
    };
    let json = match std::fs::read_to_string(&report) {
        Ok(j) => j,
        Err(_) => {
            let err = String::from_utf8_lossy(&out.stderr);
            return (
                Vec::new(),
                vec![format!(
                    "gitleaks produced no report (exit {:?}): {}",
                    out.status.code(),
                    err.lines().last().unwrap_or("").trim()
                )],
            );
        }
    };
    match parse_gitleaks_report(&json, &src, files) {
        Ok(h) => (h, Vec::new()),
        Err(e) => (Vec::new(), vec![format!("gitleaks report unreadable: {e}")]),
    }
}

/// Relative path that cannot escape the mirror dir.
fn safe_relative(p: &str) -> Option<PathBuf> {
    let path = Path::new(p);
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::Normal(s) => out.push(s),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Parses a gitleaks JSON report (array of `{RuleID, Description, File, StartLine, …}`). `File` is
/// absolute or relative to `source_root`; it is mapped back to the diff path. Only lines that are
/// ADDED lines of that file are kept.
pub fn parse_gitleaks_report(
    json: &str,
    source_root: &Path,
    files: &[FileDiff],
) -> std::result::Result<Vec<SecretHit>, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct Leak {
        #[serde(rename = "RuleID", default)]
        rule_id: String,
        #[serde(rename = "Description", default)]
        description: String,
        #[serde(rename = "File", default)]
        file: String,
        #[serde(rename = "StartLine", default)]
        start_line: u32,
    }
    let trimmed = json.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(Vec::new());
    }
    let leaks: Vec<Leak> = serde_json::from_str(trimmed)?;
    let root = source_root.to_string_lossy().to_string();
    let mut hits = Vec::new();
    for leak in leaks {
        let rel = leak
            .file
            .strip_prefix(&root)
            .unwrap_or(&leak.file)
            .trim_start_matches('/')
            .to_string();
        let Some(f) = files.iter().find(|f| f.new_path == rel) else {
            continue;
        };
        let Some((_, text)) = f.added_lines().find(|(n, _)| *n == leak.start_line) else {
            continue;
        };
        let (redacted, _) = redact_secrets(text);
        let label = if leak.description.trim().is_empty() {
            format!("secret ({})", leak.rule_id)
        } else {
            leak.description.trim().to_string()
        };
        hits.push(SecretHit {
            file: rel,
            line: leak.start_line,
            kind: format!("gitleaks:{}", leak.rule_id),
            label,
            redacted_line: redacted,
            confidence: 0.95,
            detector: "gitleaks",
        });
    }
    Ok(hits)
}

/// Merges gitleaks hits into regex hits, dropping gitleaks hits on a line the regex scan already
/// flagged.
pub(crate) fn merge_hits(mut regex_hits: Vec<SecretHit>, extra: Vec<SecretHit>) -> Vec<SecretHit> {
    let seen: HashSet<(String, u32)> = regex_hits
        .iter()
        .map(|h| (h.file.clone(), h.line))
        .collect();
    regex_hits.extend(
        extra
            .into_iter()
            .filter(|h| !seen.contains(&(h.file.clone(), h.line))),
    );
    regex_hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_not_secrets() {
        for v in [
            "changeme",
            "${API_KEY}",
            "{{ token }}",
            "<your token>",
            "xxxxxxxx",
            "AKIAIOSFODNN7EXAMPLE",
        ] {
            assert!(is_placeholder(v), "{v}");
        }
        assert!(!is_placeholder("s3cr3t-Pa55word!"));
    }

    #[test]
    fn safe_relative_rejects_escape() {
        assert!(safe_relative("../etc/passwd").is_none());
        assert!(safe_relative("/abs").is_none());
        assert_eq!(safe_relative("./a/b.py"), Some(PathBuf::from("a/b.py")));
    }
}
