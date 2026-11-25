// OWNER: integration
//! History KPIs (`get_stats`), the per-rule catalogue (`rule_stats`), the guardrail live preview
//! (`simulate_guardrails`) and the approval queue list (`list_pending_findings`).
//!
//! The pure functions (`compute_*`, `simulate_findings`) take rows already loaded from the DB so
//! they can be unit-tested without SQLite; the async wrappers do the loading.
//!
//! Days are UTC calendar days (`YYYY-MM-DD`). A posted note counts on the day of its `posted_at`
//! (falling back to its run's start day).

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Duration, NaiveDate, Utc};

use crate::config::{RuleConfig, WardenConfig};
use crate::db::{format_ts, Db};
use crate::diff::PathMatcher;
use crate::error::Result;
use crate::guardrails::{apply_budget, auto_post_for, is_policy_reason, policy_check, OVER_BUDGET};
use crate::types::{
    DayStat, Finding, FindingStatus, MergeRequest, PendingFinding, PostingMode, ReviewRun,
    RuleStat, RunStatus, SimulatedFinding, Stats,
};

/// Upper bound for the `days` window.
pub const MAX_DAYS: u32 = 366;
/// Upper bound for `simulate_guardrails`' `limit_runs`.
pub const MAX_SIM_RUNS: u32 = 100;

/// UTC day of an RFC 3339 timestamp.
fn day_of(ts: &str) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(ts.trim())
        .ok()
        .map(|t| t.with_timezone(&Utc).date_naive())
}

fn is_posted(s: FindingStatus) -> bool {
    matches!(s, FindingStatus::Posted | FindingStatus::Resolved)
}

/// A run that actually reviewed something (same idea as `Db::reviews_started_since`): skipped runs
/// and queued/cancelled/failed runs that never reached the engine do not count.
fn counts_as_review(r: &ReviewRun) -> bool {
    match r.status {
        RunStatus::Skipped => false,
        RunStatus::Queued | RunStatus::Cancelled | RunStatus::Failed => r.turns > 0,
        _ => true,
    }
}

/// First day of a `days`-long window ending `today` (inclusive).
pub fn window_start(today: NaiveDate, days: u32) -> NaiveDate {
    today - Duration::days(i64::from(days.clamp(1, MAX_DAYS)) - 1)
}

// ---------------------------------------------------------------------------------------------
// get_stats
// ---------------------------------------------------------------------------------------------

/// `runs`: runs started in the window; `findings`: (finding, run started_at, posted_at) for those
/// runs. Rows outside `[window_start, today]` are ignored.
pub fn compute_stats(
    runs: &[ReviewRun],
    findings: &[(Finding, String, Option<String>)],
    days: u32,
    today: NaiveDate,
) -> Stats {
    let days = days.clamp(1, MAX_DAYS);
    let start = window_start(today, days);
    let mut per_day: Vec<DayStat> = (0..days)
        .map(|i| DayStat {
            date: (start + Duration::days(i64::from(i)))
                .format("%Y-%m-%d")
                .to_string(),
            reviews: 0,
            notes: 0,
        })
        .collect();
    let slot = |d: NaiveDate| -> Option<usize> {
        (d >= start && d <= today).then(|| (d - start).num_days() as usize)
    };

    let mut reviews = 0u32;
    let mut durations: Vec<u64> = Vec::new();
    for r in runs.iter().filter(|r| counts_as_review(r)) {
        let Some(i) = day_of(&r.started_at).and_then(slot) else {
            continue;
        };
        reviews += 1;
        per_day[i].reviews += 1;
        if matches!(r.status, RunStatus::Posted | RunStatus::AwaitingApproval)
            && r.ended_at.is_some()
            && r.duration_ms > 0
        {
            durations.push(r.duration_ms);
        }
    }

    let mut notes_posted = 0u32;
    let mut resolved = 0u32;
    for (f, run_started, posted_at) in findings {
        if !is_posted(f.status) {
            continue;
        }
        let day = posted_at
            .as_deref()
            .and_then(day_of)
            .filter(|d| slot(*d).is_some())
            .or_else(|| day_of(run_started));
        let Some(i) = day.and_then(slot) else {
            continue;
        };
        notes_posted += 1;
        per_day[i].notes += 1;
        if f.status == FindingStatus::Resolved {
            resolved += 1;
        }
    }

    Stats {
        reviews,
        notes_posted,
        resolved,
        resolved_rate: if notes_posted == 0 {
            0.0
        } else {
            f64::from(resolved) / f64::from(notes_posted)
        },
        median_run_ms: median(&mut durations),
        per_day,
    }
}

fn median(v: &mut [u64]) -> u64 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2
    }
}

