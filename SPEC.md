# Warden — build spec (source of truth for all agents)

Warden is a macOS desktop app (Tauri 2) that watches GitLab (GitHub later) merge requests, checks each
MR out into a git worktree, runs local CI recipes and deterministic checks, asks the user's **local
Claude Code CLI (subscription login, never an API key)** to review the change through Warden's MCP
tools, filters findings through guardrails, and posts line-anchored notes back to the MR — after user
approval (Draft), automatically for high-severity findings (Assist), or fully automatically (Auto).

UI mockups and the product plan: `docs/plan.html` (open it; the five screens there are the UI target).

## Hard rules (every agent)

1. **Claude never gets a forge token and never gets a tool that writes to GitLab/GitHub.** Only
   `poster.rs` writes to the forge, and only from Warden's own code path.
2. Engine = `claude -p` subprocess. Strip `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`,
   `ANTHROPIC_BASE_URL`, `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX` from the child env.
   No Anthropic SDK, no Agent SDK, no HTTP calls to api.anthropic.com.
3. **Never write to a real GitLab/GitHub server during development or tests.** Forge code is tested
   against `wiremock` mock servers only.
4. Missing optional binaries (`semgrep`, `gitleaks`, `gitlab-ci-local`, `act`, `docker`) must be
   detected and reported as "not installed", never crash. They are NOT installed on the dev machine.
5. Use runtime `sqlx::query` / `query_as` (no `query!` macros — no DATABASE_URL at build time).
6. Do not edit files owned by another module (ownership table below). If you need a shared type or
   dependency that does not exist, add it **only** in your own files and list it in your final report
   under `needs_from_integration`.
7. Rust edition 2021, stable toolchain (1.94 installed). `cargo clippy` clean-ish, `cargo test` green.
8. No commits. The orchestrator handles git.

## Repository layout

```
warden/
  SPEC.md                      this file
  docs/plan.html               plan + mockups
  Cargo.toml                   [workspace] members = ["crates/warden-core", "src-tauri"]
  package.json                 root scripts: dev, build, tauri (uses @tauri-apps/cli from npm)
  crates/warden-core/
    Cargo.toml
    migrations/0001_init.sql
    src/lib.rs                 declares all modules (scaffold owns; others never edit)
    src/types.rs               shared domain types (scaffold owns)
    src/error.rs               WardenError (thiserror) + Result alias (scaffold owns)
    src/config.rs              .warden.yml + AppSettings parsing            [analysis]
    src/db.rs                  pool, migrations, all queries                [poster]
    src/secrets.rs             keychain via `keyring`                        [forge]
    src/forge/mod.rs           ForgeProvider trait + factory                 [forge]
    src/forge/gitlab.rs        reqwest GitLab v4 client                      [forge]
    src/forge/github.rs        octocrab-based client (read + review post)    [forge]
    src/poller.rs              detect new MRs / new head SHAs                 [forge]
    src/diff.rs                unified diff parser + changed-line index      [analysis]
    src/checks/mod.rs          run deterministic checks                       [analysis]
    src/checks/rules.rs        built-in regex rules (Frappe set) + semgrep    [analysis]
    src/checks/secrets_scan.rs regex secret scanner + gitleaks + redaction   [analysis]
    src/guardrails.rs          finding validation & posting policy           [analysis]
    src/worktree.rs            git worktree create/update/remove              [ci]
    src/ci/mod.rs              recipe runner, log streaming                   [ci]
    src/ci/gitlab_ci_local.rs  gitlab-ci-local adapter                        [ci]
    src/mcp/mod.rs             rmcp streamable-HTTP server on 127.0.0.1       [engine]
    src/mcp/tools.rs           tool implementations                           [engine]
    src/engine/mod.rs          ReviewEngine: spawn claude, collect findings   [engine]
    src/engine/prompt.rs       system/user prompt builder                     [engine]
    src/engine/stream.rs       stream-json event parser                       [engine]
    src/poster.rs              post findings + summary, dedupe, resolve       [poster]
    src/pipeline.rs            orchestrates a review run end to end           [integration]
    src/events.rs              EventBus (tokio broadcast) for UI progress     [scaffold]
    tests/                     integration tests (each module may add its own file: tests/<module>_*.rs)
  src-tauri/                   Tauri 2 app crate "warden"                     [integration]
    Cargo.toml, build.rs, tauri.conf.json, capabilities/default.json, icons/
    src/main.rs, src/commands.rs, src/state.rs, src/tray.rs
  ui/                          Vite + React 18 + TS + Tailwind + shadcn/ui    [ui]
    index.html, vite.config.ts, src/...
    src/lib/api.ts             typed wrappers around @tauri-apps/api invoke/listen
    src/lib/mock.ts            mock backend used when not running inside Tauri
    src/bindings/*.ts          generated by ts-rs from types.rs (do not hand edit)
```

