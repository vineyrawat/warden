// OWNER: poster
//! SQLite storage (`<data_dir>/warden.db`, WAL). Migrations embedded via `sqlx::migrate!("./migrations")`.
//! Runtime `sqlx::query` / `query_as` only (hard rule 5: no `query!` macros). Enums are stored as their
//! snake_case `as_str()`; Vec/struct fields as JSON text; timestamps as RFC 3339 text.
//!
//! Timestamps written by this module use one fixed shape, UTC with millisecond precision
//! (`2026-11-25T10:04:05.123Z`, see [`now_ts`]), so plain text comparison orders them correctly.
//! Counter queries (`runs_started_since`, `posts_since`) normalise their argument to the same shape
//! first, so callers may pass any RFC 3339 string (`+00:00`, other offsets, no fraction, ...).

use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow, SqliteSynchronous,
};
use sqlx::{Row, SqlitePool};

use crate::config::AppSettings;
use crate::error::{Result, WardenError};
use crate::types::{
    AuditEvent, CiJob, CiStatus, CiSummary, DiffPosition, Finding, FindingCounts, FindingStatus,
    ForgeKind, MergeRequest, NewFinding, PostingMode, Project, ReviewRun, RunRow, RunStatus,
    Severity,
};

const SETTINGS_KEY: &str = "app";

/// Current time in the canonical storage shape (`YYYY-MM-DDTHH:MM:SS.mmmZ`, UTC).
pub fn now_ts() -> String {
    format_ts(Utc::now())
}

