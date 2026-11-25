// OWNER: poster
//! The ONLY code path that writes to the forge (hard rule 1). Posts approved / auto-post findings as
//! line notes (fallback to file-level note on position errors), a summary note, the "ai-reviewed"
//! label; updates finding status and audit events; respects max_notes_per_hour and posting_paused.
//!
//! Posting policy (`PostingConfig`: summary_note, max_notes_per_hour) is taken from, in order:
//! 1. [`Poster::with_posting`] (the pipeline passes the merged `.warden.yml` + overrides config), else
//! 2. the project's stored overrides (`project_overrides.posting`) laid over `PostingConfig::default()`.

use std::collections::HashSet;

use chrono::{Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ts_rs::TS;

use crate::checks::redact_secrets;
use crate::config::{MrStatusMode, PostingConfig};
use crate::db::{format_ts, Db};
use crate::error::{Result, WardenError};
use crate::events::EventBus;
use crate::forge::ForgeProvider;
use crate::types::{
    AppEvent, CiJob, CiStatus, Finding, FindingStatus, ForgeKind, MergeRequest, MrReviewStatus,
    Project, ReviewRun, Severity,
};

/// Label added to the MR after Warden posted something.
pub const REVIEWED_LABEL: &str = "ai-reviewed";
/// Scoped labels for `posting.mr_status` (scoped `::` labels replace each other on GitLab Premium).
pub const APPROVED_LABEL: &str = "warden::approved";
pub const CHANGES_LABEL: &str = "warden::changes-requested";

/// Changes requested when a posted or still pending finding (Draft/Approved) has one of the
/// `request_changes_on` severities, or Claude's verdict is "request_changes"; otherwise approved.
/// Dismissed and guardrail-rejected findings do not count.
pub fn decide_mr_status(
    run: &ReviewRun,
    findings: &[Finding],
    request_changes_on: &[Severity],
) -> MrReviewStatus {
    let serious = findings.iter().any(|f| {
        matches!(
            f.status,
            FindingStatus::Posted | FindingStatus::Approved | FindingStatus::Draft
        ) && request_changes_on.contains(&f.severity)
    });
    let verdict_changes = run.verdict.as_deref().map(str::trim) == Some("request_changes");
    if serious || verdict_changes {
        MrReviewStatus::ChangesRequested
    } else {
        MrReviewStatus::Approved
    }
}

/// Prefix of a stored `forge_note_id` that is a plain note, not a resolvable discussion.
pub const PLAIN_NOTE_PREFIX: &str = "note:";

/// Audit event kind used for every forge write attempt.
const AUDIT_POST: &str = "post";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct PostReport {
    /// Findings posted in this call (line notes + file-level fallback notes).
    pub posted: u32,
    /// Of `posted`: posted as a file-level note because the line position was rejected (or missing).
    pub fallback: u32,
    pub failed: u32,
    /// Selected findings not posted: posting paused, over the hourly budget, ineligible status for an
    /// explicitly requested id, or already posted on this MR (same fingerprint).
    pub skipped: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Poster {
    pub events: EventBus,
    /// Explicit posting policy; `None` => project overrides over defaults.
    pub posting: Option<PostingConfig>,
}

impl Poster {
    pub fn new(events: EventBus) -> Self {
        Self {
            events,
            posting: None,
        }
    }

    /// Uses this posting policy instead of reading the project overrides.
    pub fn with_posting(mut self, posting: PostingConfig) -> Self {
        self.posting = Some(posting);
        self
    }

    /// Posting policy for a project (see module docs).
    pub async fn posting_config(&self, db: &Db, project: &Project) -> Result<PostingConfig> {
        if let Some(p) = &self.posting {
            return Ok(p.clone());
        }
        let overrides = db.get_project_overrides(project.id).await?;
        posting_from_overrides(&overrides)
    }

    /// Posts Approved findings of the run (or only `only` ids). On a 400 position error falls back to
    /// a file-level note "**path:line**". Then the summary note (if enabled) and label "ai-reviewed".
    pub async fn post_run(
        &self,
        db: &Db,
        provider: &dyn ForgeProvider,
        project: &Project,
        mr: &MergeRequest,
        run: &ReviewRun,
        only: Option<&[i64]>,
    ) -> Result<PostReport> {
        let posting = self.posting_config(db, project).await?;
        let settings = db.get_settings().await?;
        let mut report = PostReport::default();

        let all = db.list_findings(run.id).await?;

        // ---- select -------------------------------------------------------------------------
        let mut candidates: Vec<Finding> = Vec::new();
        match only {
            Some(ids) => {
                let mut seen = HashSet::new();
                for id in ids {
                    if !seen.insert(*id) {
                        continue;
                    }
                    match all.iter().find(|f| f.id == *id) {
                        Some(f) if is_postable(f.status) => candidates.push(f.clone()),
                        Some(f) => {
                            report.skipped += 1;
                            self.audit_finding(db, run, mr, f, "skipped_status", None, None)
                                .await;
                        }
                        None => {
                            report.skipped += 1;
                            audit_quiet(
                                db,
                                Some(run.id),
                                json!({
                                    "finding_id": id, "run_id": run.id, "mr_iid": mr.iid,
                                    "outcome": "skipped_unknown",
                                }),
                            )
                            .await;
                        }
                    }
                }
            }
            None => candidates.extend(
                all.iter()
                    .filter(|f| f.status == FindingStatus::Approved)
                    .cloned(),
            ),
        }

        // ---- paused -------------------------------------------------------------------------
        if settings.posting_paused {
            report.skipped += candidates.len() as u32;
            audit_quiet(
                db,
                Some(run.id),
                json!({
                    "run_id": run.id, "mr_iid": mr.iid, "outcome": "paused",
                    "finding_ids": candidates.iter().map(|f| f.id).collect::<Vec<_>>(),
                }),
            )
            .await;
            if !candidates.is_empty() {
                self.notice(
                    "warn",
                    format!(
                        "Posting is paused: {} finding(s) on !{} were not posted",
                        candidates.len(),
                        mr.iid
                    ),
                );
            }
            return Ok(report);
        }

        // ---- dedupe against findings already posted on this MR ------------------------------
        let already: HashSet<String> = db
            .posted_findings_for_mr(mr.id)
            .await?
            .into_iter()
            .map(|f| f.fingerprint)
            .collect();
        let mut fresh = Vec::with_capacity(candidates.len());
        let mut batch_fps = HashSet::new();
        for f in candidates {
            if already.contains(&f.fingerprint) || !batch_fps.insert(f.fingerprint.clone()) {
                report.skipped += 1;
                self.audit_finding(db, run, mr, &f, "skipped_duplicate", None, None)
                    .await;
            } else {
                fresh.push(f);
            }
        }
        let mut candidates = fresh;

        // ---- hourly budget ------------------------------------------------------------------
        let hour_ago = format_ts(Utc::now() - ChronoDuration::hours(1));
        let used = db.posts_since(&hour_ago).await?;
        let remaining = posting.max_notes_per_hour.saturating_sub(used) as usize;
        // Most important first so the budget keeps blockers.
        candidates.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then(
                    b.confidence
                        .partial_cmp(&a.confidence)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
                .then(a.id.cmp(&b.id))
        });
        if candidates.len() > remaining {
            let over = candidates.split_off(remaining);
            report.skipped += over.len() as u32;
            for f in &over {
                self.audit_finding(db, run, mr, f, "skipped_budget", None, None)
                    .await;
            }
            self.notice(
                "warn",
                format!(
                    "Hourly note budget ({}) reached: {} finding(s) on !{} were not posted",
                    posting.max_notes_per_hour,
                    over.len(),
                    mr.iid
                ),
            );
        }

        // ---- post each ----------------------------------------------------------------------
        for f in &candidates {
            match self.post_one(provider, project, mr, f).await {
                Ok((note_id, fell_back, line_error)) => {
                    db.update_finding_status(f.id, FindingStatus::Posted, Some(&note_id))
                        .await?;
                    report.posted += 1;
                    let outcome = if fell_back {
                        report.fallback += 1;
                        "fallback"
                    } else {
                        "posted"
                    };
                    self.audit_finding(
                        db,
                        run,
                        mr,
                        f,
                        outcome,
                        Some(&note_id),
                        line_error.as_deref(),
                    )
                    .await;
                }
                Err(e) => {
                    report.failed += 1;
                    self.audit_finding(db, run, mr, f, "failed", None, Some(&e.to_string()))
                        .await;
                }
            }
        }
        if !candidates.is_empty() {
            self.events
                .emit(AppEvent::FindingsUpdated { run_id: run.id });
        }

        // ---- summary note -------------------------------------------------------------------
        let ci = db.list_ci_jobs(run.id).await?;
        let ci_failed = ci.iter().any(|j| ci_is_failure(j.status));
        let mut summary_posted = false;
        if posting.summary_note && (report.posted > 0 || ci_failed) {
            let duplicate_ci_only = report.posted == 0 && summary_already_posted(db, run.id).await;
            if !duplicate_ci_only {
                let posted: Vec<Finding> = db
                    .list_findings(run.id)
                    .await?
                    .into_iter()
                    .filter(|f| f.status == FindingStatus::Posted)
                    .collect();
                let body = Self::format_summary(run, &posted, &ci);
                match provider.post_note(project, mr.iid, &body).await {
                    Ok(note_id) => {
                        summary_posted = true;
                        audit_quiet(
                            db,
                            Some(run.id),
                            json!({
                                "kind": "summary", "run_id": run.id, "mr_iid": mr.iid,
                                "outcome": "posted", "note_id": note_id,
                                "findings": posted.len(), "ci_jobs": ci.len(),
                            }),
                        )
                        .await;
                    }
                    Err(e) => {
                        audit_quiet(
                            db,
                            Some(run.id),
                            json!({
                                "kind": "summary", "run_id": run.id, "mr_iid": mr.iid,
                                "outcome": "failed", "error": e.to_string(),
                            }),
                        )
                        .await;
                    }
                }
            }
        }

        // ---- label (errors ignored) ---------------------------------------------------------
        if report.posted > 0 || summary_posted {
            let outcome = match provider.add_label(project, mr.iid, REVIEWED_LABEL).await {
                Ok(()) => json!({"outcome": "labeled"}),
                Err(e) => json!({"outcome": "label_failed", "error": e.to_string()}),
            };
            let mut payload = json!({"kind": "label", "label": REVIEWED_LABEL,
                                     "run_id": run.id, "mr_iid": mr.iid});
            merge_json(&mut payload, &outcome);
            audit_quiet(db, Some(run.id), payload).await;
        }

        // ---- MR review status (opt-in) -------------------------------------------------------
        if report.posted > 0 || summary_posted {
            self.apply_mr_status(db, provider, project, mr, run, &posting)
                .await;
        }

        Ok(report)
    }

    /// Sets the MR's review status per `posting.mr_status` (no-op when off). Errors are audited
    /// and raised as a notice, never returned: the notes are already posted.
    pub async fn apply_mr_status(
        &self,
        db: &Db,
        provider: &dyn ForgeProvider,
        project: &Project,
        mr: &MergeRequest,
        run: &ReviewRun,
        posting: &PostingConfig,
    ) -> Option<MrReviewStatus> {
        let cfg = &posting.mr_status;
        if cfg.mode == MrStatusMode::Off {
            return None;
        }
        let findings = db.list_findings(run.id).await.ok()?;
        let status = decide_mr_status(run, &findings, &cfg.request_changes_on);
        let (add, remove) = match status {
            MrReviewStatus::Approved => (APPROVED_LABEL, CHANGES_LABEL),
            MrReviewStatus::ChangesRequested => (CHANGES_LABEL, APPROVED_LABEL),
        };
        let mut errors: Vec<String> = Vec::new();
        let use_labels = cfg.mode == MrStatusMode::Labels || project.forge == ForgeKind::Gitlab;
        if cfg.mode == MrStatusMode::Review {
            let body = match status {
                MrReviewStatus::Approved => {
                    "Warden found nothing blocking in this change.".to_string()
                }
                MrReviewStatus::ChangesRequested => format!(
                    "Warden requests changes: see the {} note(s) above.",
                    findings
                        .iter()
                        .filter(|f| matches!(
                            f.status,
                            FindingStatus::Posted | FindingStatus::Resolved
                        ))
                        .count()
                ),
            };
            if let Err(e) = provider
                .set_review_status(project, mr.iid, status, &body)
                .await
            {
                errors.push(e.to_string());
            }
        }
        if use_labels {
            if let Err(e) = provider.remove_label(project, mr.iid, remove).await {
                errors.push(e.to_string());
            }
            // GitLab "review" mode only needs the label for "changes requested" (approval is native).
            let want_label =
                cfg.mode == MrStatusMode::Labels || status == MrReviewStatus::ChangesRequested;
            if want_label {
                if let Err(e) = provider.add_label(project, mr.iid, add).await {
                    errors.push(e.to_string());
                }
            } else if let Err(e) = provider.remove_label(project, mr.iid, add).await {
                errors.push(e.to_string());
            }
        }
        let outcome = if errors.is_empty() { "set" } else { "failed" };
        audit_quiet(
            db,
            Some(run.id),
            json!({
                "kind": "mr_status", "run_id": run.id, "mr_iid": mr.iid,
                "mode": cfg.mode, "status": status, "outcome": outcome, "errors": errors,
            }),
        )
        .await;
        if !errors.is_empty() {
            self.events.emit(AppEvent::Notice {
                level: "warn".into(),
                message: format!(
                    "Could not set the status of !{}: {}",
                    mr.iid,
                    errors.join("; ")
                ),
            });
        }
        Some(status)
    }

    /// Posts one finding. Returns (note id, fell back to file-level note, line-note error if any).
    async fn post_one(
        &self,
        provider: &dyn ForgeProvider,
        project: &Project,
        mr: &MergeRequest,
        f: &Finding,
    ) -> Result<(String, bool, Option<String>)> {
        let body = Self::format_note(f);
        let line_error = match &f.position {
            Some(pos) => match provider.post_line_note(project, mr.iid, &body, pos).await {
                Ok(id) => return Ok((id, false, None)),
                Err(e) if is_position_error(&e) => Some(e.to_string()),
                Err(e) => return Err(e),
            },
            None => Some("finding has no diff position".to_string()),
        };
        let (id, resolvable) = provider
            .post_file_note(project, mr.iid, &Self::format_fallback_note(f))
            .await?;
        let id = if resolvable {
            id
        } else {
            format!("{PLAIN_NOTE_PREFIX}{id}")
        };
        Ok((id, true, line_error))
    }

    /// Resolves discussions of previously posted findings whose fingerprints are now fixed.
    /// Returns the number resolved. Does nothing while posting is paused (it is a forge write).
    pub async fn auto_resolve(
        &self,
        db: &Db,
        provider: &dyn ForgeProvider,
        project: &Project,
        mr: &MergeRequest,
        fixed_fingerprints: &[String],
    ) -> Result<u32> {
        if fixed_fingerprints.is_empty() {
            return Ok(0);
        }
        let settings = db.get_settings().await?;
        if settings.posting_paused {
            audit_quiet(
                db,
                None,
                json!({"kind": "resolve", "mr_iid": mr.iid, "outcome": "paused",
                       "fingerprints": fixed_fingerprints}),
            )
            .await;
            return Ok(0);
        }
        let fixed: HashSet<&str> = fixed_fingerprints.iter().map(String::as_str).collect();
        let mut resolved = 0u32;
        let mut touched_runs = HashSet::new();
        for f in db.posted_findings_for_mr(mr.id).await? {
            if !fixed.contains(f.fingerprint.as_str()) {
                continue;
            }
            let Some(note_id) = f.forge_note_id.clone() else {
                continue;
            };
            let payload_base = json!({
                "kind": "resolve", "finding_id": f.id, "run_id": f.run_id, "mr_iid": mr.iid,
                "discussion_id": note_id, "fingerprint": f.fingerprint,
            });
            if note_id.starts_with(PLAIN_NOTE_PREFIX) {
                // A plain note has no resolve state on the forge: close it on our side only.
                db.update_finding_status(f.id, FindingStatus::Resolved, None)
                    .await?;
                resolved += 1;
                touched_runs.insert(f.run_id);
                let mut p = payload_base;
                merge_json(
                    &mut p,
                    &json!({"outcome": "resolved_locally",
                    "reason": "plain note; not resolvable on the forge"}),
                );
                audit_quiet(db, Some(f.run_id), p).await;
                continue;
            }
            match provider.resolve_discussion(project, mr.iid, &note_id).await {
                Ok(()) => {
                    db.update_finding_status(f.id, FindingStatus::Resolved, None)
                        .await?;
                    resolved += 1;
                    touched_runs.insert(f.run_id);
                    let mut p = payload_base;
                    merge_json(&mut p, &json!({"outcome": "resolved"}));
                    audit_quiet(db, Some(f.run_id), p).await;
                }
                Err(e) => {
                    let mut p = payload_base;
                    merge_json(
                        &mut p,
                        &json!({"outcome": "failed", "error": e.to_string()}),
                    );
                    audit_quiet(db, Some(f.run_id), p).await;
                }
            }
        }
        for run_id in touched_runs {
            self.events.emit(AppEvent::FindingsUpdated { run_id });
        }
        Ok(resolved)
    }

    /// Severity badge, title, body, ```suggestion block, footer "— Warden · rule FR-002 · confidence 0.98".
    /// Secrets are redacted from the whole note (title, body, suggestion): the text is model output
    /// that a prompt injection could steer into quoting credentials.
    pub fn format_note(f: &Finding) -> String {
        redact_secrets(&Self::format_note_raw(f)).0
    }

    fn format_note_raw(f: &Finding) -> String {
        let mut out = format!(
            "{} · **{}**\n\n{}\n",
            severity_badge(f.severity),
            one_line(&f.title),
            f.body.trim()
        );
        if let Some(s) = f.suggestion.as_deref() {
            let s = s.trim_end_matches(['\n', '\r']);
            if !s.trim().is_empty() {
                let fence = fence_for(s);
                if is_single_line(s) && f.side != "old" {
                    out.push_str(&format!("\n{fence}suggestion:-0+0\n{s}\n{fence}\n"));
                } else {
                    out.push_str(&format!("\n{fence}\n{s}\n{fence}\n"));
                }
            }
        }
        out.push('\n');
        out.push_str(&note_footer(f));
        out
    }

    /// The note as `post_one` sends it: the line note, or the file-level fallback when the finding
    /// has no diff position (a line note the forge rejects also falls back at post time).
    pub fn preview_note(f: &Finding) -> String {
        if f.position.is_some() {
            Self::format_note(f)
        } else {
            Self::format_fallback_note(f)
        }
    }

    /// File-level note used when the line position is rejected: "**path:line**" + the note, with the
    /// suggestion shown as a plain code block (suggestions only apply on diff notes).
    pub fn format_fallback_note(f: &Finding) -> String {
        let note = Self::format_note(f).replace("suggestion:-0+0\n", "\n");
        redact_secrets(&format!("**{}:{}**\n\n{}", f.file, f.line, note)).0
    }

    /// Summary note with findings table + CI table (secrets redacted, see [`Poster::format_note`]).
    pub fn format_summary(run: &ReviewRun, findings: &[Finding], ci: &[CiJob]) -> String {
        redact_secrets(&Self::format_summary_raw(run, findings, ci)).0
    }

    fn format_summary_raw(run: &ReviewRun, findings: &[Finding], ci: &[CiJob]) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "### Warden review: {}\n\n",
            verdict_text(run, findings)
        ));
        if let Some(summary) = run.summary.as_deref().map(str::trim) {
            if !summary.is_empty() {
                out.push_str(summary);
                out.push_str("\n\n");
            }
        }

        let mut sorted: Vec<&Finding> = findings.iter().collect();
        sorted.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then(a.file.cmp(&b.file))
                .then(a.line.cmp(&b.line))
        });
        if sorted.is_empty() {
            out.push_str("No findings posted.\n\n");
        } else {
            out.push_str(&format!("**Findings ({})**\n\n", sorted.len()));
            out.push_str("| Severity | File | Title |\n|---|---|---|\n");
            for f in sorted {
                out.push_str(&format!(
                    "| {} | `{}:{}` | {} |\n",
                    severity_badge(f.severity),
                    cell(&f.file),
                    f.line,
                    cell(&f.title)
                ));
            }
            out.push('\n');
        }

        if !ci.is_empty() {
            out.push_str("**Local CI**\n\n");
            out.push_str("| Job | Status | Duration |\n|---|---|---|\n");
            for j in ci {
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    cell(&j.name),
                    ci_status_text(j.status),
                    format_duration(j.duration_ms)
                ));
            }
            out.push('\n');
        }

        out.push_str("---\n");
        out.push_str(&format!(
            "Reviewed by Warden with Claude Code · head {}",
            short_sha(&run.head_sha)
        ));
        out
    }

    fn notice(&self, level: &str, message: String) {
        self.events.emit(AppEvent::Notice {
            level: level.to_string(),
            message,
        });
    }

    #[allow(clippy::too_many_arguments)]
    async fn audit_finding(
        &self,
        db: &Db,
        run: &ReviewRun,
        mr: &MergeRequest,
        f: &Finding,
        outcome: &str,
        note_id: Option<&str>,
        error: Option<&str>,
    ) {
        let mut payload = json!({
            "kind": "finding", "finding_id": f.id, "run_id": run.id, "mr_id": mr.id,
            "mr_iid": mr.iid, "file": f.file, "line": f.line, "severity": f.severity.as_str(),
            "fingerprint": f.fingerprint, "outcome": outcome,
        });
        if let Some(n) = note_id {
            payload["note_id"] = json!(n);
        }
        if let Some(e) = error {
            payload["error"] = json!(e);
        }
        audit_quiet(db, Some(run.id), payload).await;
    }
}