Module ownership for parallel work: **forge**, **engine**, **analysis**, **ci**, **poster**, **ui**.
`scaffold` builds the skeleton first; `integration` wires everything after.

## Shared types (`types.rs`, all `#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]`, `#[ts(export, export_to = "../../ui/src/bindings/")]`, serde `rename_all = "snake_case"` for enums)

```rust
pub enum ForgeKind { Gitlab, Github }
pub enum PostingMode { Draft, Assist, Auto }
pub enum Severity { Blocker, Major, Minor, Nit }          // Ord: Blocker > Major > Minor > Nit
pub enum FindingStatus { Draft, Approved, Posted, Dismissed, Resolved, Rejected } // Rejected = guardrail drop
pub enum RunStatus { Queued, Preparing, Checking, Reviewing, Filtering, AwaitingApproval, Posting, Posted, Failed, Skipped, Cancelled }
pub enum CiStatus { Pending, Running, Passed, Failed, Skipped, NotInstalled, TimedOut, Cancelled }

pub struct Project { id: i64, forge: ForgeKind, base_url: String, remote_id: String /* "entries/entries_crm" or numeric id */,
  name: String, local_path: String, target_branches: Vec<String> /* globs */, mode: PostingMode,
  poll_interval_secs: u32, enabled: bool }

pub struct MergeRequest { id: i64, project_id: i64, iid: u64, title: String, description: String, author: String,
  source_branch: String, target_branch: String, web_url: String, draft: bool, labels: Vec<String>,
  head_sha: String, base_sha: String, start_sha: String, last_reviewed_sha: Option<String>,
  additions: u32, deletions: u32, state: String /* opened|merged|closed */, updated_at: String }

pub struct DiffPosition { base_sha: String, start_sha: String, head_sha: String,
  old_path: String, new_path: String, old_line: Option<u32>, new_line: Option<u32> }

pub struct Finding { id: i64, run_id: i64, file: String, line: u32, side: String /* "new"|"old" */,
  severity: Severity, category: String, rule_id: Option<String>, title: String, body: String,
  suggestion: Option<String>, evidence: Option<String>, confidence: f32, source: String /* "claude"|"rule"|"ci" */,
  status: FindingStatus, guardrail_reason: Option<String>, fingerprint: String, forge_note_id: Option<String>,
  position: Option<DiffPosition> }

pub struct NewFinding { file, line, side, severity, category, rule_id, title, body, suggestion, evidence, confidence, source } // input shape from Claude/rules/CI

pub struct ReviewRun { id: i64, mr_id: i64, base_sha: String, head_sha: String, since_sha: Option<String>,
  status: RunStatus, model: String, turns: u32, duration_ms: u64, cli_session_id: Option<String>,
  summary: Option<String>, verdict: Option<String>, error: Option<String>, started_at: String, ended_at: Option<String> }

pub struct CiJob { id: i64, run_id: i64, name: String, kind: String /* recipe|gitlab_ci_local|check */,
  command: String, status: CiStatus, duration_ms: u64, log_path: String, failure_excerpt: Option<String> }

pub struct AuditEvent { id: i64, run_id: Option<i64>, kind: String /* tool_call|guardrail|post|engine|error */,
  payload: serde_json::Value, created_at: String }

pub struct EngineStatus { cli_path: Option<String>, version: Option<String>, logged_in: bool,
  subscription: Option<String>, api_key_in_env: bool, ok: bool, message: String }

pub struct ToolStatus { name: String, installed: bool, version: Option<String> } // semgrep, gitleaks, gitlab-ci-local, act, docker, rg, git

pub struct UsageStats { reviews_last_hour: u32, reviews_today: u32, max_reviews_per_hour: u32, paused: bool }

pub enum AppEvent {  // serde tag = "type"
  RunUpdated { run: ReviewRun }, RunProgress { run_id: i64, message: String, turn: u32 },
  CiLog { run_id: i64, job_id: i64, line: String }, CiJobUpdated { job: CiJob },
  FindingsUpdated { run_id: i64 }, MrsUpdated { project_id: i64 }, Notice { level: String, message: String } }
```