/// Stats for the last `days` UTC days (today included).
pub async fn get_stats(db: &Db, days: u32) -> Result<Stats> {
    let today = Utc::now().date_naive();
    let start = window_start(today, days);
    let since = format_ts(start.and_hms_opt(0, 0, 0).expect("midnight").and_utc());
    let runs = db.runs_started_from(&since).await?;
    let findings = db.findings_from(&since, None).await?;
    Ok(compute_stats(&runs, &findings, days, today))
}

// ---------------------------------------------------------------------------------------------
// rule_stats
// ---------------------------------------------------------------------------------------------

/// Catalogue key of a finding: its rule id, else its category (Claude findings without a rule).
pub fn rule_key(f: &Finding) -> String {
    if let Some(r) = f
        .rule_id
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        return r.to_string();
    }
    let c = f.category.trim();
    if c.is_empty() {
        "uncategorized".to_string()
    } else {
        c.to_string()
    }
}

/// `findings`: (finding, run started_at, posted_at), oldest run first. `rules`: configured rules
/// whose titles win over finding titles. Sorted by hits desc, then rule id.
pub fn compute_rule_stats(
    findings: &[(Finding, String, Option<String>)],
    rules: &[RuleConfig],
) -> Vec<RuleStat> {
    let mut rows: BTreeMap<String, RuleStat> = BTreeMap::new();
    for (f, started, _) in findings {
        let key = rule_key(f);
        let row = rows.entry(key.clone()).or_insert_with(|| RuleStat {
            rule_id: key,
            ..Default::default()
        });
        row.hits += 1;
        match f.status {
            FindingStatus::Posted | FindingStatus::Resolved => row.posted += 1,
            FindingStatus::Dismissed => row.dismissed += 1,
            FindingStatus::Rejected => row.rejected += 1,
            _ => {}
        }
        let newer = row
            .last_hit_at
            .as_deref()
            .is_none_or(|last| crate::db::normalize_ts(started) >= crate::db::normalize_ts(last));
        if newer {
            row.last_hit_at = Some(started.clone());
            row.title = f.title.clone();
        }
    }
    let mut out: Vec<RuleStat> = rows
        .into_values()
        .map(|mut r| {
            if let Some(rule) = rules.iter().find(|c| c.id == r.rule_id) {
                r.title = rule.title.clone();
            }
            r
        })
        .collect();
    out.sort_by(|a, b| b.hits.cmp(&a.hits).then_with(|| a.rule_id.cmp(&b.rule_id)));
    out
}

/// Rule stats for the last `days` days, optionally for one project.
pub async fn rule_stats(
    db: &Db,
    project_id: Option<i64>,
    days: u32,
    rules: &[RuleConfig],
) -> Result<Vec<RuleStat>> {
    let since = format_ts(Utc::now() - Duration::days(i64::from(days.clamp(1, MAX_DAYS))));
    let findings = db.findings_from(&since, project_id).await?;
    Ok(compute_rule_stats(&findings, rules))
}

// ---------------------------------------------------------------------------------------------
// simulate_guardrails
// ---------------------------------------------------------------------------------------------

pub const OUTCOME_POST: &str = "post";
pub const OUTCOME_DRAFT: &str = "draft";
pub const OUTCOME_DROP: &str = "drop";

/// `!42 Title`
pub fn mr_label(mr: &MergeRequest) -> String {
    format!("!{} {}", mr.iid, mr.title)
}