// ---------------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------------

/// Statuses a finding may be posted from when its id is requested explicitly.
fn is_postable(s: FindingStatus) -> bool {
    matches!(s, FindingStatus::Draft | FindingStatus::Approved)
}

/// 400 (GitLab: invalid line_code / position) or 422 (GitHub: line not part of the diff).
fn is_position_error(e: &WardenError) -> bool {
    matches!(e, WardenError::Forge { status, .. } if *status == 400 || *status == 422)
}

fn ci_is_failure(s: CiStatus) -> bool {
    matches!(s, CiStatus::Failed | CiStatus::TimedOut)
}

/// Audit failures must never abort a forge write sequence; they are logged instead.
async fn audit_quiet(db: &Db, run_id: Option<i64>, payload: Value) {
    if let Err(e) = db.audit(run_id, AUDIT_POST, payload).await {
        tracing::warn!("audit write failed: {e}");
    }
}

async fn summary_already_posted(db: &Db, run_id: i64) -> bool {
    match db.list_audit(Some(run_id), 10_000).await {
        Ok(events) => events.iter().any(|e| {
            e.kind == AUDIT_POST
                && e.payload["kind"] == "summary"
                && e.payload["outcome"] == "posted"
        }),
        Err(_) => false,
    }
}

fn merge_json(target: &mut Value, extra: &Value) {
    if let (Value::Object(t), Value::Object(x)) = (target, extra) {
        for (k, v) in x {
            t.insert(k.clone(), v.clone());
        }
    }
}