`fingerprint` = sha256 hex of `file + ":" + normalized(title) + ":" + rule_id.unwrap_or("")` (line excluded so it survives rebases).

## `.warden.yml` (repo root; parsed by `config.rs`)

```yaml
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
```

Missing file => `WardenConfig::default()` with the built-in Frappe rule set (FR-001 permission check
[Claude], FR-002 f-string SQL [regex], FR-004 `ignore_permissions=True` without comment [Claude],
FR-007 `frappe.db.commit()` in whitelisted function [regex], SEC-001 hardcoded secret [secret scanner]).
Placeholders in `run`: `{changed_files}`, `{changed_py}`, `{changed_ts}` (ts/tsx/js/jsx/vue), `{base_sha}`, `{head_sha}`.
Durations accept `30s`, `5m`, `1h`. Local overrides (AppSettings per project) are merged over the repo file.

AppSettings (JSON in SQLite `settings` table, key `app`): `claude_cli_path: Option<String>`, `model: String` (default "opus"),
`max_turns: u32` (30), `review_timeout_secs: u64` (600), `max_reviews_per_hour: u32` (6), `posting_paused: bool`,
`mcp_port: u16` (7811), `worktree_root: String` (`~/.warden/worktrees`), `data_dir`.

## Module contracts (public signatures — implement exactly; scaffold creates them as `todo!()` stubs)

