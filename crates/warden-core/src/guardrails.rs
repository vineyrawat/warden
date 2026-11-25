// OWNER: analysis
//! Finding validation & posting policy. Every finding (Claude, rule, CI) passes through
//! `validate_new_finding` before it is stored.
//!
//! Checks, in order (the first failure is the rejection reason, worded so Claude can fix and retry):
//! 1. schema — file/title/body/category non-empty, side `new`|`old`, line ≥ 1, confidence in 0..=1
//! 2. grounding — a `rule_id` or non-empty `evidence`
//! 3. tone — see [`tone_check`] (hedging is allowed when `evidence` is given)
//! 4. anchor — file in the diff; side `new` needs an ADDED line, side `old` a DELETED line
//! 5. nits — rejected when `review.nits` is false
//! 6. severity floor — below `review.severity_floor`
//! 7. confidence — below `review.min_confidence` (deterministic `source: "rule"` findings exempt)
//! 8. duplicate — fingerprint already stored/posted for this MR
//! 9. near-duplicate — body ≈ an existing discussion note (token Jaccard ≥ 0.8, or ≥ 0.9 containment)

use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;
use sha2::{Digest, Sha256};

use crate::config::WardenConfig;
use crate::diff::ChangedIndex;
use crate::types::{DiffPosition, Finding, FindingStatus, NewFinding, PostingMode, Severity};

pub struct GuardrailInput<'a> {
    pub cfg: &'a WardenConfig,
    pub index: &'a ChangedIndex,
    /// Fingerprints already stored/posted for this MR (dedupe).
    pub existing_fingerprints: &'a HashSet<String>,
    /// Bodies of existing MR discussion notes (dedupe against humans/previous runs).
    pub existing_note_bodies: &'a [String],
    /// (base_sha, start_sha, head_sha)
    pub shas: (&'a str, &'a str, &'a str),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Verdict {
    pub keep: bool,
    /// Why it was dropped (or a soft note when kept).
    pub reason: Option<String>,
    /// Eligible for automatic posting under the configured mode / auto_post policy.
    pub auto_post: bool,
    pub position: Option<DiffPosition>,
}

impl Verdict {
    fn reject(reason: impl Into<String>) -> Self {
        Verdict {
            keep: false,
            reason: Some(reason.into()),
            auto_post: false,
            position: None,
        }
    }
}

/// Reason used for budget overflow (see [`apply_budget`]).
pub const OVER_BUDGET: &str = "over note budget";
/// Reason used for exact duplicates.
pub const DUPLICATE: &str = "duplicate of existing thread";
/// Jaccard similarity at or above which a body counts as a near-duplicate of an existing note.
pub const NEAR_DUP_JACCARD: f64 = 0.8;
/// Containment (|A∩B| / |smaller|) at or above which a body counts as a near-duplicate, when the
/// smaller side has at least [`NEAR_DUP_MIN_TOKENS`] tokens (catches notes wrapped in a badge/footer).
pub const NEAR_DUP_CONTAINMENT: f64 = 0.9;
pub const NEAR_DUP_MIN_TOKENS: usize = 8;

/// schema / anchor / confidence / severity floor / duplicate / tone checks.
pub fn validate_new_finding(f: &NewFinding, input: &GuardrailInput) -> Verdict {
    // 1. schema
    for (name, value) in [
        ("file", &f.file),
        ("title", &f.title),
        ("body", &f.body),
        ("category", &f.category),
    ] {
        if value.trim().is_empty() {
            return Verdict::reject(format!("missing required field: {name}"));
        }
    }
    let side = f.side.trim().to_ascii_lowercase();
    if side != "new" && side != "old" {
        return Verdict::reject(format!("invalid side {:?}: use \"new\" or \"old\"", f.side));
    }
    if f.line == 0 {
        return Verdict::reject("line must be 1 or greater");
    }
    if !f.confidence.is_finite() || !(0.0..=1.0).contains(&f.confidence) {
        return Verdict::reject(format!(
            "confidence must be between 0 and 1, got {}",
            f.confidence
        ));
    }

    // 2. grounding
    let has_rule = f.rule_id.as_deref().is_some_and(|r| !r.trim().is_empty());
    let has_evidence = f.evidence.as_deref().is_some_and(|e| !e.trim().is_empty());
    if !has_rule && !has_evidence {
        return Verdict::reject(
            "needs a rule_id or evidence: cite the rule or quote the code that proves the issue",
        );
    }

    // 3. tone
    if let Some(reason) = tone_check_with(&f.body, has_evidence) {
        return Verdict::reject(reason);
    }

    // 4. anchor
    let index = input.index;
    let Some(path) = index.resolve(&f.file) else {
        return Verdict::reject(format!(
            "file not in diff: {} (only comment on files this MR changes)",
            f.file
        ));
    };
    if side == "new" && !index.is_changed(path, f.line) {
        let hint = if index.in_hunk(path, f.line) {
            "is an unchanged context line"
        } else {
            "is outside the diff"
        };
        return Verdict::reject(format!(
            "line {} of {} {hint}; only comment on added lines (side \"new\") or deleted lines (side \"old\")",
            f.line, f.file
        ));
    }
    if side == "old" && !index.is_deleted_line(path, f.line) {
        return Verdict::reject(format!(
            "old-side line {} of {} is not a deleted line",
            f.line, f.file
        ));
    }

    // 5-7. nits / severity floor / confidence
    if let Some(reason) = policy_check(input.cfg, f.severity, f.confidence, &f.source) {
        return Verdict::reject(reason);
    }

    // 8. exact duplicate
    let fp = fingerprint(path, &f.title, f.rule_id.as_deref());
    let fp_raw = fingerprint(&f.file, &f.title, f.rule_id.as_deref());
    if input.existing_fingerprints.contains(&fp) || input.existing_fingerprints.contains(&fp_raw) {
        return Verdict::reject(DUPLICATE);
    }

    // 9. near duplicate of an existing note
    let body_tokens = tokens(&f.body);
    if input
        .existing_note_bodies
        .iter()
        .any(|note| is_near_duplicate(&body_tokens, &tokens(note)))
    {
        return Verdict::reject("near-duplicate of an existing discussion note");
    }

    let position = index.position(path, f.line, &side, input.shas);
    Verdict {
        keep: true,
        reason: None,
        auto_post: auto_post_for(input.cfg, f.severity, f.confidence),
        position,
    }
}