fn deep_merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(slot) => deep_merge(slot, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (slot, v) => *slot = v.clone(),
    }
}

/// `overrides.posting` laid over `PostingConfig::default()`.
pub fn posting_from_overrides(overrides: &Value) -> Result<PostingConfig> {
    let mut base = serde_json::to_value(PostingConfig::default())?;
    if let Some(p) = overrides.get("posting") {
        if p.is_object() {
            deep_merge(&mut base, p);
        }
    }
    serde_json::from_value(base).map_err(|e| WardenError::Config(format!("posting overrides: {e}")))
}

pub fn severity_badge(s: Severity) -> &'static str {
    match s {
        Severity::Blocker => "🔴 Blocker",
        Severity::Major => "🟠 Major",
        Severity::Minor => "🟡 Minor",
        Severity::Nit => "⚪ Nit",
    }
}

fn note_footer(f: &Finding) -> String {
    let mut parts = vec!["— Warden".to_string()];
    if let Some(rule) = f.rule_id.as_deref().filter(|r| !r.trim().is_empty()) {
        parts.push(format!("rule {}", rule.trim()));
    }
    parts.push(format!("confidence {:.2}", f.confidence));
    parts.join(" · ")
}

fn is_single_line(s: &str) -> bool {
    !s.contains('\n')
}