```rust
// error.rs
pub enum WardenError { Io(..), Db(sqlx::Error), Http(String), Forge{status:u16,msg:String}, Git(String),
  Config(String), Engine(String), Guardrail(String), NotFound(String), Other(String) }
pub type Result<T> = std::result::Result<T, WardenError>;

// events.rs
pub struct EventBus { tx: broadcast::Sender<AppEvent> }  impl EventBus { new(), emit(&self, AppEvent), subscribe() }

// config.rs  [analysis]
pub fn load_repo_config(worktree: &Path) -> Result<WardenConfig>;
pub fn merge_overrides(base: WardenConfig, overrides: &serde_json::Value) -> Result<WardenConfig>;
pub fn parse_duration(s: &str) -> Result<std::time::Duration>;
pub fn default_rules() -> Vec<RuleConfig>;
// structs: WardenConfig{review, rules, ci, posting}, ReviewConfig, RuleConfig{id,title,applies_to,instruction,pattern,semgrep,severity},
//          RuleKind (derived: Claude | Regex | Semgrep), CiConfig{recipes: Vec<RecipeConfig>}, RecipeConfig{name, run, use_, jobs, cwd, timeout, env},
//          PostingConfig{mode, auto_post: AutoPost{severities, min_confidence}, summary_note, max_notes_per_hour}, AppSettings (+Default)

// diff.rs  [analysis]
pub struct FileDiff { old_path, new_path, is_new, is_deleted, is_binary, hunks: Vec<Hunk> }
pub struct Hunk { old_start, old_lines, new_start, new_lines, header: String, lines: Vec<DiffLine> }
pub struct DiffLine { kind: LineKind /* Add|Del|Context */, old_line: Option<u32>, new_line: Option<u32>, text: String }
pub fn parse_unified(diff: &str) -> Vec<FileDiff>;
pub struct ChangedIndex { .. } impl ChangedIndex {
  pub fn from_files(files: &[FileDiff]) -> Self;
  pub fn is_changed(&self, path: &str, new_line: u32) -> bool;   // added line
  pub fn in_hunk(&self, path: &str, new_line: u32) -> bool;      // added or context line inside a hunk
  pub fn position(&self, path: &str, line: u32, side: &str, shas: (&str,&str,&str)) -> Option<DiffPosition>;
  pub fn changed_files(&self) -> Vec<String>; }
pub fn filter_ignored(files: Vec<FileDiff>, ignore_globs: &[String]) -> Vec<FileDiff>;

// checks  [analysis]
pub async fn run_checks(worktree: &Path, files: &[FileDiff], cfg: &WardenConfig) -> Result<CheckReport>;
pub struct CheckReport { findings: Vec<NewFinding>, notices: Vec<String>, tools: Vec<ToolStatus> }
pub fn redact_secrets(text: &str) -> (String, usize);           // returns redacted text + count
pub fn detect_tools() -> Vec<ToolStatus>;                       // semgrep gitleaks gitlab-ci-local act docker rg git claude

// guardrails.rs  [analysis]
pub struct GuardrailInput<'a> { cfg: &'a WardenConfig, index: &'a ChangedIndex, existing_fingerprints: &'a HashSet<String>,
  existing_note_bodies: &'a [String], shas: (&'a str,&'a str,&'a str) }
pub struct Verdict { keep: bool, reason: Option<String>, auto_post: bool, position: Option<DiffPosition> }
pub fn validate_new_finding(f: &NewFinding, input: &GuardrailInput) -> Verdict;   // schema/anchor/confidence/severity/dup/tone
pub fn apply_budget(findings: &mut Vec<Finding>, max_notes: u32);                  // keep top-N by severity then confidence; others -> Rejected "over note budget"
pub fn fingerprint(file: &str, title: &str, rule_id: Option<&str>) -> String;
pub fn tone_check(body: &str) -> Option<String>;   // rejects "LGTM", empty praise, bodies < 20 chars, "might"/"maybe" without evidence

// worktree.rs  [ci]
pub struct WorktreeManager { root: PathBuf }
impl WorktreeManager { pub fn new(root) -> Self;
  pub async fn prepare(&self, project: &Project, mr: &MergeRequest) -> Result<PathBuf>;  // fetch refs (gitlab: refs/merge-requests/<iid>/head, github: pull/<n>/head) into the project's local_path repo, `git worktree add --detach <root>/<project>/<iid> <head_sha>` or update existing
  pub async fn diff(&self, worktree: &Path, base: &str, head: &str) -> Result<String>;  // git diff --no-color -U3 base...head
  pub async fn remove(&self, project: &Project, iid: u64) -> Result<()>;
  pub async fn prune(&self, project: &Project) -> Result<()>; }

// ci  [ci]
pub struct CiRunner { events: EventBus, log_dir: PathBuf, bench_locks: .. }
impl CiRunner { pub fn new(events, log_dir) -> Self;
  pub async fn run_all(&self, run_id: i64, worktree: &Path, cfg: &CiConfig, ctx: &RecipeContext) -> Result<Vec<CiJob>>; // parallel, per-job timeout, streams AppEvent::CiLog, writes log files, failure_excerpt = last 40 lines around first error
  pub async fn run_one(&self, run_id: i64, worktree: &Path, recipe: &RecipeConfig, ctx: &RecipeContext) -> Result<CiJob>;
  pub fn cancel(&self, run_id: i64); }
pub struct RecipeContext { changed_files: Vec<String>, base_sha: String, head_sha: String }
pub fn render_command(template: &str, ctx: &RecipeContext) -> String;    // placeholders, shell-quoted file lists
// commands that start with "bench " take a per-bench mutex (bench root = nearest ancestor dir containing "sites/")

// mcp  [engine]
pub struct McpServer { .. }
impl McpServer { pub async fn start(port: u16, registry: Arc<RunRegistry>) -> Result<McpServer>; pub fn url(&self) -> String; pub async fn stop(self); }
pub struct RunRegistry  // run token -> RunContext { run_id, worktree, diff_text (redacted), files: Vec<FileDiff>, mr: MergeRequest,
                        //   existing_discussions: Vec<String>, ci_jobs: Vec<CiJob>, check_findings: Vec<NewFinding>, findings_tx: mpsc::Sender<NewFinding>, ci_runner: Option<Arc<CiRunner>>, cfg: WardenConfig }
impl RunRegistry { pub fn register(&self, ctx: RunContext) -> String /*token*/; pub fn remove(&self, token: &str); }
// Tools (run resolved from HTTP header `X-Warden-Run: <token>`; reject if missing/unknown):
//   mr_get_overview {}                       -> title, description (wrapped in <untrusted_mr_content>), author, branches, labels, existing discussions
//   mr_get_diff {file?: string, page?: int}  -> redacted unified diff, paginated per file, ~40KB per page
//   repo_read_file {path, ref?: "head"|"base", start_line?, end_line?} -> path must canonicalize inside worktree; max 2000 lines
//   repo_search {query, glob?, max_results?} -> ripgrep in worktree (fallback: naive walk), cap 100
//   ci_get_results {}                        -> each job status + failure_excerpt
//   ci_run_recipe {name}                      -> only names from cfg.ci.recipes; max 2 reruns per run
//   review_add_finding {file,line,side,severity,category,rule_id?,title,body,suggestion?,evidence?,confidence}
//                                             -> runs guardrails::validate_new_finding immediately; returns accepted|rejected+reason so Claude can fix
//   review_submit {summary, verdict: "approve"|"comment"|"request_changes"} -> ends the run
// Tool names use underscores (Claude sees mcp__warden__mr_get_diff etc.)

// engine  [engine]
pub struct ReviewEngine { settings: AppSettings, events: EventBus }
impl ReviewEngine {
  pub async fn status(&self) -> EngineStatus;  // `claude --version`; login check via `claude -p "reply OK" --max-turns 1 --output-format json` ONLY when explicitly asked (status_deep); cheap status just finds binary + env check
  pub async fn status_deep(&self) -> EngineStatus;
  pub async fn review(&self, req: ReviewRequest) -> Result<EngineOutcome>; }
pub struct ReviewRequest { run_id: i64, worktree: PathBuf, mcp_url: String, run_token: String, system_prompt: String, user_prompt: String, max_turns: u32, timeout: Duration, model: String }
pub struct EngineOutcome { turns: u32, duration_ms: u64, session_id: Option<String>, submitted: bool, summary: Option<String>, verdict: Option<String>, raw_result: Option<String>, is_error: bool, error: Option<String> }
// Spawn (cwd = worktree, env stripped per hard rule 2):
//   claude -p <user_prompt> --append-system-prompt <system_prompt> --output-format stream-json --verbose
//     --mcp-config <tmp json: {"mcpServers":{"warden":{"type":"http","url":mcp_url,"headers":{"X-Warden-Run":token}}}}>
//     --strict-mcp-config --allowedTools mcp__warden__* Read Grep Glob
//     --disallowedTools Bash Edit Write MultiEdit NotebookEdit WebFetch WebSearch Task
//     --permission-mode default --model <model> --max-turns <n>
// Parse stream-json lines (stream.rs): system/init (session_id, tools), assistant (tool_use names -> RunProgress), result (num_turns, duration_ms, is_error, result, total_cost_usd ignored).
// Kill process group on timeout/cancel. Findings arrive via MCP tool calls, not stdout.
pub fn build_system_prompt(cfg: &WardenConfig) -> String;    // prompt.rs: role, untrusted-content rule, focus, Claude rules with ids, finding schema, severity guide, "only comment on changed lines", "cite rule_id or evidence", "max N findings", "call review_submit at the end"
pub fn build_user_prompt(mr: &MergeRequest, files: &[FileDiff], checks: &CheckReport, ci: &[CiJob], incremental_since: Option<&str>) -> String;

// forge  [forge]
#[async_trait] pub trait ForgeProvider: Send + Sync {
  async fn whoami(&self) -> Result<String>;
  async fn list_projects(&self, search: &str) -> Result<Vec<(String /*remote_id*/, String /*name*/)>>;
  async fn list_open_mrs(&self, project: &Project) -> Result<Vec<MergeRequest>>;   // fills shas from diff_refs
  async fn get_mr(&self, project: &Project, iid: u64) -> Result<MergeRequest>;
  async fn list_discussions(&self, project: &Project, iid: u64) -> Result<Vec<ForgeDiscussion>>;
  async fn post_line_note(&self, project: &Project, iid: u64, body: &str, pos: &DiffPosition) -> Result<String /*note/discussion id*/>;
  async fn post_note(&self, project: &Project, iid: u64, body: &str) -> Result<String>;
  async fn resolve_discussion(&self, project: &Project, iid: u64, discussion_id: &str) -> Result<()>;
  async fn add_label(&self, project: &Project, iid: u64, label: &str) -> Result<()>; }
pub struct ForgeDiscussion { id: String, resolved: bool, notes: Vec<ForgeNote> }  pub struct ForgeNote { id: String, author: String, body: String, path: Option<String>, line: Option<u32> }
pub fn provider_for(project: &Project, token: String) -> Arc<dyn ForgeProvider>;
// GitLab: base_url like https://gitlab.com ; header PRIVATE-TOKEN ; endpoints /api/v4/projects/:id/merge_requests?state=opened,
//   /merge_requests/:iid (diff_refs), /merge_requests/:iid/discussions (paginate via x-next-page), POST discussions with position[position_type]=text,
//   PUT discussions/:id?resolved=true, PUT merge_requests/:iid add_labels. project id = URL-encoded path.
// secrets.rs: pub fn set_token(service_key: &str, token: &str) -> Result<()>; get_token; delete_token  (keyring service "dev.warden.app")
// poller.rs: pub struct Poller; impl Poller { pub async fn poll_project(&self, db, provider, project) -> Result<Vec<MrChange>> }
//   MrChange { mr: MergeRequest, reason: New | NewCommits{since: String} | Closed } ; skip draft and label "no-ai-review" (reason Skipped recorded by pipeline)

// poster.rs  [poster]
pub struct Poster { .. }
impl Poster {
  pub async fn post_run(&self, db: &Db, provider: &dyn ForgeProvider, project: &Project, mr: &MergeRequest, run: &ReviewRun, only: Option<&[i64]>) -> Result<PostReport>;
     // posts Approved (or auto_post) findings: line note with position; on 400 position error -> fallback file-level note "**path:line**"; then summary note (if enabled) with findings table + CI table; label "ai-reviewed"; updates finding status/forge_note_id; audit events; respects max_notes_per_hour and posting_paused
  pub async fn auto_resolve(&self, db: &Db, provider: &dyn ForgeProvider, project: &Project, mr: &MergeRequest, fixed_fingerprints: &[String]) -> Result<u32>;
  pub fn format_note(f: &Finding) -> String;     // severity badge, title, body, ```suggestion block, footer "— Warden · rule FR-002 · confidence 0.98"
  pub fn format_summary(run: &ReviewRun, findings: &[Finding], ci: &[CiJob]) -> String; }