/// Policy checks 5-7 of [`validate_new_finding`] (nits, severity floor, confidence; deterministic
/// `source: "rule"` findings are exempt from the confidence threshold). Returns the rejection
/// reason. Also used by the guardrail live preview to re-classify stored findings.
pub fn policy_check(
    cfg: &WardenConfig,
    severity: Severity,
    confidence: f32,
    source: &str,
) -> Option<String> {
    let review = &cfg.review;
    if severity == Severity::Nit && !review.nits {
        return Some(NITS_DISABLED.to_string());
    }
    if severity < review.severity_floor {
        return Some(format!(
            "severity {} is below the floor ({})",
            severity, review.severity_floor
        ));
    }
    if source != "rule" && confidence < review.min_confidence {
        return Some(format!(
            "confidence {:.2} is below the threshold {:.2}",
            confidence, review.min_confidence
        ));
    }
    None
}

/// True when a stored `guardrail_reason` came from a settings-dependent check (nits, severity
/// floor, confidence, note budget), i.e. the finding may pass under different settings. Other
/// reasons (anchor, schema, tone, duplicates) do not depend on the guardrail settings.
pub fn is_policy_reason(reason: &str) -> bool {
    reason == NITS_DISABLED
        || reason == OVER_BUDGET
        || (reason.starts_with("severity ") && reason.contains("is below the floor"))
        || (reason.starts_with("confidence ") && reason.contains("is below the threshold"))
}

/// Reason used when nits are turned off.
pub const NITS_DISABLED: &str = "nits are disabled for this project";

/// Whether a kept finding may be posted without approval: Draft never, Auto always, Assist when the
/// severity is in `posting.auto_post.severities` and confidence ≥ `posting.auto_post.min_confidence`.
pub fn auto_post_for(cfg: &WardenConfig, severity: Severity, confidence: f32) -> bool {
    match cfg.posting.mode {
        PostingMode::Draft => false,
        PostingMode::Auto => true,
        PostingMode::Assist => {
            cfg.posting.auto_post.severities.contains(&severity)
                && confidence >= cfg.posting.auto_post.min_confidence
        }
    }
}

