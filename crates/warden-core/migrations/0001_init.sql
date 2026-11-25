-- Warden initial schema. Owned by poster (db.rs); later schema changes go in new numbered files.
-- Conventions: enums stored as snake_case text (types.rs `as_str()`), Vec/struct fields as JSON text,
-- booleans as INTEGER 0/1, timestamps as RFC 3339 text in one shape: UTC, milliseconds, 'Z'
-- (2026-11-25T10:04:05.123Z, db::now_ts) so text comparison orders them.

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL                       -- JSON (key "app" = AppSettings)
);

CREATE TABLE IF NOT EXISTS projects (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    forge              TEXT    NOT NULL,      -- gitlab | github
    base_url           TEXT    NOT NULL,
    remote_id          TEXT    NOT NULL,
    name               TEXT    NOT NULL,
    local_path         TEXT    NOT NULL DEFAULT '',
    target_branches    TEXT    NOT NULL DEFAULT '[]',   -- JSON array of globs
    mode               TEXT    NOT NULL DEFAULT 'draft',-- draft | assist | auto
    poll_interval_secs INTEGER NOT NULL DEFAULT 120,
    enabled            INTEGER NOT NULL DEFAULT 1,
    created_at         TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (forge, base_url, remote_id)
);

CREATE TABLE IF NOT EXISTS project_overrides (
    project_id INTEGER PRIMARY KEY NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    overrides  TEXT    NOT NULL DEFAULT '{}', -- JSON merged over .warden.yml
    updated_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS merge_requests (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id        INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    iid               INTEGER NOT NULL,
    title             TEXT    NOT NULL DEFAULT '',
    description       TEXT    NOT NULL DEFAULT '',
    author            TEXT    NOT NULL DEFAULT '',
    source_branch     TEXT    NOT NULL DEFAULT '',
    target_branch     TEXT    NOT NULL DEFAULT '',
    web_url           TEXT    NOT NULL DEFAULT '',
    draft             INTEGER NOT NULL DEFAULT 0,
    labels            TEXT    NOT NULL DEFAULT '[]',    -- JSON array
    head_sha          TEXT    NOT NULL DEFAULT '',
    base_sha          TEXT    NOT NULL DEFAULT '',
    start_sha         TEXT    NOT NULL DEFAULT '',
    last_reviewed_sha TEXT,
    additions         INTEGER NOT NULL DEFAULT 0,
    deletions         INTEGER NOT NULL DEFAULT 0,
    state             TEXT    NOT NULL DEFAULT 'opened',-- opened | merged | closed
    updated_at        TEXT    NOT NULL DEFAULT '',
    UNIQUE (project_id, iid)
);

CREATE TABLE IF NOT EXISTS review_runs (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    mr_id          INTEGER NOT NULL REFERENCES merge_requests(id) ON DELETE CASCADE,
    base_sha       TEXT    NOT NULL DEFAULT '',
    head_sha       TEXT    NOT NULL DEFAULT '',
    since_sha      TEXT,
    status         TEXT    NOT NULL DEFAULT 'queued',
    model          TEXT    NOT NULL DEFAULT '',
    turns          INTEGER NOT NULL DEFAULT 0,
    duration_ms    INTEGER NOT NULL DEFAULT 0,
    cli_session_id TEXT,
    summary        TEXT,
    verdict        TEXT,                      -- approve | comment | request_changes
    error          TEXT,
    started_at     TEXT    NOT NULL,
    ended_at       TEXT
);
CREATE INDEX IF NOT EXISTS idx_review_runs_mr ON review_runs(mr_id);
CREATE INDEX IF NOT EXISTS idx_review_runs_started ON review_runs(started_at);

CREATE TABLE IF NOT EXISTS findings (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id           INTEGER NOT NULL REFERENCES review_runs(id) ON DELETE CASCADE,
    file             TEXT    NOT NULL,
    line             INTEGER NOT NULL,
    side             TEXT    NOT NULL DEFAULT 'new',     -- new | old
    severity         TEXT    NOT NULL,                   -- blocker | major | minor | nit
    category         TEXT    NOT NULL DEFAULT '',
    rule_id          TEXT,
    title            TEXT    NOT NULL,
    body             TEXT    NOT NULL,
    suggestion       TEXT,
    evidence         TEXT,
    confidence       REAL    NOT NULL DEFAULT 0,
    source           TEXT    NOT NULL DEFAULT 'claude',  -- claude | rule | ci
    status           TEXT    NOT NULL DEFAULT 'draft',
    guardrail_reason TEXT,
    fingerprint      TEXT    NOT NULL,
    forge_note_id    TEXT,
    position         TEXT,                               -- JSON DiffPosition
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    posted_at        TEXT
);
CREATE INDEX IF NOT EXISTS idx_findings_run ON findings(run_id);
CREATE INDEX IF NOT EXISTS idx_findings_fingerprint ON findings(fingerprint);
CREATE INDEX IF NOT EXISTS idx_findings_status ON findings(status);
CREATE INDEX IF NOT EXISTS idx_findings_posted_at ON findings(posted_at);

CREATE TABLE IF NOT EXISTS ci_jobs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id          INTEGER NOT NULL REFERENCES review_runs(id) ON DELETE CASCADE,
    name            TEXT    NOT NULL,
    kind            TEXT    NOT NULL DEFAULT 'recipe',   -- recipe | gitlab_ci_local | check
    command         TEXT    NOT NULL DEFAULT '',
    status          TEXT    NOT NULL DEFAULT 'pending',
    duration_ms     INTEGER NOT NULL DEFAULT 0,
    log_path        TEXT    NOT NULL DEFAULT '',
    failure_excerpt TEXT,
    created_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_ci_jobs_run ON ci_jobs(run_id);

CREATE TABLE IF NOT EXISTS audit_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id     INTEGER REFERENCES review_runs(id) ON DELETE SET NULL,
    kind       TEXT    NOT NULL,              -- tool_call | guardrail | post | engine | error
    payload    TEXT    NOT NULL DEFAULT '{}', -- JSON
    created_at TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_audit_run ON audit_events(run_id);
CREATE INDEX IF NOT EXISTS idx_audit_kind_created ON audit_events(kind, created_at);