pub struct PostReport { posted: u32, fallback: u32, failed: u32, skipped: u32 }

// db.rs  [poster]  (SQLite at <data_dir>/warden.db, WAL; migrations embedded via sqlx::migrate!("./migrations"))
pub struct Db { pool: SqlitePool }
impl Db { pub async fn open(path: &Path) -> Result<Db>; pub async fn open_memory() -> Result<Db>;
  // settings
  get_settings() -> AppSettings; save_settings(&AppSettings); get_project_overrides(project_id) -> serde_json::Value; save_project_overrides
  // projects
  list_projects; get_project(id); upsert_project(&Project) -> i64; delete_project(id)
  // mrs
  upsert_mr(&MergeRequest) -> i64; list_mrs(project_id: Option<i64>) -> Vec<MergeRequest>; get_mr(id); set_last_reviewed(mr_id, sha)
  // runs
  create_run(mr_id, base, head, since: Option<&str>, model) -> ReviewRun; update_run(&ReviewRun); get_run(id); list_runs(mr_id: Option<i64>, limit) ; latest_run_for_mr(mr_id)
  runs_started_since(ts: &str) -> u32
  // findings
  insert_finding(run_id, &NewFinding, status, reason: Option<&str>, fingerprint, position: Option<&DiffPosition>) -> Finding
  list_findings(run_id); update_finding_status(id, status, forge_note_id: Option<&str>); update_finding_text(id, title, body, suggestion: Option<&str>)
  fingerprints_for_mr(mr_id) -> HashSet<String>; posted_findings_for_mr(mr_id) -> Vec<Finding>
  // ci
  insert_ci_job(&CiJob) -> i64; update_ci_job(&CiJob); list_ci_jobs(run_id)
  // audit
  audit(run_id: Option<i64>, kind, payload: serde_json::Value); list_audit(run_id, limit)
  posts_since(ts) -> u32 }