/// Keeps the top `max_notes` (severity desc, then confidence desc) among non-rejected findings;
/// the rest become `Rejected` with reason "over note budget".
///
/// Findings already `Posted`/`Resolved` cannot be withdrawn: they are kept and use budget first.
/// `Dismissed` and `Rejected` findings are ignored. Order of the vector is unchanged; ties keep
/// their original order.
#[allow(clippy::ptr_arg)] // signature fixed by SPEC
pub fn apply_budget(findings: &mut Vec<Finding>, max_notes: u32) {
    let budget = max_notes as usize;
    let locked = findings
        .iter()
        .filter(|f| matches!(f.status, FindingStatus::Posted | FindingStatus::Resolved))
        .count();
    let mut candidates: Vec<usize> = findings
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(f.status, FindingStatus::Draft | FindingStatus::Approved))
        .map(|(i, _)| i)
        .collect();
    candidates.sort_by(|&a, &b| {
        let (fa, fb) = (&findings[a], &findings[b]);
        fb.severity
            .cmp(&fa.severity)
            .then_with(|| {
                fb.confidence
                    .partial_cmp(&fa.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.cmp(&b))
    });
    let remaining = budget.saturating_sub(locked);
    for &i in candidates.iter().skip(remaining) {
        let f = &mut findings[i];
        f.status = FindingStatus::Rejected;
        f.guardrail_reason = Some(OVER_BUDGET.to_string());
    }
}

/// Fingerprint of a finding with its path resolved against the diff (so `./a.py` and `a.py`, or a
/// renamed file's old path, give the same fingerprint as the stored one).
pub fn fingerprint_for(f: &NewFinding, index: &ChangedIndex) -> String {
    let path = index.resolve(&f.file).unwrap_or(&f.file);
    fingerprint(path, &f.title, f.rule_id.as_deref())
}

/// Lowercase, whitespace collapsed, trimmed.
pub fn normalize_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// sha256 hex of `file + ":" + normalized(title) + ":" + rule_id.unwrap_or("")`.
pub fn fingerprint(file: &str, title: &str, rule_id: Option<&str>) -> String {
    let mut h = Sha256::new();
    h.update(file.as_bytes());
    h.update(b":");
    h.update(normalize_title(title).as_bytes());
    h.update(b":");
    h.update(rule_id.unwrap_or("").as_bytes());
    hex::encode(h.finalize())
}

/// Returns a rejection reason for "LGTM", empty praise, bodies < 20 chars, hedging without evidence.
pub fn tone_check(body: &str) -> Option<String> {
    tone_check_with(body, false)
}

fn praise_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(looks good( to me)?|nice (work|job|change|catch)|great (work|job|change)|well done|good (job|work)|awesome|nicely done|ship it)\b",
        )
        .expect("praise regex")
    })
}

fn lgtm_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\blgtm\b").expect("lgtm regex"))
}

fn hedge_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\b(might|maybe|perhaps|possibly)\b").expect("hedge regex"))
}

/// Concrete grounding inside the body itself: inline code, a code block, or a line reference.
fn body_has_evidence(body: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)`[^`\n]+`|```|\blines? \d+|\bL\d+\b|:\d+\b").expect("evidence regex")
    });
    re.is_match(body)
}

/// [`tone_check`] where `has_evidence` (the finding's `evidence` field) excuses hedging.
pub fn tone_check_with(body: &str, has_evidence: bool) -> Option<String> {
    let t = body.trim();
    if t.is_empty() {
        return Some("body is empty".into());
    }
    if lgtm_re().is_match(t) {
        return Some(
            "approval/\"LGTM\" comments are not findings; describe a concrete problem".into(),
        );
    }
    if t.chars().count() < 20 {
        return Some(
            "body is too short (under 20 characters); explain the problem and the fix".into(),
        );
    }
    let words = t.split_whitespace().count();
    if praise_re().is_match(t) && words < 15 {
        return Some("empty praise is not a finding; describe a concrete problem".into());
    }
    if hedge_re().is_match(t) && !has_evidence && !body_has_evidence(t) {
        return Some(
            "hedged wording (\"might\"/\"maybe\") without evidence; add evidence or state the issue definitively"
                .into(),
        );
    }
    None
}

/// Lowercased alphanumeric tokens (markdown, punctuation and 1-char tokens dropped).
pub fn tokens(text: &str) -> HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| w.chars().count() > 1)
        .map(str::to_string)
        .collect()
}

/// Token Jaccard similarity in 0..=1 (0 when both are empty).
pub fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(b).count() as f64 / union as f64
}

/// Jaccard ≥ 0.8, or containment ≥ 0.9 when the smaller set has ≥ 8 tokens.
pub fn is_near_duplicate(a: &HashSet<String>, b: &HashSet<String>) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if jaccard(a, b) >= NEAR_DUP_JACCARD {
        return true;
    }
    let smaller = a.len().min(b.len());
    if smaller >= NEAR_DUP_MIN_TOKENS {
        let inter = a.intersection(b).count();
        return inter as f64 / smaller as f64 >= NEAR_DUP_CONTAINMENT;
    }
    false
}

/// Text similarity of two bodies (token Jaccard), exposed for UI/debugging.
pub fn similarity(a: &str, b: &str) -> f64 {
    jaccard(&tokens(a), &tokens(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize() {
        assert_eq!(
            normalize_title("  SQL   built\twith\nF-String "),
            "sql built with f-string"
        );
    }

    #[test]
    fn jaccard_basics() {
        let a = tokens("the quick brown fox");
        let b = tokens("The quick, brown fox!");
        assert!((jaccard(&a, &b) - 1.0).abs() < 1e-9);
        assert_eq!(jaccard(&HashSet::new(), &HashSet::new()), 0.0);
    }
}