/// Formats a UTC instant in the canonical storage shape.
pub fn format_ts(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Re-formats any RFC 3339 timestamp into the canonical storage shape. Unparseable input is returned
/// unchanged (text comparison then still works for values already in canonical shape).
pub fn normalize_ts(ts: &str) -> String {
    match DateTime::parse_from_rfc3339(ts.trim()) {
        Ok(t) => format_ts(t.with_timezone(&Utc)),
        Err(_) => ts.to_string(),
    }
}

#[derive(Debug, Clone)]
pub struct Db {
    pub pool: SqlitePool,
}

impl Db {
    /// Opens/creates the database file (WAL, foreign keys on) and runs migrations.
    pub async fn open(path: &Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await?;
        let db = Db { pool };
        db.migrate().await?;
        Ok(db)
    }

    /// In-memory database with migrations applied (tests).
    ///
    /// Uses a single connection that is never recycled: every new connection to `:memory:` would be a
    /// fresh, empty database.
    pub async fn open_memory() -> Result<Db> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")?.foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await?;
        let db = Db { pool };
        db.migrate().await?;
        Ok(db)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    // --- settings ---

    /// Missing row => `AppSettings::default()`. A corrupt row also yields the defaults (logged) so the
    /// app can always start.
    pub async fn get_settings(&self) -> Result<AppSettings> {
        let row = sqlx::query("SELECT value FROM settings WHERE key = ?")
            .bind(SETTINGS_KEY)
            .fetch_optional(&self.pool)
            .await?;
        let Some(row) = row else {
            return Ok(AppSettings::default());
        };
        let raw: String = row.try_get("value")?;
        match serde_json::from_str::<AppSettings>(&raw) {
            Ok(s) => Ok(s),
            Err(e) => {
                tracing::warn!("settings row is not valid AppSettings JSON ({e}); using defaults");
                Ok(AppSettings::default())
            }
        }
    }

    pub async fn save_settings(&self, settings: &AppSettings) -> Result<()> {
        let json = serde_json::to_string(settings)?;
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(SETTINGS_KEY)
        .bind(json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Integration addition: arbitrary JSON value in the `settings` table under `key` (not "app").
    pub async fn get_kv(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            None => Ok(None),
            Some(r) => {
                let raw: String = r.try_get("value")?;
                Ok(serde_json::from_str(&raw).ok())
            }
        }
    }

    /// Integration addition: see [`Db::get_kv`].
    pub async fn set_kv(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        if key == SETTINGS_KEY {
            return Err(WardenError::Other(
                "use save_settings for the app settings".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(serde_json::to_string(value)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Missing row => `{}`.
    pub async fn get_project_overrides(&self, project_id: i64) -> Result<serde_json::Value> {
        let row = sqlx::query("SELECT overrides FROM project_overrides WHERE project_id = ?")
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await?;
        match row {
            None => Ok(serde_json::json!({})),
            Some(r) => {
                let raw: String = r.try_get("overrides")?;
                Ok(serde_json::from_str(&raw)?)
            }
        }
    }

    pub async fn save_project_overrides(
        &self,
        project_id: i64,
        overrides: &serde_json::Value,
    ) -> Result<()> {
        let json = serde_json::to_string(overrides)?;
        sqlx::query(
            "INSERT INTO project_overrides (project_id, overrides, updated_at) VALUES (?, ?, ?)
             ON CONFLICT(project_id) DO UPDATE SET overrides = excluded.overrides,
                                                   updated_at = excluded.updated_at",
        )
        .bind(project_id)
        .bind(json)
        .bind(now_ts())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // --- projects ---

    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        let rows = sqlx::query("SELECT * FROM projects ORDER BY name COLLATE NOCASE, id")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_project).collect()
    }

    /// Missing => `WardenError::NotFound`.
    pub async fn get_project(&self, id: i64) -> Result<Project> {
        let row = sqlx::query("SELECT * FROM projects WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| WardenError::NotFound(format!("project {id}")))?;
        row_project(&row)
    }

    /// Insert when `id == 0`, else update. Returns the id.
    ///
    /// Inserting a project that already exists (same forge + base_url + remote_id) updates that row
    /// and returns its id. Updating an id that does not exist => `NotFound`.
    pub async fn upsert_project(&self, project: &Project) -> Result<i64> {
        let branches = serde_json::to_string(&project.target_branches)?;
        if project.id == 0 {
            let row = sqlx::query(
                "INSERT INTO projects (forge, base_url, remote_id, name, local_path, target_branches,
                                       mode, poll_interval_secs, enabled, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(forge, base_url, remote_id) DO UPDATE SET
                    name = excluded.name,
                    local_path = excluded.local_path,
                    target_branches = excluded.target_branches,
                    mode = excluded.mode,
                    poll_interval_secs = excluded.poll_interval_secs,
                    enabled = excluded.enabled
                 RETURNING id",
            )
            .bind(project.forge.as_str())
            .bind(&project.base_url)
            .bind(&project.remote_id)
            .bind(&project.name)
            .bind(&project.local_path)
            .bind(branches)
            .bind(project.mode.as_str())
            .bind(i64::from(project.poll_interval_secs))
            .bind(project.enabled)
            .bind(now_ts())
            .fetch_one(&self.pool)
            .await?;
            Ok(row.try_get("id")?)
        } else {
            let res = sqlx::query(
                "UPDATE projects SET forge = ?, base_url = ?, remote_id = ?, name = ?, local_path = ?,
                        target_branches = ?, mode = ?, poll_interval_secs = ?, enabled = ?
                 WHERE id = ?",
            )
            .bind(project.forge.as_str())
            .bind(&project.base_url)
            .bind(&project.remote_id)
            .bind(&project.name)
            .bind(&project.local_path)
            .bind(branches)
            .bind(project.mode.as_str())
            .bind(i64::from(project.poll_interval_secs))
            .bind(project.enabled)
            .bind(project.id)
            .execute(&self.pool)
            .await?;
            if res.rows_affected() == 0 {
                return Err(WardenError::NotFound(format!("project {}", project.id)));
            }
            Ok(project.id)
        }
    }

    /// Deletes the project and (cascade) its overrides, MRs, runs, findings and CI jobs.
    /// Deleting a missing id is not an error.
    pub async fn delete_project(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // --- merge requests ---

    /// Upsert on (project_id, iid); keeps `last_reviewed_sha`. Returns the id.
    ///
    /// `mr.id` is ignored. An existing non-null `last_reviewed_sha` always wins (forge data never
    /// carries it; use [`Db::set_last_reviewed`] to change it); a new row takes the given value.
    pub async fn upsert_mr(&self, mr: &MergeRequest) -> Result<i64> {
        let labels = serde_json::to_string(&mr.labels)?;
        let row = sqlx::query(
            "INSERT INTO merge_requests (project_id, iid, title, description, author, source_branch,
                    target_branch, web_url, draft, labels, head_sha, base_sha, start_sha,
                    last_reviewed_sha, additions, deletions, state, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(project_id, iid) DO UPDATE SET
                title = excluded.title,
                description = excluded.description,
                author = excluded.author,
                source_branch = excluded.source_branch,
                target_branch = excluded.target_branch,
                web_url = excluded.web_url,
                draft = excluded.draft,
                labels = excluded.labels,
                head_sha = excluded.head_sha,
                base_sha = excluded.base_sha,
                start_sha = excluded.start_sha,
                last_reviewed_sha = COALESCE(merge_requests.last_reviewed_sha, excluded.last_reviewed_sha),
                additions = excluded.additions,
                deletions = excluded.deletions,
                state = excluded.state,
                updated_at = excluded.updated_at
             RETURNING id",
        )
        .bind(mr.project_id)
        .bind(u64_to_i64(mr.iid))
        .bind(&mr.title)
        .bind(&mr.description)
        .bind(&mr.author)
        .bind(&mr.source_branch)
        .bind(&mr.target_branch)
        .bind(&mr.web_url)
        .bind(mr.draft)
        .bind(labels)
        .bind(&mr.head_sha)
        .bind(&mr.base_sha)
        .bind(&mr.start_sha)
        .bind(mr.last_reviewed_sha.as_deref())
        .bind(i64::from(mr.additions))
        .bind(i64::from(mr.deletions))
        .bind(&mr.state)
        .bind(&mr.updated_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("id")?)
    }

    /// `None` => all projects. Most recently updated first.
    pub async fn list_mrs(&self, project_id: Option<i64>) -> Result<Vec<MergeRequest>> {
        let rows = match project_id {
            Some(pid) => {
                sqlx::query(
                    "SELECT * FROM merge_requests WHERE project_id = ?
                     ORDER BY updated_at DESC, id DESC",
                )
                .bind(pid)
                .fetch_all(&self.pool)
                .await?
            }
            None => {
                sqlx::query("SELECT * FROM merge_requests ORDER BY updated_at DESC, id DESC")
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        rows.iter().map(row_mr).collect()
    }

    pub async fn get_mr(&self, id: i64) -> Result<MergeRequest> {
        let row = sqlx::query("SELECT * FROM merge_requests WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| WardenError::NotFound(format!("merge request {id}")))?;
        row_mr(&row)
    }

    pub async fn set_last_reviewed(&self, mr_id: i64, sha: &str) -> Result<()> {
        let res = sqlx::query("UPDATE merge_requests SET last_reviewed_sha = ? WHERE id = ?")
            .bind(sha)
            .bind(mr_id)
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(WardenError::NotFound(format!("merge request {mr_id}")));
        }
        Ok(())
    }

    // --- runs ---

    /// New run in `Queued` with `started_at = now`.
    pub async fn create_run(
        &self,
        mr_id: i64,
        base: &str,
        head: &str,
        since: Option<&str>,
        model: &str,
    ) -> Result<ReviewRun> {
        let started_at = now_ts();
        let row = sqlx::query(
            "INSERT INTO review_runs (mr_id, base_sha, head_sha, since_sha, status, model, started_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(mr_id)
        .bind(base)
        .bind(head)
        .bind(since)
        .bind(RunStatus::Queued.as_str())
        .bind(model)
        .bind(&started_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(ReviewRun {
            id: row.try_get("id")?,
            mr_id,
            base_sha: base.to_string(),
            head_sha: head.to_string(),
            since_sha: since.map(str::to_string),
            status: RunStatus::Queued,
            model: model.to_string(),
            turns: 0,
            duration_ms: 0,
            cli_session_id: None,
            summary: None,
            verdict: None,
            error: None,
            started_at,
            ended_at: None,
        })
    }

    /// Writes every field of the run except `id` and `mr_id`. Missing id => `NotFound`.
    pub async fn update_run(&self, run: &ReviewRun) -> Result<()> {
        let res = sqlx::query(
            "UPDATE review_runs SET base_sha = ?, head_sha = ?, since_sha = ?, status = ?, model = ?,
                    turns = ?, duration_ms = ?, cli_session_id = ?, summary = ?, verdict = ?,
                    error = ?, started_at = ?, ended_at = ?
             WHERE id = ?",
        )
        .bind(&run.base_sha)
        .bind(&run.head_sha)
        .bind(run.since_sha.as_deref())
        .bind(run.status.as_str())
        .bind(&run.model)
        .bind(i64::from(run.turns))
        .bind(u64_to_i64(run.duration_ms))
        .bind(run.cli_session_id.as_deref())
        .bind(run.summary.as_deref())
        .bind(run.verdict.as_deref())
        .bind(run.error.as_deref())
        .bind(&run.started_at)
        .bind(run.ended_at.as_deref())
        .bind(run.id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(WardenError::NotFound(format!("run {}", run.id)));
        }
        Ok(())
    }

    pub async fn get_run(&self, id: i64) -> Result<ReviewRun> {
        let row = sqlx::query("SELECT * FROM review_runs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| WardenError::NotFound(format!("run {id}")))?;
        row_run(&row)
    }

    /// Newest first.
    pub async fn list_runs(&self, mr_id: Option<i64>, limit: u32) -> Result<Vec<ReviewRun>> {
        let rows = match mr_id {
            Some(mid) => {
                sqlx::query("SELECT * FROM review_runs WHERE mr_id = ? ORDER BY id DESC LIMIT ?")
                    .bind(mid)
                    .bind(i64::from(limit))
                    .fetch_all(&self.pool)
                    .await?
            }
            None => {
                sqlx::query("SELECT * FROM review_runs ORDER BY id DESC LIMIT ?")
                    .bind(i64::from(limit))
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        rows.iter().map(row_run).collect()
    }

    pub async fn latest_run_for_mr(&self, mr_id: i64) -> Result<Option<ReviewRun>> {
        let row = sqlx::query("SELECT * FROM review_runs WHERE mr_id = ? ORDER BY id DESC LIMIT 1")
            .bind(mr_id)
            .fetch_optional(&self.pool)
            .await?;
        row.as_ref().map(row_run).transpose()
    }

    /// Count of runs with `started_at >= ts` (RFC 3339).
    pub async fn runs_started_since(&self, ts: &str) -> Result<u32> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM review_runs WHERE started_at >= ?")
            .bind(normalize_ts(ts))
            .fetch_one(&self.pool)
            .await?;
        count_col(&row)
    }

    /// Integration addition: count of reviews that used (or are using) Claude since `ts`: runs past
    /// `queued`, excluding skipped ones and cancelled/failed runs that never reached the engine
    /// (`turns = 0`). Used by the hourly usage gate and `get_usage`.
    pub async fn reviews_started_since(&self, ts: &str) -> Result<u32> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS n FROM review_runs
             WHERE started_at >= ?
               AND (status NOT IN ('queued', 'skipped', 'cancelled', 'failed') OR turns > 0)",
        )
        .bind(normalize_ts(ts))
        .fetch_one(&self.pool)
        .await?;
        count_col(&row)
    }

    /// Integration addition: runs in one of `statuses` (oldest first), e.g. queued runs to resume.
    pub async fn runs_with_status(&self, statuses: &[RunStatus]) -> Result<Vec<ReviewRun>> {
        if statuses.is_empty() {
            return Ok(Vec::new());
        }
        let list = statuses
            .iter()
            .map(|s| format!("'{}'", s.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT * FROM review_runs WHERE status IN ({list}) ORDER BY id ASC");
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_run).collect()
    }

    // --- findings ---

    /// Integration addition: sets status and guardrail reason together (budget overflow etc.).
    pub async fn set_finding_guardrail(
        &self,
        id: i64,
        status: FindingStatus,
        reason: Option<&str>,
    ) -> Result<()> {
        let res = sqlx::query("UPDATE findings SET status = ?, guardrail_reason = ? WHERE id = ?")
            .bind(status.as_str())
            .bind(reason)
            .bind(id)
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(WardenError::NotFound(format!("finding {id}")));
        }
        Ok(())
    }

    /// Stores a finding (position as JSON) and returns it with its new id.
    pub async fn insert_finding(
        &self,
        run_id: i64,
        f: &NewFinding,
        status: FindingStatus,
        reason: Option<&str>,
        fingerprint: &str,
        position: Option<&DiffPosition>,
    ) -> Result<Finding> {
        let pos_json = position.map(serde_json::to_string).transpose()?;
        let now = now_ts();
        let posted_at = (status == FindingStatus::Posted).then(|| now.clone());
        let row = sqlx::query(
            "INSERT INTO findings (run_id, file, line, side, severity, category, rule_id, title, body,
                    suggestion, evidence, confidence, source, status, guardrail_reason, fingerprint,
                    position, created_at, posted_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(run_id)
        .bind(&f.file)
        .bind(i64::from(f.line))
        .bind(&f.side)
        .bind(f.severity.as_str())
        .bind(&f.category)
        .bind(f.rule_id.as_deref())
        .bind(&f.title)
        .bind(&f.body)
        .bind(f.suggestion.as_deref())
        .bind(f.evidence.as_deref())
        .bind(f64::from(f.confidence))
        .bind(&f.source)
        .bind(status.as_str())
        .bind(reason)
        .bind(fingerprint)
        .bind(pos_json)
        .bind(&now)
        .bind(posted_at)
        .fetch_one(&self.pool)
        .await?;
        Ok(Finding {
            id: row.try_get("id")?,
            run_id,
            file: f.file.clone(),
            line: f.line,
            side: f.side.clone(),
            severity: f.severity,
            category: f.category.clone(),
            rule_id: f.rule_id.clone(),
            title: f.title.clone(),
            body: f.body.clone(),
            suggestion: f.suggestion.clone(),
            evidence: f.evidence.clone(),
            confidence: f.confidence,
            source: f.source.clone(),
            status,
            guardrail_reason: reason.map(str::to_string),
            fingerprint: fingerprint.to_string(),
            forge_note_id: None,
            position: position.cloned(),
        })
    }

    pub async fn get_finding(&self, id: i64) -> Result<Finding> {
        let row = sqlx::query("SELECT * FROM findings WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| WardenError::NotFound(format!("finding {id}")))?;
        row_finding(&row)
    }

    /// In insertion order.
    pub async fn list_findings(&self, run_id: i64) -> Result<Vec<Finding>> {
        let rows = sqlx::query("SELECT * FROM findings WHERE run_id = ? ORDER BY id")
            .bind(run_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_finding).collect()
    }

    /// Sets `posted_at` when status becomes Posted. `forge_note_id = None` keeps the stored note id
    /// (so resolving or dismissing a posted finding does not lose its discussion id).
    pub async fn update_finding_status(
        &self,
        id: i64,
        status: FindingStatus,
        forge_note_id: Option<&str>,
    ) -> Result<()> {
        let posted_at = (status == FindingStatus::Posted).then(now_ts);
        let res = sqlx::query(
            "UPDATE findings SET status = ?,
                    forge_note_id = COALESCE(?, forge_note_id),
                    posted_at = COALESCE(?, posted_at)
             WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(forge_note_id)
        .bind(posted_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(WardenError::NotFound(format!("finding {id}")));
        }
        Ok(())
    }

    pub async fn update_finding_text(
        &self,
        id: i64,
        title: &str,
        body: &str,
        suggestion: Option<&str>,
    ) -> Result<()> {
        let res =
            sqlx::query("UPDATE findings SET title = ?, body = ?, suggestion = ? WHERE id = ?")
                .bind(title)
                .bind(body)
                .bind(suggestion)
                .bind(id)
                .execute(&self.pool)
                .await?;
        if res.rows_affected() == 0 {
            return Err(WardenError::NotFound(format!("finding {id}")));
        }
        Ok(())
    }

    /// Fingerprints the MR already "has": findings that were posted (or posted and resolved) or
    /// that a human dismissed. Draft / Approved findings of earlier runs were never posted, so they
    /// do not count (a re-found pending finding must not become a "duplicate of existing thread").
    pub async fn fingerprints_for_mr(&self, mr_id: i64) -> Result<HashSet<String>> {
        let rows = sqlx::query(
            "SELECT DISTINCT f.fingerprint AS fingerprint
             FROM findings f JOIN review_runs r ON r.id = f.run_id
             WHERE r.mr_id = ? AND f.status IN (?, ?, ?)",
        )
        .bind(mr_id)
        .bind(FindingStatus::Posted.as_str())
        .bind(FindingStatus::Resolved.as_str())
        .bind(FindingStatus::Dismissed.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| r.try_get::<String, _>("fingerprint").map_err(Into::into))
            .collect()
    }

    /// Marks still-pending Draft findings with `fingerprint` in the MR's OTHER runs as Rejected
    /// with `reason` (a newer run re-found them with a fresh position). Returns the affected run ids.
    pub async fn supersede_drafts(
        &self,
        mr_id: i64,
        fingerprint: &str,
        keep_run_id: i64,
        reason: &str,
    ) -> Result<Vec<i64>> {
        let rows = sqlx::query(
            "UPDATE findings SET status = ?, guardrail_reason = ?
             WHERE fingerprint = ? AND status = ? AND run_id != ?
               AND run_id IN (SELECT id FROM review_runs WHERE mr_id = ?)
             RETURNING run_id",
        )
        .bind(FindingStatus::Rejected.as_str())
        .bind(reason)
        .bind(fingerprint)
        .bind(FindingStatus::Draft.as_str())
        .bind(keep_run_id)
        .bind(mr_id)
        .fetch_all(&self.pool)
        .await?;
        let mut ids: Vec<i64> = rows
            .iter()
            .map(|r| r.try_get::<i64, _>("run_id"))
            .collect::<std::result::Result<_, _>>()?;
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }

    /// Findings across the MR's runs whose status is Posted (oldest first).
    pub async fn posted_findings_for_mr(&self, mr_id: i64) -> Result<Vec<Finding>> {
        let rows = sqlx::query(
            "SELECT f.* FROM findings f JOIN review_runs r ON r.id = f.run_id
             WHERE r.mr_id = ? AND f.status = ?
             ORDER BY f.id",
        )
        .bind(mr_id)
        .bind(FindingStatus::Posted.as_str())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_finding).collect()
    }

    // --- ci jobs ---

    /// `job.id` is ignored; returns the new id.
    pub async fn insert_ci_job(&self, job: &CiJob) -> Result<i64> {
        let row = sqlx::query(
            "INSERT INTO ci_jobs (run_id, name, kind, command, status, duration_ms, log_path,
                                  failure_excerpt, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(job.run_id)
        .bind(&job.name)
        .bind(&job.kind)
        .bind(&job.command)
        .bind(job.status.as_str())
        .bind(u64_to_i64(job.duration_ms))
        .bind(&job.log_path)
        .bind(job.failure_excerpt.as_deref())
        .bind(now_ts())
        .fetch_one(&self.pool)
        .await?;
        Ok(row.try_get("id")?)
    }

    pub async fn update_ci_job(&self, job: &CiJob) -> Result<()> {
        let res = sqlx::query(
            "UPDATE ci_jobs SET name = ?, kind = ?, command = ?, status = ?, duration_ms = ?,
                    log_path = ?, failure_excerpt = ?
             WHERE id = ?",
        )
        .bind(&job.name)
        .bind(&job.kind)
        .bind(&job.command)
        .bind(job.status.as_str())
        .bind(u64_to_i64(job.duration_ms))
        .bind(&job.log_path)
        .bind(job.failure_excerpt.as_deref())
        .bind(job.id)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() == 0 {
            return Err(WardenError::NotFound(format!("ci job {}", job.id)));
        }
        Ok(())
    }

    pub async fn get_ci_job(&self, id: i64) -> Result<CiJob> {
        let row = sqlx::query("SELECT * FROM ci_jobs WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| WardenError::NotFound(format!("ci job {id}")))?;
        row_ci_job(&row)
    }

    /// In insertion order.
    pub async fn list_ci_jobs(&self, run_id: i64) -> Result<Vec<CiJob>> {
        let rows = sqlx::query("SELECT * FROM ci_jobs WHERE run_id = ? ORDER BY id")
            .bind(run_id)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_ci_job).collect()
    }

    // --- audit ---

    /// Appends an audit event with `created_at = now`.
    pub async fn audit(
        &self,
        run_id: Option<i64>,
        kind: &str,
        payload: serde_json::Value,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO audit_events (run_id, kind, payload, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(run_id)
        .bind(kind)
        .bind(serde_json::to_string(&payload)?)
        .bind(now_ts())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Newest first; `run_id = None` => all runs.
    pub async fn list_audit(&self, run_id: Option<i64>, limit: u32) -> Result<Vec<AuditEvent>> {
        let rows = match run_id {
            Some(rid) => {
                sqlx::query("SELECT * FROM audit_events WHERE run_id = ? ORDER BY id DESC LIMIT ?")
                    .bind(rid)
                    .bind(i64::from(limit))
                    .fetch_all(&self.pool)
                    .await?
            }
            None => {
                sqlx::query("SELECT * FROM audit_events ORDER BY id DESC LIMIT ?")
                    .bind(i64::from(limit))
                    .fetch_all(&self.pool)
                    .await?
            }
        };
        rows.iter().map(row_audit).collect()
    }

    /// Count of findings posted (`posted_at >= ts`), used for max_notes_per_hour.
    pub async fn posts_since(&self, ts: &str) -> Result<u32> {
        let row = sqlx::query(
            "SELECT COUNT(*) AS n FROM findings WHERE posted_at IS NOT NULL AND posted_at >= ?",
        )
        .bind(normalize_ts(ts))
        .fetch_one(&self.pool)
        .await?;
        count_col(&row)
    }

    // --- stats / rule catalogue / approval queue (UX redesign) ---

    /// Runs with `started_at >= since` (any RFC 3339), oldest first.
    pub async fn runs_started_from(&self, since: &str) -> Result<Vec<ReviewRun>> {
        let rows =
            sqlx::query("SELECT * FROM review_runs WHERE started_at >= ? ORDER BY started_at, id")
                .bind(normalize_ts(since))
                .fetch_all(&self.pool)
                .await?;
        rows.iter().map(row_run).collect()
    }

    /// Findings of runs started at or after `since`, optionally limited to one project, each with
    /// its run's `started_at` and its own `posted_at`. Oldest run first, then insertion order.
    pub async fn findings_from(
        &self,
        since: &str,
        project_id: Option<i64>,
    ) -> Result<Vec<(Finding, String, Option<String>)>> {
        let rows = sqlx::query(
            "SELECT f.*, r.started_at AS run_started_at
             FROM findings f
             JOIN review_runs r ON r.id = f.run_id
             JOIN merge_requests m ON m.id = r.mr_id
             WHERE r.started_at >= ? AND (? IS NULL OR m.project_id = ?)
             ORDER BY r.started_at, f.id",
        )
        .bind(normalize_ts(since))
        .bind(project_id)
        .bind(project_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                Ok((
                    row_finding(r)?,
                    r.try_get::<String, _>("run_started_at")?,
                    r.try_get::<Option<String>, _>("posted_at")?,
                ))
            })
            .collect()
    }

    /// History rows: newest `limit` runs (by id) joined with their MR and project, with finding
    /// counts (draft = draft|approved, posted = posted|resolved, rejected) and a CI summary per run.
    /// No git work. Local projects (`base_url = local_base_url`) get `project_id = 0`.
    pub async fn list_run_rows(&self, limit: u32, local_base_url: &str) -> Result<Vec<RunRow>> {
        let rows = sqlx::query(
            "SELECT r.*,
                    m.iid AS rr_iid, m.title AS rr_title, m.author AS rr_author,
                    m.web_url AS rr_web_url, m.draft AS rr_draft, m.project_id AS rr_project_id,
                    COALESCE(p.name, '') AS rr_project_name,
                    COALESCE(p.base_url, '') AS rr_base_url,
                    (SELECT COUNT(*) FROM findings f WHERE f.run_id = r.id
                       AND f.status IN ('draft', 'approved')) AS n_draft,
                    (SELECT COUNT(*) FROM findings f WHERE f.run_id = r.id
                       AND f.status IN ('posted', 'resolved')) AS n_posted,
                    (SELECT COUNT(*) FROM findings f WHERE f.run_id = r.id
                       AND f.status = 'rejected') AS n_rejected,
                    (SELECT COUNT(*) FROM ci_jobs j WHERE j.run_id = r.id
                       AND j.status = 'passed') AS ci_passed,
                    (SELECT COUNT(*) FROM ci_jobs j WHERE j.run_id = r.id
                       AND j.status IN ('failed', 'timed_out')) AS ci_failed,
                    (SELECT COUNT(*) FROM ci_jobs j WHERE j.run_id = r.id
                       AND j.status IN ('running', 'pending')) AS ci_running,
                    (SELECT COUNT(*) FROM ci_jobs j WHERE j.run_id = r.id) AS ci_total
             FROM review_runs r
             JOIN merge_requests m ON m.id = r.mr_id
             LEFT JOIN projects p ON p.id = m.project_id
             ORDER BY r.id DESC
             LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| {
                let local = r.try_get::<String, _>("rr_base_url")? == local_base_url;
                Ok(RunRow {
                    run: row_run(r)?,
                    mr_iid: u64_col(r, "rr_iid")?,
                    mr_title: r.try_get("rr_title")?,
                    author: r.try_get("rr_author")?,
                    web_url: r.try_get("rr_web_url")?,
                    draft: r.try_get::<i64, _>("rr_draft")? != 0,
                    project_id: if local {
                        0
                    } else {
                        r.try_get("rr_project_id")?
                    },
                    project_name: r.try_get("rr_project_name")?,
                    finding_counts: FindingCounts {
                        draft: u32_col(r, "n_draft")?,
                        posted: u32_col(r, "n_posted")?,
                        rejected: u32_col(r, "n_rejected")?,
                    },
                    ci_summary: CiSummary {
                        passed: u32_col(r, "ci_passed")?,
                        failed: u32_col(r, "ci_failed")?,
                        running: u32_col(r, "ci_running")?,
                        total: u32_col(r, "ci_total")?,
                    },
                })
            })
            .collect()
    }

    /// The project's newest `limit` runs that have at least one finding (newest first).
    pub async fn recent_runs_with_findings(
        &self,
        project_id: i64,
        limit: u32,
    ) -> Result<Vec<ReviewRun>> {
        let rows = sqlx::query(
            "SELECT r.* FROM review_runs r
             JOIN merge_requests m ON m.id = r.mr_id
             WHERE m.project_id = ? AND EXISTS (SELECT 1 FROM findings f WHERE f.run_id = r.id)
             ORDER BY r.started_at DESC, r.id DESC
             LIMIT ?",
        )
        .bind(project_id)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_run).collect()
    }

    /// Draft/Approved findings of the latest run of every open MR (MR iid desc, then insertion
    /// order), each with the MR id.
    pub async fn pending_findings(&self) -> Result<Vec<(Finding, i64)>> {
        let rows = sqlx::query(
            "SELECT f.*, m.id AS pending_mr_id
             FROM findings f
             JOIN review_runs r ON r.id = f.run_id
             JOIN merge_requests m ON m.id = r.mr_id
             WHERE m.state = 'opened'
               AND f.status IN ('draft', 'approved')
               AND r.id = (SELECT MAX(r2.id) FROM review_runs r2 WHERE r2.mr_id = m.id)
             ORDER BY m.iid DESC, m.id, f.id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|r| Ok((row_finding(r)?, r.try_get::<i64, _>("pending_mr_id")?)))
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Row mapping
// ---------------------------------------------------------------------------------------------

fn u64_to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn u32_col(r: &SqliteRow, col: &str) -> Result<u32> {
    let v: i64 = r.try_get(col)?;
    Ok(u32::try_from(v.max(0)).unwrap_or(u32::MAX))
}

fn u64_col(r: &SqliteRow, col: &str) -> Result<u64> {
    let v: i64 = r.try_get(col)?;
    Ok(u64::try_from(v).unwrap_or(0))
}

fn count_col(r: &SqliteRow) -> Result<u32> {
    u32_col(r, "n")
}

fn json_col<T: serde::de::DeserializeOwned + Default>(r: &SqliteRow, col: &str) -> Result<T> {
    let raw: Option<String> = r.try_get(col)?;
    match raw.as_deref().map(str::trim) {
        None | Some("") => Ok(T::default()),
        Some(s) => Ok(serde_json::from_str(s)?),
    }
}

fn row_project(r: &SqliteRow) -> Result<Project> {
    Ok(Project {
        id: r.try_get("id")?,
        forge: ForgeKind::from_str(&r.try_get::<String, _>("forge")?)?,
        base_url: r.try_get("base_url")?,
        remote_id: r.try_get("remote_id")?,
        name: r.try_get("name")?,
        local_path: r.try_get("local_path")?,
        target_branches: json_col(r, "target_branches")?,
        mode: PostingMode::from_str(&r.try_get::<String, _>("mode")?)?,
        poll_interval_secs: u32_col(r, "poll_interval_secs")?,
        enabled: r.try_get("enabled")?,
    })
}

fn row_mr(r: &SqliteRow) -> Result<MergeRequest> {
    Ok(MergeRequest {
        id: r.try_get("id")?,
        project_id: r.try_get("project_id")?,
        iid: u64_col(r, "iid")?,
        title: r.try_get("title")?,
        description: r.try_get("description")?,
        author: r.try_get("author")?,
        source_branch: r.try_get("source_branch")?,
        target_branch: r.try_get("target_branch")?,
        web_url: r.try_get("web_url")?,
        draft: r.try_get("draft")?,
        labels: json_col(r, "labels")?,
        head_sha: r.try_get("head_sha")?,
        base_sha: r.try_get("base_sha")?,
        start_sha: r.try_get("start_sha")?,
        last_reviewed_sha: r.try_get("last_reviewed_sha")?,
        additions: u32_col(r, "additions")?,
        deletions: u32_col(r, "deletions")?,
        state: r.try_get("state")?,
        updated_at: r.try_get("updated_at")?,
    })
}

fn row_run(r: &SqliteRow) -> Result<ReviewRun> {
    Ok(ReviewRun {
        id: r.try_get("id")?,
        mr_id: r.try_get("mr_id")?,
        base_sha: r.try_get("base_sha")?,
        head_sha: r.try_get("head_sha")?,
        since_sha: r.try_get("since_sha")?,
        status: RunStatus::from_str(&r.try_get::<String, _>("status")?)?,
        model: r.try_get("model")?,
        turns: u32_col(r, "turns")?,
        duration_ms: u64_col(r, "duration_ms")?,
        cli_session_id: r.try_get("cli_session_id")?,
        summary: r.try_get("summary")?,
        verdict: r.try_get("verdict")?,
        error: r.try_get("error")?,
        started_at: r.try_get("started_at")?,
        ended_at: r.try_get("ended_at")?,
    })
}

fn row_finding(r: &SqliteRow) -> Result<Finding> {
    let position: Option<String> = r.try_get("position")?;
    let position = match position.as_deref().map(str::trim) {
        None | Some("") | Some("null") => None,
        Some(s) => Some(serde_json::from_str::<DiffPosition>(s)?),
    };
    let confidence: f64 = r.try_get("confidence")?;
    Ok(Finding {
        id: r.try_get("id")?,
        run_id: r.try_get("run_id")?,
        file: r.try_get("file")?,
        line: u32_col(r, "line")?,
        side: r.try_get("side")?,
        severity: Severity::from_str(&r.try_get::<String, _>("severity")?)?,
        category: r.try_get("category")?,
        rule_id: r.try_get("rule_id")?,
        title: r.try_get("title")?,
        body: r.try_get("body")?,
        suggestion: r.try_get("suggestion")?,
        evidence: r.try_get("evidence")?,
        confidence: confidence as f32,
        source: r.try_get("source")?,
        status: FindingStatus::from_str(&r.try_get::<String, _>("status")?)?,
        guardrail_reason: r.try_get("guardrail_reason")?,
        fingerprint: r.try_get("fingerprint")?,
        forge_note_id: r.try_get("forge_note_id")?,
        position,
    })
}

fn row_ci_job(r: &SqliteRow) -> Result<CiJob> {
    Ok(CiJob {
        id: r.try_get("id")?,
        run_id: r.try_get("run_id")?,
        name: r.try_get("name")?,
        kind: r.try_get("kind")?,
        command: r.try_get("command")?,
        status: CiStatus::from_str(&r.try_get::<String, _>("status")?)?,
        duration_ms: u64_col(r, "duration_ms")?,
        log_path: r.try_get("log_path")?,
        failure_excerpt: r.try_get("failure_excerpt")?,
    })
}

fn row_audit(r: &SqliteRow) -> Result<AuditEvent> {
    let raw: String = r.try_get("payload")?;
    let payload = serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw));
    Ok(AuditEvent {
        id: r.try_get("id")?,
        run_id: r.try_get("run_id")?,
        kind: r.try_get("kind")?,
        payload,
        created_at: r.try_get("created_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_normalise() {
        assert_eq!(
            normalize_ts("2026-11-25T10:04:05+00:00"),
            "2026-11-25T10:04:05.000Z"
        );
        assert_eq!(
            normalize_ts("2026-11-25T12:04:05.123456+02:00"),
            "2026-11-25T10:04:05.123Z"
        );
        assert_eq!(normalize_ts("not a date"), "not a date");
        let now = now_ts();
        assert_eq!(now.len(), "2026-11-25T10:04:05.123Z".len());
        assert!(now.ends_with('Z'));
    }
}