```

## Pipeline (`pipeline.rs`, integration) — one review run

1. Usage gate: if `posting_paused` only blocks posting; if `runs_started_since(now-1h) >= max_reviews_per_hour` → keep Queued, retry later.
2. Preparing: `WorktreeManager::prepare`, load `.warden.yml` from worktree + overrides, `diff(base, head)` (or `since..head` for incremental), parse, filter ignored, redact.
3. Checking: `run_checks` and `CiRunner::run_all` concurrently. Check findings go through guardrails and are stored with source "rule"; failed CI jobs give the engine context.
4. Reviewing: register RunContext in RunRegistry, build prompts, `ReviewEngine::review`. Findings streamed through `findings_tx` → guardrails → `insert_finding` (Draft, or Rejected with reason).
5. Filtering: `apply_budget`, compute auto_post per mode (Draft: none, Assist: per posting.auto_post, Auto: all kept).
6. AwaitingApproval (if anything not auto) and/or Posting (auto items) → `Poster::post_run`. Status Posted when nothing left in Draft.
7. Incremental re-review: fingerprints previously posted but whose anchor line is no longer changed and not re-found → `auto_resolve`.
8. Every step emits `AppEvent`s and audit events; errors → RunStatus::Failed with message; worktree kept for inspection.
Also: `review_local(repo_path, base_ref, head_ref)` — runs steps 2–5 on a local repo with no forge (used for smoke tests and "Review local branch" in UI).

## Tauri commands (`src-tauri/src/commands.rs`, integration; ui/src/lib/api.ts must mirror these names and arg shapes exactly)

```
get_engine_status() -> EngineStatus            check_engine_deep() -> EngineStatus
detect_tools() -> Vec<ToolStatus>               get_usage() -> UsageStats
get_settings() -> AppSettings                   save_settings(settings: AppSettings)
set_forge_token(forge: ForgeKind, base_url: String, token: String) -> String /*whoami*/
forge_status() -> Vec<{forge, base_url, user: Option<String>, ok: bool}>
search_remote_projects(forge: ForgeKind, base_url: String, query: String) -> Vec<(String,String)>
list_projects() -> Vec<Project>                 save_project(project: Project) -> i64     delete_project(id: i64)
get_project_config(id: i64) -> serde_json::Value /* merged WardenConfig */   save_project_overrides(id: i64, overrides: serde_json::Value)
poll_now() -> ()                                list_mrs(project_id: Option<i64>) -> Vec<MrRow>   // MrRow = MergeRequest + project_name + latest_run: Option<ReviewRun> + finding_counts {draft,posted,rejected} + ci_summary {passed,failed,running,total}
start_review(mr_id: i64) -> i64 /*run id*/      review_local(repo_path: String, base_ref: String, head_ref: String) -> i64
cancel_run(run_id: i64)                         get_run(run_id: i64) -> RunDetail  // run + mr + findings + ci_jobs + diff files (Vec<FileDiff>)
list_runs(mr_id: Option<i64>, limit: u32) -> Vec<ReviewRun>
update_finding(id: i64, title: String, body: String, suggestion: Option<String>)
set_finding_status(id: i64, status: FindingStatus)   // approve / dismiss
post_findings(run_id: i64, finding_ids: Vec<i64>) -> PostReport
rerun_ci_job(run_id: i64, name: String) -> CiJob   read_ci_log(job_id: i64) -> String
list_audit(run_id: Option<i64>, limit: u32) -> Vec<AuditEvent>
set_posting_paused(paused: bool)
```
Events: backend emits each `AppEvent` on Tauri event channel `"warden://event"`.
Background: on startup open DB, start McpServer on settings.mcp_port (fallback to random free port), start poller loop per enabled project, tray icon (menu: Open, Poll now, Pause posting, Quit), macOS notification when a run reaches AwaitingApproval or fails.

## UI (ui/, owned by ui agent)

Vite + React 18 + TypeScript + Tailwind 3 + shadcn/ui (copy components into `ui/src/components/ui/`), TanStack Query,
simple state routing (no router needed), `@git-diff-view/react` or a hand-rolled diff table (hand-rolled is fine and lighter),
`lucide-react` icons. Fonts: IBM Plex Sans / IBM Plex Mono (bundle via @fontsource). Light + dark (follow system).
Screens exactly per `docs/plan.html`: Inbox, Review (file tree / diff with inline finding cards / findings panel / footer with mode + post button),
Local CI (jobs list + live log + "Claude explanation" placeholder card), Guardrails (posting mode, filters, agent limits, rule list),
Connections (engine status + deep check button, forge tokens, watched projects table + add project dialog), plus History (runs + audit) and Approval queue (all Draft findings across runs).
`api.ts` detects Tauri via `window.__TAURI_INTERNALS__`; otherwise uses `mock.ts` with realistic sample data (same data as the mockups) so the UI runs in a plain browser with `pnpm --dir ui dev` (port 1420).

## Dev commands

```
pnpm install                          # root (installs @tauri-apps/cli) ; pnpm --dir ui install
cargo test -p warden-core
pnpm --dir ui dev                     # browser UI with mock backend on :1420
pnpm tauri dev                        # full app
pnpm tauri build --debug              # .app bundle
```