/// Re-classifies stored findings under `cfg` (already merged with the candidate overrides).
/// Per run, in order: disabled rule, ignored path, policy checks (nits / severity floor /
/// confidence via [`policy_check`]), a stored non-policy rejection (anchor, tone, duplicate...),
/// then the note budget ([`apply_budget`]) and the posting decision ([`auto_post_for`]).
pub fn simulate_findings(
    runs: &[(MergeRequest, Vec<Finding>)],
    cfg: &WardenConfig,
    disabled: &[String],
) -> Vec<SimulatedFinding> {
    let ignore = PathMatcher::lenient(&cfg.review.ignore_paths);
    let mut out = Vec::new();
    for (mr, findings) in runs {
        let label = mr_label(mr);
        let mut rows: Vec<SimulatedFinding> = Vec::with_capacity(findings.len());
        // Survivors (index into rows) as fresh drafts for the budget pass.
        let mut survivors: Vec<usize> = Vec::new();
        let mut budget_input: Vec<Finding> = Vec::new();
        for f in findings {
            let drop = |reason: String| SimulatedFinding {
                finding: f.clone(),
                mr_label: label.clone(),
                outcome: OUTCOME_DROP.into(),
                reason,
            };
            let rule = f.rule_id.as_deref().map(str::trim).unwrap_or("");
            if !rule.is_empty() && disabled.iter().any(|d| d == rule) {
                rows.push(drop(format!("rule {rule} is disabled")));
                continue;
            }
            if ignore.is_match(&f.file) {
                rows.push(drop(format!("{} is in ignore_paths", f.file)));
                continue;
            }
            if let Some(reason) = policy_check(cfg, f.severity, f.confidence, &f.source) {
                rows.push(drop(reason));
                continue;
            }
            if f.status == FindingStatus::Rejected {
                if let Some(reason) = f
                    .guardrail_reason
                    .as_deref()
                    .filter(|r| !r.trim().is_empty() && !is_policy_reason(r))
                {
                    rows.push(drop(reason.to_string()));
                    continue;
                }
            }
            survivors.push(rows.len());
            let mut draft = f.clone();
            draft.status = FindingStatus::Draft;
            draft.guardrail_reason = None;
            budget_input.push(draft);
            rows.push(SimulatedFinding {
                finding: f.clone(),
                mr_label: label.clone(),
                outcome: OUTCOME_DRAFT.into(),
                reason: String::new(),
            });
        }
        apply_budget(&mut budget_input, cfg.review.max_notes);
        for (draft, &i) in budget_input.iter().zip(&survivors) {
            let row = &mut rows[i];
            if draft.status == FindingStatus::Rejected {
                row.outcome = OUTCOME_DROP.into();
                row.reason = format!("{OVER_BUDGET} (max {} per run)", cfg.review.max_notes);
            } else if auto_post_for(cfg, draft.severity, draft.confidence) {
                row.outcome = OUTCOME_POST.into();
                row.reason = match cfg.posting.mode {
                    PostingMode::Auto => "auto mode posts everything that passes".into(),
                    _ => format!(
                        "auto-posts: {}, confidence {:.2} ≥ {:.2}",
                        draft.severity, draft.confidence, cfg.posting.auto_post.min_confidence
                    ),
                };
            } else {
                row.outcome = OUTCOME_DRAFT.into();
                row.reason = match cfg.posting.mode {
                    PostingMode::Draft => "draft mode: waits for approval".into(),
                    _ => "waits for approval (below the auto-post bar)".into(),
                };
            }
        }
        out.extend(rows);
    }
    out
}

/// Loads the project's newest `limit_runs` runs with findings and simulates them under `cfg`.
pub async fn simulate_guardrails(
    db: &Db,
    project_id: i64,
    cfg: &WardenConfig,
    disabled: &[String],
    limit_runs: u32,
) -> Result<Vec<SimulatedFinding>> {
    let runs = db
        .recent_runs_with_findings(project_id, limit_runs.clamp(1, MAX_SIM_RUNS))
        .await?;
    let mut mrs: HashMap<i64, MergeRequest> = HashMap::new();
    let mut loaded = Vec::with_capacity(runs.len());
    for run in runs {
        let mr = match mrs.get(&run.mr_id) {
            Some(m) => m.clone(),
            None => {
                let m = db.get_mr(run.mr_id).await?;
                mrs.insert(run.mr_id, m.clone());
                m
            }
        };
        loaded.push((mr, db.list_findings(run.id).await?));
    }
    Ok(simulate_findings(&loaded, cfg, disabled))
}

// ---------------------------------------------------------------------------------------------
// approval queue
// ---------------------------------------------------------------------------------------------

/// Draft/Approved findings from the latest run of every open MR, with MR and project info.
/// Local reviews (nothing to post to) are left out.
pub async fn list_pending_findings(db: &Db) -> Result<Vec<PendingFinding>> {
    let rows = db.pending_findings().await?;
    let mut mrs: HashMap<i64, Option<(MergeRequest, String)>> = HashMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for (finding, mr_id) in rows {
        let entry = match mrs.entry(mr_id) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let mr = db.get_mr(mr_id).await?;
                let project = db.get_project(mr.project_id).await?;
                let name = project.name.clone();
                e.insert((!crate::pipeline::is_local_project(&project)).then_some((mr, name)))
            }
        };
        let Some((mr, project_name)) = entry.as_ref() else {
            continue;
        };
        out.push(PendingFinding {
            run_id: finding.run_id,
            finding,
            mr_id,
            mr_iid: mr.iid,
            mr_title: mr.title.clone(),
            project_name: project_name.clone(),
            web_url: mr.web_url.clone(),
        });
    }
    Ok(out)
}

/// Groups finding ids by run (ascending run id, ids in the given order, duplicates removed).
/// Unknown ids are an error.
pub async fn group_by_run(db: &Db, finding_ids: &[i64]) -> Result<Vec<(i64, Vec<i64>)>> {
    let mut groups: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for &id in finding_ids {
        let f = db.get_finding(id).await?;
        let ids = groups.entry(f.run_id).or_default();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(groups.into_iter().collect())
}
