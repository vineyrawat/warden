// OWNER: analysis
//! Deterministic checks over the diff: built-in regex rules, semgrep (optional), secret scanning
//! (regex + gitleaks if installed). Missing tools are reported as "not installed", never a crash.

pub mod rules;
pub mod secrets_scan;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::WardenConfig;
use crate::diff::FileDiff;
use crate::error::Result;
use crate::types::{NewFinding, Severity, ToolStatus};

pub use secrets_scan::redact_secrets;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CheckReport {
    /// Findings with `source = "rule"`; they still go through the guardrails.
    pub findings: Vec<NewFinding>,
    /// Human-readable notes, e.g. "semgrep not installed; skipped FR-003".
    pub notices: Vec<String>,
    /// Status of the optional tools the checks use (semgrep, gitleaks).
    pub tools: Vec<ToolStatus>,
}

/// Runs every deterministic check over the ADDED lines of `files`:
/// 1. regex rules (`pattern`), respecting `applies_to`;
/// 2. semgrep rules (`semgrep`), only when semgrep is installed — otherwise a notice;
/// 3. the secret scanner (always on; SEC-001 severity comes from the config's SEC-001 rule when
///    present), plus gitleaks when installed.
///
/// Never fails because a tool is missing or a rule is broken; those become notices.
pub async fn run_checks(
    worktree: &Path,
    files: &[FileDiff],
    cfg: &WardenConfig,
) -> Result<CheckReport> {
    let tools =
        tokio::task::spawn_blocking(|| vec![detect_tool("semgrep"), detect_tool("gitleaks")])
            .await?;
    let semgrep_bin = tool_path(&tools, "semgrep");
    let gitleaks_bin = tool_path(&tools, "gitleaks");
    run_checks_with(
        worktree,
        files,
        cfg,
        semgrep_bin.as_deref(),
        gitleaks_bin.as_deref(),
        tools,
    )
    .await
}

fn tool_path(tools: &[ToolStatus], name: &str) -> Option<PathBuf> {
    tools
        .iter()
        .find(|t| t.name == name && t.installed)
        .and_then(|_| which::which(name).ok())
}

/// [`run_checks`] with explicit tool binaries (`None` = not installed); used by tests.
pub async fn run_checks_with(
    worktree: &Path,
    files: &[FileDiff],
    cfg: &WardenConfig,
    semgrep: Option<&Path>,
    gitleaks: Option<&Path>,
    tools: Vec<ToolStatus>,
) -> Result<CheckReport> {
    let mut report = CheckReport {
        tools,
        ..Default::default()
    };

    // 1. regex rules
    let mut regex_findings = rules::regex_rule_findings(files, &cfg.rules, &mut report.notices);
    report.findings.append(&mut regex_findings);

    // 2. semgrep rules
    let (mut sg_findings, sg_notices) =
        rules::run_semgrep_with(semgrep, worktree, files, &cfg.rules).await;
    report.findings.append(&mut sg_findings);
    report.notices.extend(sg_notices);

    // 3. secrets
    let severity = cfg
        .rule(secrets_scan::SECRET_RULE_ID)
        .map(|r| r.severity)
        .unwrap_or(Severity::Blocker);
    let regex_hits = secrets_scan::secret_hits(files);
    let (gl_hits, gl_notices) = secrets_scan::gitleaks_hits(gitleaks, files).await;
    report.notices.extend(gl_notices);
    let hits = secrets_scan::merge_hits(regex_hits, gl_hits);
    report
        .findings
        .extend(secrets_scan::hits_to_findings(&hits, severity));

    Ok(report)
}

// ---------------------------------------------------------------------------------------------
// Tool detection
// ---------------------------------------------------------------------------------------------

/// Tools reported by [`detect_tools`], in display order.
pub const KNOWN_TOOLS: [&str; 8] = [
    "semgrep",
    "gitleaks",
    "gitlab-ci-local",
    "act",
    "docker",
    "rg",
    "git",
    "claude",
];

/// How long `<tool> --version` may take before we give up (installed, version unknown).
const VERSION_TIMEOUT: Duration = Duration::from_secs(3);

/// Detects: semgrep gitleaks gitlab-ci-local act docker rg git claude (in parallel).
pub fn detect_tools() -> Vec<ToolStatus> {
    std::thread::scope(|s| {
        let handles: Vec<_> = KNOWN_TOOLS
            .iter()
            .map(|name| s.spawn(move || detect_tool(name)))
            .collect();
        handles
            .into_iter()
            .zip(KNOWN_TOOLS.iter())
            .map(|(h, name)| {
                h.join().unwrap_or_else(|_| ToolStatus {
                    name: name.to_string(),
                    installed: false,
                    version: None,
                })
            })
            .collect()
    })
}

/// `which <name>` + `<name> --version` with a short timeout. Never panics.
pub fn detect_tool(name: &str) -> ToolStatus {
    match which::which(name) {
        Ok(path) => ToolStatus {
            name: name.to_string(),
            installed: true,
            version: tool_version(&path, VERSION_TIMEOUT),
        },
        Err(_) => ToolStatus {
            name: name.to_string(),
            installed: false,
            version: None,
        },
    }
}

/// Runs `<bin> --version`, killing it after `timeout`; returns the parsed version.
pub fn tool_version(bin: &Path, timeout: Duration) -> Option<String> {
    let mut child = Command::new(bin)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => return None,
        }
    }
    let out = child.wait_with_output().ok()?;
    let text = if out.stdout.iter().any(|b| !b.is_ascii_whitespace()) {
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else {
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    parse_version(&text)
}

/// Extracts a version from `--version` output: the first `x.y[.z…]` token of the first non-empty
/// line, else that whole line (trimmed, capped).
pub fn parse_version(output: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"\bv?(\d+\.\d+(?:\.\d+)*(?:[-+][0-9A-Za-z.\-]+)?)").expect("version regex")
    });
    let line = output.lines().map(str::trim).find(|l| !l.is_empty())?;
    if let Some(c) = re.captures(line) {
        return Some(c[1].trim_end_matches(['.', ',']).to_string());
    }
    Some(line.chars().take(80).collect())
}