/// A backtick fence longer than any backtick run inside `s` (minimum three).
fn fence_for(s: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in s.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Text safe inside a Markdown table cell.
fn cell(s: &str) -> String {
    one_line(s).replace('|', "\\|")
}

fn short_sha(sha: &str) -> &str {
    let end = sha
        .char_indices()
        .nth(8)
        .map(|(i, _)| i)
        .unwrap_or(sha.len());
    &sha[..end]
}

fn verdict_text(run: &ReviewRun, findings: &[Finding]) -> &'static str {
    match run.verdict.as_deref().map(str::trim) {
        Some("approve") => "no blocking issues",
        Some("request_changes") => "changes requested",
        Some("comment") => "comments",
        _ => {
            if findings.iter().any(|f| f.severity == Severity::Blocker) {
                "changes requested"
            } else if findings.is_empty() {
                "no blocking issues"
            } else {
                "comments"
            }
        }
    }
}

fn ci_status_text(s: CiStatus) -> &'static str {
    match s {
        CiStatus::Passed => "✅ passed",
        CiStatus::Failed => "❌ failed",
        CiStatus::TimedOut => "⏱️ timed out",
        CiStatus::Running => "running",
        CiStatus::Pending => "pending",
        CiStatus::Skipped => "skipped",
        CiStatus::NotInstalled => "not installed",
        CiStatus::Cancelled => "cancelled",
    }
}

