// OWNER: scaffold
//! Shared domain types. TypeScript bindings are generated into `ui/src/bindings/` by
//! `cargo test -p warden-core` (ts-rs; see `.cargo/config.toml` for the export base dir).
//!
//! Conventions:
//! - Enums serialize as snake_case strings; `as_str()` / `FromStr` give the same strings for SQLite columns.
//! - Timestamps are RFC 3339 strings (`chrono::Utc::now().to_rfc3339()`).
//! - `i64`/`u64` fields are typed as `number` in TypeScript (ids and counters stay far below 2^53).

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::diff::FileDiff;

/// Implements `as_str`, `Display` and `FromStr` using the snake_case serde names.
macro_rules! string_enum {
    ($ty:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        impl $ty {
            pub const ALL: &'static [$ty] = &[$($ty::$variant),+];

            pub fn as_str(&self) -> &'static str {
                match self { $($ty::$variant => $s),+ }
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $ty {
            type Err = crate::error::WardenError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s.trim().to_ascii_lowercase().as_str() {
                    $($s => Ok($ty::$variant),)+
                    other => Err(crate::error::WardenError::Other(format!(
                        concat!("invalid ", stringify!($ty), ": {}"), other
                    ))),
                }
            }
        }
    };
}

// ---------------------------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub enum ForgeKind {
    Gitlab,
    Github,
}
string_enum!(ForgeKind { Gitlab => "gitlab", Github => "github" });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub enum PostingMode {
    /// Nothing is posted until the user approves.
    #[default]
    Draft,
    /// High-severity / high-confidence findings auto-post (per `posting.auto_post`), the rest wait.
    Assist,
    /// Everything that passes the guardrails is posted automatically.
    Auto,
}
string_enum!(PostingMode { Draft => "draft", Assist => "assist", Auto => "auto" });

/// Ordering: `Blocker > Major > Minor > Nit` (so `max()` is the most severe).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub enum Severity {
    Blocker,
    Major,
    Minor,
    Nit,
}
string_enum!(Severity { Blocker => "blocker", Major => "major", Minor => "minor", Nit => "nit" });

impl Severity {
    /// Higher is more severe: Blocker=3, Major=2, Minor=1, Nit=0.
    pub fn rank(&self) -> u8 {
        match self {
            Severity::Blocker => 3,
            Severity::Major => 2,
            Severity::Minor => 1,
            Severity::Nit => 0,
        }
    }
}

