/**
 * Typed wrappers around the Tauri commands in SPEC "Tauri commands" (src-tauri/src/commands.rs).
 *
 * One function per command, same snake_case name. ARGUMENT KEYS ARE camelCase: Tauri 2 `invoke`
 * converts JS camelCase keys to the Rust snake_case parameter names, so the Rust command
 * `start_review(mr_id: i64)` is called as `invoke("start_review", { mrId })`. Every wrapper below
 * follows that convention; the mock backend (mock.ts) receives exactly the same argument objects.
 *
 * When the page is not running inside Tauri (no `window.__TAURI_INTERNALS__`, e.g. `pnpm --dir ui dev`
 * in a plain browser), every call is routed to the in-memory mock backend in `mock.ts`.
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { AddProjectItem } from "@/bindings/AddProjectItem";
import type { AddProjectResult } from "@/bindings/AddProjectResult";
import type { AppEvent } from "@/bindings/AppEvent";
import type { Discovery } from "@/bindings/Discovery";
import type { RuleConfig } from "@/bindings/RuleConfig";
import type { RuleTestResult } from "@/bindings/RuleTestResult";
import type { OpenMrResult } from "@/bindings/OpenMrResult";
import type { PendingFinding } from "@/bindings/PendingFinding";
import type { RuleStat } from "@/bindings/RuleStat";
import type { RunRow } from "@/bindings/RunRow";
import type { SimulatedFinding } from "@/bindings/SimulatedFinding";
import type { Stats } from "@/bindings/Stats";
import type { PostingMode } from "@/bindings/PostingMode";
import type { AppSettings } from "@/bindings/AppSettings";
import type { AuditEvent } from "@/bindings/AuditEvent";
import type { CiJob } from "@/bindings/CiJob";
import type { EngineStatus } from "@/bindings/EngineStatus";
import type { FindingStatus } from "@/bindings/FindingStatus";
import type { ForgeKind } from "@/bindings/ForgeKind";
import type { ForgeStatus } from "@/bindings/ForgeStatus";
import type { MrRow } from "@/bindings/MrRow";
import type { PostReport } from "@/bindings/PostReport";
import type { Project } from "@/bindings/Project";
import type { ReviewRun } from "@/bindings/ReviewRun";
import type { RunDetail } from "@/bindings/RunDetail";
import type { ToolStatus } from "@/bindings/ToolStatus";
import type { UsageStats } from "@/bindings/UsageStats";
import type { WardenConfig } from "@/bindings/WardenConfig";

import { mockBackend } from "./mock";

export const EVENT_CHANNEL = "warden://event";

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && window.__TAURI_INTERNALS__ !== undefined;
}

async function call<T>(cmd: string, args: Record<string, unknown> = {}): Promise<T> {
  if (isTauri()) return tauriInvoke<T>(cmd, args);
  return mockBackend.invoke(cmd, args) as Promise<T>;
}

/** Subscribe to backend `AppEvent`s. Returns an unsubscribe function. */
export function onEvent(cb: (event: AppEvent) => void): () => void {
  if (!isTauri()) return mockBackend.subscribe(cb);
  let disposed = false;
  let unlisten: (() => void) | undefined;
  listen<AppEvent>(EVENT_CHANNEL, (e) => cb(e.payload))
    .then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    })
    .catch((err) => console.error("warden: failed to subscribe to events", err));
  return () => {
    disposed = true;
    unlisten?.();
  };
}

/**
 * Per-project local overrides as written by the Guardrails screen. A deep-partial WardenConfig
 * (objects merged over the repo `.warden.yml`) plus `disabled_rules`, a list of rule ids to skip.
 */
export type ProjectOverrides = {
  review?: Partial<WardenConfig["review"]>;
  posting?: Partial<WardenConfig["posting"]>;
  disabled_rules?: string[];
  [key: string]: unknown;
};

/** `get_project_config` returns the merged config; the backend may echo `disabled_rules` alongside. */
export type MergedConfig = WardenConfig & { disabled_rules?: string[] };

export type RemoteProject = [remoteId: string, name: string];

// ---- engine / tools / usage ------------------------------------------------------------------
export const get_engine_status = () => call<EngineStatus>("get_engine_status");
export const check_engine_deep = () => call<EngineStatus>("check_engine_deep");
export const detect_tools = () => call<ToolStatus[]>("detect_tools");
export const get_usage = () => call<UsageStats>("get_usage");

// ---- settings --------------------------------------------------------------------------------
export const get_settings = () => call<AppSettings>("get_settings");
export const save_settings = (settings: AppSettings) => call<void>("save_settings", { settings });

// ---- forges ----------------------------------------------------------------------------------
/** Stores the token in the keychain (backend) and returns the forge username (whoami). */
export const set_forge_token = (forge: ForgeKind, baseUrl: string, token: string) =>
  call<string>("set_forge_token", { forge, baseUrl, token });
export const forge_status = () => call<ForgeStatus[]>("forge_status");
export const search_remote_projects = (forge: ForgeKind, baseUrl: string, query: string) =>
  call<RemoteProject[]>("search_remote_projects", { forge, baseUrl, query });

// ---- project discovery -----------------------------------------------------------------------
/** Remote projects you can access merged with git clones found under `roots` ([] = default folders). */
export const discover_projects = (forge: ForgeKind, baseUrl: string, roots: string[]) =>
  call<Discovery>("discover_projects", { forge, baseUrl, roots });