/// `850ms`, `3.2s`, `2m 05s`.
pub fn format_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let secs = ms / 1000;
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(sev: Severity, status: FindingStatus) -> Finding {
        Finding {
            id: 1,
            run_id: 1,
            file: "a.py".into(),
            line: 1,
            side: "new".into(),
            severity: sev,
            category: "x".into(),
            rule_id: None,
            title: "t".into(),
            body: "b".into(),
            suggestion: None,
            evidence: None,
            confidence: 0.9,
            source: "claude".into(),
            status,
            guardrail_reason: None,
            fingerprint: "fp".into(),
            forge_note_id: None,
            position: None,
        }
    }

    fn run(verdict: Option<&str>) -> ReviewRun {
        ReviewRun {
            verdict: verdict.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn mr_status_decision() {
        let on = [Severity::Blocker, Severity::Major];
        // posted major -> changes requested
        assert_eq!(
            decide_mr_status(
                &run(None),
                &[f(Severity::Major, FindingStatus::Posted)],
                &on
            ),
            MrReviewStatus::ChangesRequested
        );
        // only minors posted, approve verdict -> approved
        assert_eq!(
            decide_mr_status(
                &run(Some("approve")),
                &[f(Severity::Minor, FindingStatus::Posted)],
                &on
            ),
            MrReviewStatus::Approved
        );
        // dismissed / rejected blockers do not count
        assert_eq!(
            decide_mr_status(
                &run(None),
                &[
                    f(Severity::Blocker, FindingStatus::Dismissed),
                    f(Severity::Blocker, FindingStatus::Rejected)
                ],
                &on
            ),
            MrReviewStatus::Approved
        );
        // a pending (draft) blocker still blocks approval
        assert_eq!(
            decide_mr_status(
                &run(None),
                &[f(Severity::Blocker, FindingStatus::Draft)],
                &on
            ),
            MrReviewStatus::ChangesRequested
        );
        // Claude's verdict alone
        assert_eq!(
            decide_mr_status(&run(Some("request_changes")), &[], &on),
            MrReviewStatus::ChangesRequested
        );
        // narrower threshold
        assert_eq!(
            decide_mr_status(
                &run(None),
                &[f(Severity::Major, FindingStatus::Posted)],
                &[Severity::Blocker]
            ),
            MrReviewStatus::Approved
        );
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(850), "850ms");
        assert_eq!(format_duration(3_210), "3.2s");
        assert_eq!(format_duration(125_000), "2m 05s");
    }

    #[test]
    fn fences_grow_past_backticks() {
        assert_eq!(fence_for("x = 1"), "```");
        assert_eq!(fence_for("a ``` b"), "````");
    }

    #[test]
    fn short_sha_handles_short_input() {
        assert_eq!(short_sha("abc"), "abc");
        assert_eq!(short_sha("0123456789abcdef"), "01234567");
    }

    #[test]
    fn posting_overrides_merge() {
        let p = posting_from_overrides(&json!({"posting": {"max_notes_per_hour": 3}})).unwrap();
        assert_eq!(p.max_notes_per_hour, 3);
        assert!(p.summary_note);
        let d = posting_from_overrides(&json!({})).unwrap();
        assert_eq!(d, PostingConfig::default());
        assert!(posting_from_overrides(&json!({"posting": {"summary_note": "x"}})).is_err());
    }
}