impl Ord for Severity {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl PartialOrd for Severity {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub enum FindingStatus {
    #[default]
    Draft,
    Approved,
    Posted,
    Dismissed,
    Resolved,
    /// Dropped by the guardrails (see `guardrail_reason`).
    Rejected,
}
string_enum!(FindingStatus {
    Draft => "draft",
    Approved => "approved",
    Posted => "posted",
    Dismissed => "dismissed",
    Resolved => "resolved",
    Rejected => "rejected",
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub enum RunStatus {
    #[default]
    Queued,
    Preparing,
    Checking,
    Reviewing,
    Filtering,
    AwaitingApproval,
    Posting,
    Posted,
    Failed,
    Skipped,
    Cancelled,
}
string_enum!(RunStatus {
    Queued => "queued",
    Preparing => "preparing",
    Checking => "checking",
    Reviewing => "reviewing",
    Filtering => "filtering",
    AwaitingApproval => "awaiting_approval",
    Posting => "posting",
    Posted => "posted",
    Failed => "failed",
    Skipped => "skipped",
    Cancelled => "cancelled",
});

impl RunStatus {
    /// True once the run will make no further progress on its own.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunStatus::AwaitingApproval
                | RunStatus::Posted
                | RunStatus::Failed
                | RunStatus::Skipped
                | RunStatus::Cancelled
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub enum CiStatus {
    #[default]
    Pending,
    Running,
    Passed,
    Failed,
    Skipped,
    NotInstalled,
    TimedOut,
    Cancelled,
}
string_enum!(CiStatus {
    Pending => "pending",
    Running => "running",
    Passed => "passed",
    Failed => "failed",
    Skipped => "skipped",
    NotInstalled => "not_installed",
    TimedOut => "timed_out",
    Cancelled => "cancelled",
});

// ---------------------------------------------------------------------------------------------
// Entities
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct Project {
    #[ts(type = "number")]
    pub id: i64,
    pub forge: ForgeKind,
    /// e.g. `https://gitlab.com`
    pub base_url: String,
    /// `"entries/entries_crm"` or a numeric id.
    pub remote_id: String,
    pub name: String,
    /// Local clone used as the source repo for worktrees.
    pub local_path: String,
    /// Glob patterns for target branches to watch (e.g. `["main", "release/*"]`).
    pub target_branches: Vec<String>,
    pub mode: PostingMode,
    pub poll_interval_secs: u32,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct MergeRequest {
    #[ts(type = "number")]
    pub id: i64,
    #[ts(type = "number")]
    pub project_id: i64,
    #[ts(type = "number")]
    pub iid: u64,
    pub title: String,
    pub description: String,
    pub author: String,
    pub source_branch: String,
    pub target_branch: String,
    pub web_url: String,
    pub draft: bool,
    pub labels: Vec<String>,
    pub head_sha: String,
    pub base_sha: String,
    pub start_sha: String,
    pub last_reviewed_sha: Option<String>,
    pub additions: u32,
    pub deletions: u32,
    /// `opened` | `merged` | `closed`
    pub state: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct DiffPosition {
    pub base_sha: String,
    pub start_sha: String,
    pub head_sha: String,
    pub old_path: String,
    pub new_path: String,
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct Finding {
    #[ts(type = "number")]
    pub id: i64,
    #[ts(type = "number")]
    pub run_id: i64,
    pub file: String,
    pub line: u32,
    /// `"new"` | `"old"`
    pub side: String,
    pub severity: Severity,
    pub category: String,
    pub rule_id: Option<String>,
    pub title: String,
    pub body: String,
    pub suggestion: Option<String>,
    pub evidence: Option<String>,
    pub confidence: f32,
    /// `"claude"` | `"rule"` | `"ci"`
    pub source: String,
    pub status: FindingStatus,
    pub guardrail_reason: Option<String>,
    /// See `guardrails::fingerprint`: sha256 hex of `file:normalized(title):rule_id`.
    pub fingerprint: String,
    pub forge_note_id: Option<String>,
    pub position: Option<DiffPosition>,
}

/// Input shape for a finding coming from Claude (MCP `review_add_finding`), rules or CI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct NewFinding {
    pub file: String,
    pub line: u32,
    /// `"new"` | `"old"`
    pub side: String,
    pub severity: Severity,
    pub category: String,
    pub rule_id: Option<String>,
    pub title: String,
    pub body: String,
    pub suggestion: Option<String>,
    pub evidence: Option<String>,
    pub confidence: f32,
    /// `"claude"` | `"rule"` | `"ci"`
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct ReviewRun {
    #[ts(type = "number")]
    pub id: i64,
    #[ts(type = "number")]
    pub mr_id: i64,
    pub base_sha: String,
    pub head_sha: String,
    /// Set for incremental re-reviews (`since..head`).
    pub since_sha: Option<String>,
    pub status: RunStatus,
    pub model: String,
    pub turns: u32,
    #[ts(type = "number")]
    pub duration_ms: u64,
    pub cli_session_id: Option<String>,
    pub summary: Option<String>,
    /// `approve` | `comment` | `request_changes`
    pub verdict: Option<String>,
    pub error: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct CiJob {
    #[ts(type = "number")]
    pub id: i64,
    #[ts(type = "number")]
    pub run_id: i64,
    pub name: String,
    /// `recipe` | `gitlab_ci_local` | `check`
    pub kind: String,
    pub command: String,
    pub status: CiStatus,
    #[ts(type = "number")]
    pub duration_ms: u64,
    pub log_path: String,
    pub failure_excerpt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct AuditEvent {
    #[ts(type = "number")]
    pub id: i64,
    #[ts(type = "number | null")]
    pub run_id: Option<i64>,
    /// `tool_call` | `guardrail` | `post` | `engine` | `error`
    pub kind: String,
    #[ts(type = "unknown")]
    pub payload: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct EngineStatus {
    pub cli_path: Option<String>,
    pub version: Option<String>,
    pub logged_in: bool,
    pub subscription: Option<String>,
    /// True if ANTHROPIC_API_KEY (or similar) is set in Warden's env; it is stripped from the child anyway.
    pub api_key_in_env: bool,
    pub ok: bool,
    pub message: String,
}

/// semgrep, gitleaks, gitlab-ci-local, act, docker, rg, git, claude
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct ToolStatus {
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct UsageStats {
    pub reviews_last_hour: u32,
    pub reviews_today: u32,
    pub max_reviews_per_hour: u32,
    pub paused: bool,
}

/// Events streamed to the UI on the Tauri channel `"warden://event"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub enum AppEvent {
    RunUpdated {
        run: ReviewRun,
    },
    RunProgress {
        #[ts(type = "number")]
        run_id: i64,
        message: String,
        turn: u32,
    },
    CiLog {
        #[ts(type = "number")]
        run_id: i64,
        #[ts(type = "number")]
        job_id: i64,
        line: String,
    },
    CiJobUpdated {
        job: CiJob,
    },
    FindingsUpdated {
        #[ts(type = "number")]
        run_id: i64,
    },
    MrsUpdated {
        #[ts(type = "number")]
        project_id: i64,
    },
    Notice {
        /// `info` | `warn` | `error`
        level: String,
        message: String,
    },
}

// ---------------------------------------------------------------------------------------------
// Command-level view models (returned by Tauri commands; see SPEC "Tauri commands")
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct FindingCounts {
    pub draft: u32,
    pub posted: u32,
    pub rejected: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct CiSummary {
    pub passed: u32,
    pub failed: u32,
    pub running: u32,
    pub total: u32,
}

/// `list_mrs` row: MergeRequest fields (flattened) + project name + latest run + counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct MrRow {
    #[serde(flatten)]
    pub mr: MergeRequest,
    pub project_name: String,
    pub latest_run: Option<ReviewRun>,
    pub finding_counts: FindingCounts,
    pub ci_summary: CiSummary,
}

/// `get_run` payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct RunDetail {
    pub run: ReviewRun,
    pub mr: MergeRequest,
    pub findings: Vec<Finding>,
    pub ci_jobs: Vec<CiJob>,
    pub files: Vec<FileDiff>,
}

/// `forge_status` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct ForgeStatus {
    pub forge: ForgeKind,
    pub base_url: String,
    pub user: Option<String>,
    pub ok: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_ordering() {
        assert!(Severity::Blocker > Severity::Major);
        assert!(Severity::Major > Severity::Minor);
        assert!(Severity::Minor > Severity::Nit);
        let mut v = vec![
            Severity::Nit,
            Severity::Blocker,
            Severity::Minor,
            Severity::Major,
        ];
        v.sort_by(|a, b| b.cmp(a));
        assert_eq!(
            v,
            vec![
                Severity::Blocker,
                Severity::Major,
                Severity::Minor,
                Severity::Nit
            ]
        );
    }

    #[test]
    fn enums_round_trip_snake_case() {
        assert_eq!(
            serde_json::to_string(&RunStatus::AwaitingApproval).unwrap(),
            "\"awaiting_approval\""
        );
        assert_eq!(
            serde_json::to_string(&CiStatus::NotInstalled).unwrap(),
            "\"not_installed\""
        );
        for s in RunStatus::ALL {
            assert_eq!(s.as_str().parse::<RunStatus>().unwrap(), *s);
            assert_eq!(
                serde_json::to_string(s).unwrap(),
                format!("\"{}\"", s.as_str())
            );
        }
        for s in CiStatus::ALL {
            assert_eq!(
                serde_json::to_string(s).unwrap(),
                format!("\"{}\"", s.as_str())
            );
        }
        for s in FindingStatus::ALL {
            assert_eq!(
                serde_json::to_string(s).unwrap(),
                format!("\"{}\"", s.as_str())
            );
        }
        assert_eq!("Major".parse::<Severity>().unwrap(), Severity::Major);
        assert!("huge".parse::<Severity>().is_err());
    }

    #[test]
    fn app_event_is_tagged() {
        let ev = AppEvent::RunProgress {
            run_id: 3,
            message: "reading diff".into(),
            turn: 2,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "run_progress");
        assert_eq!(v["run_id"], 3);
    }

    #[test]
    fn mr_row_is_flattened() {
        let row = MrRow {
            mr: MergeRequest {
                iid: 42,
                title: "t".into(),
                ..Default::default()
            },
            project_name: "p".into(),
            latest_run: None,
            finding_counts: FindingCounts::default(),
            ci_summary: CiSummary::default(),
        };
        let v = serde_json::to_value(&row).unwrap();
        assert_eq!(v["iid"], 42);
        assert_eq!(v["project_name"], "p");
    }
}

// ---------------------------------------------------------------------------------------------
// Project discovery (add-project screen)
// ---------------------------------------------------------------------------------------------

/// A project on the forge the token owner can access.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct RemoteProjectInfo {
    /// `"entries/entries_crm"` (GitLab path / GitHub `owner/repo`).
    pub remote_id: String,
    pub name: String,
    pub default_branch: Option<String>,
    /// RFC 3339 timestamp of the last push/activity.
    pub last_activity_at: Option<String>,
    pub web_url: Option<String>,
    /// HTTPS clone URL (used for Warden-managed clones).
    pub clone_url: Option<String>,
}

/// A remote project merged with what Warden found on this Mac.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct DiscoveredProject {
    pub remote: RemoteProjectInfo,
    /// Local clones whose remote points at this project (best match first).
    pub local_paths: Vec<String>,
    /// Set when Warden already watches this project.
    #[ts(type = "number | null")]
    pub existing_project_id: Option<i64>,
}

/// A git repo found by a folder scan.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct LocalRepo {
    pub path: String,
    /// URL of the remote that matched (or the first remote).
    pub remote_url: String,
    /// The forge project it belongs to, when it is on the connected forge.
    pub remote_id: Option<String>,
    /// Another scanned path with the same project (this one is skipped by default).
    pub duplicate_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct Discovery {
    pub projects: Vec<DiscoveredProject>,
    pub local_repos: Vec<LocalRepo>,
    pub scanned_roots: Vec<String>,
    #[ts(type = "number")]
    pub scanned_dirs: u32,
}

/// One project to add. `local_path = None` → Warden clones its own copy.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct AddProjectItem {
    pub remote_id: String,
    pub name: String,
    pub local_path: Option<String>,
    /// `None` → default branch + protected branches from the forge.
    pub target_branches: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct AddProjectResult {
    pub remote_id: String,
    #[ts(type = "number | null")]
    pub project_id: Option<i64>,
    pub error: Option<String>,
}

/// A merge request / pull request link pasted by the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct MrLink {
    pub forge: ForgeKind,
    pub base_url: String,
    pub remote_id: String,
    #[ts(type = "number")]
    pub iid: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct OpenMrResult {
    #[ts(type = "number")]
    pub project_id: i64,
    #[ts(type = "number")]
    pub mr_id: i64,
    #[ts(type = "number")]
    pub run_id: i64,
    /// True when the project was added by this call.
    pub added_project: bool,
}

// ---------------------------------------------------------------------------------------------
// Stats, rule catalogue, guardrail simulation, approval queue (UX redesign)
// ---------------------------------------------------------------------------------------------

/// One day of `Stats::per_day` (`date` = `YYYY-MM-DD`, UTC).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct DayStat {
    pub date: String,
    pub reviews: u32,
    pub notes: u32,
}

/// `get_stats(days)`: totals over the last `days` days plus one row per day (zero days included).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct Stats {
    pub reviews: u32,
    /// Findings posted to the forge (status Posted or Resolved).
    pub notes_posted: u32,
    /// Posted findings later resolved.
    pub resolved: u32,
    /// `resolved / notes_posted`, 0 when nothing was posted.
    pub resolved_rate: f64,
    /// Median duration of finished runs (0 when none).
    #[ts(type = "number")]
    pub median_run_ms: u64,
    pub per_day: Vec<DayStat>,
}

/// `rule_stats(project_id, days)` row: how one rule (or Claude category) behaved.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct RuleStat {
    pub rule_id: String,
    pub title: String,
    pub hits: u32,
    /// Posted or Resolved.
    pub posted: u32,
    /// Dismissed by a reviewer.
    pub dismissed: u32,
    /// Dropped by the guardrails.
    pub rejected: u32,
    pub last_hit_at: Option<String>,
}

/// `simulate_guardrails` row: what would happen to an existing finding under candidate settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct SimulatedFinding {
    pub finding: Finding,
    /// e.g. `"!42 Add billing export"`
    pub mr_label: String,
    /// `"post"` | `"draft"` | `"drop"`
    pub outcome: String,
    pub reason: String,
}

/// `list_pending_findings` row: a Draft/Approved finding from the latest run of an open MR.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct PendingFinding {
    pub finding: Finding,
    #[ts(type = "number")]
    pub run_id: i64,
    #[ts(type = "number")]
    pub mr_id: i64,
    #[ts(type = "number")]
    pub mr_iid: u64,
    pub mr_title: String,
    pub project_name: String,
    pub web_url: String,
}

/// `list_run_rows` row: a run with its MR identity, project and per-run counts (History screen).
/// Covers runs of merged/closed MRs and local reviews (local runs have `project_id = 0`, the UI's
/// "local" marker, like `list_mrs`). Count buckets match `MrRow`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../ui/src/bindings/")]
pub struct RunRow {
    pub run: ReviewRun,
    #[ts(type = "number")]
    pub mr_iid: u64,
    pub mr_title: String,
    pub author: String,
    pub web_url: String,
    pub draft: bool,
    #[ts(type = "number")]
    pub project_id: i64,
    pub project_name: String,
    pub finding_counts: FindingCounts,
    pub ci_summary: CiSummary,
}