/** Adds several projects; items without `local_path` get a Warden-managed clone. */
export const add_projects = (
  forge: ForgeKind,
  baseUrl: string,
  items: AddProjectItem[],
  mode: PostingMode,
  pollIntervalSecs: number,
) => call<AddProjectResult[]>("add_projects", { forge, baseUrl, items, mode, pollIntervalSecs });
/** Pasted MR/PR link: adds the project if needed and queues a review. */
export const open_mr_link = (url: string) => call<OpenMrResult>("open_mr_link", { url });
/** Native folder picker; null when cancelled. */
export const pick_folder = () => call<string | null>("pick_folder");

// ---- projects --------------------------------------------------------------------------------
export const list_projects = () => call<Project[]>("list_projects");
/** `project.id = 0` creates a new project. Returns the id. */
export const save_project = (project: Project) => call<number>("save_project", { project });
export const delete_project = (id: number) => call<void>("delete_project", { id });
export const get_project_config = (id: number) => call<MergedConfig>("get_project_config", { id });
/** Lines of pasted code a pattern rule matches (rule editor test box). */
export const test_rule_pattern = (pattern: string, code: string) => call<RuleTestResult>("test_rule_pattern", { pattern, code });
/** What an unsaved rule would flag in a past run's diff (read-only). */
export const test_rule_on_run = (rule: RuleConfig, runId: number) => call<RuleTestResult>("test_rule_on_run", { rule, runId });
/** Stored local overrides only (not merged with `.warden.yml`). */
export const get_project_overrides = (id: number) => call<ProjectOverrides>("get_project_overrides", { id });
export const save_project_overrides = (id: number, overrides: ProjectOverrides) =>
  call<void>("save_project_overrides", { id, overrides });

// ---- MRs and runs ----------------------------------------------------------------------------
export const poll_now = () => call<void>("poll_now");
export const list_mrs = (projectId: number | null) => call<MrRow[]>("list_mrs", { projectId });
export const start_review = (mrId: number) => call<number>("start_review", { mrId });
export const review_local = (repoPath: string, baseRef: string, headRef: string) =>
  call<number>("review_local", { repoPath, baseRef, headRef });
export const cancel_run = (runId: number) => call<void>("cancel_run", { runId });
export const get_run = (runId: number) => call<RunDetail>("get_run", { runId });
export const list_runs = (mrId: number | null, limit: number) => call<ReviewRun[]>("list_runs", { mrId, limit });
/** History rows: every run (merged/closed MRs and local reviews too) with MR, project and counts. */
export const list_run_rows = (limit: number) => call<RunRow[]>("list_run_rows", { limit });

// ---- findings --------------------------------------------------------------------------------
export const update_finding = (id: number, title: string, body: string, suggestion: string | null) =>
  call<void>("update_finding", { id, title, body, suggestion });
export const set_finding_status = (id: number, status: FindingStatus) =>
  call<void>("set_finding_status", { id, status });
export const post_findings = (runId: number, findingIds: number[]) =>
  call<PostReport>("post_findings", { runId, findingIds });

// ---- CI --------------------------------------------------------------------------------------
export const rerun_ci_job = (runId: number, name: string) => call<CiJob>("rerun_ci_job", { runId, name });
export const read_ci_log = (jobId: number) => call<string>("read_ci_log", { jobId });

// ---- audit / posting -------------------------------------------------------------------------
export const list_audit = (runId: number | null, limit: number) => call<AuditEvent[]>("list_audit", { runId, limit });
export const set_posting_paused = (paused: boolean) => call<void>("set_posting_paused", { paused });

// ---- stats / rules / guardrail preview -------------------------------------------------------
/** Totals + one row per day for the last `days` days (zero days included). */
export const get_stats = (days: number) => call<Stats>("get_stats", { days });
/** Per-rule hit/post/dismiss/reject counts; `projectId = null` → all projects. */
export const rule_stats = (projectId: number | null, days: number) => call<RuleStat[]>("rule_stats", { projectId, days });
/** Re-classify the project's recent findings under candidate overrides (nothing is saved). */
export const simulate_guardrails = (projectId: number, overrides: ProjectOverrides, limitRuns: number) =>
  call<SimulatedFinding[]>("simulate_guardrails", { projectId, overrides, limitRuns });

// ---- approval queue --------------------------------------------------------------------------
/** The note markdown exactly as Warden would post it. */
export const preview_note = (findingId: number) => call<string>("preview_note", { findingId });
/** Draft/Approved findings from the latest run of every open MR. */
export const list_pending_findings = () => call<PendingFinding[]>("list_pending_findings");
/** Posts findings from several runs; one report per run. */
export const post_findings_bulk = (findingIds: number[]) => call<PostReport[]>("post_findings_bulk", { findingIds });

// ---- windows ---------------------------------------------------------------------------------
/** Tauri: open (or focus) the Settings window on `pane`. No-op in the browser mock. */
export const open_settings_window = (pane: string | null) => call<void>("open_settings_window", { pane });

export const api = {
  get_engine_status,
  check_engine_deep,
  detect_tools,
  get_usage,
  get_settings,
  save_settings,
  set_forge_token,
  forge_status,
  search_remote_projects,
  discover_projects,
  add_projects,
  open_mr_link,
  pick_folder,
  list_projects,
  save_project,
  delete_project,
  get_project_config,
  get_project_overrides,
  test_rule_pattern,
  test_rule_on_run,
  save_project_overrides,
  poll_now,
  list_mrs,
  start_review,
  review_local,
  cancel_run,
  get_run,
  list_runs,
  list_run_rows,
  update_finding,
  set_finding_status,
  post_findings,
  rerun_ci_job,
  read_ci_log,
  list_audit,
  set_posting_paused,
  get_stats,
  rule_stats,
  simulate_guardrails,
  preview_note,
  list_pending_findings,
  post_findings_bulk,
  open_settings_window,
};
