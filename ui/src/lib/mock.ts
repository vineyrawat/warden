/**
 * In-memory mock backend used when the UI runs outside Tauri (`pnpm --dir ui dev`).
 *
 * `invoke(cmd, args)` accepts the same command names and camelCase argument objects as api.ts sends
 * to Tauri. Mutations change the in-memory state and emit the same `AppEvent`s the Rust backend would.
 * Long-running work (the !29 review advancing turns, CI logs streaming) is modelled as generators that
 * advance one step per `step()`; in the browser a timer calls `step()` once the UI subscribes to events.
 * Tests create their own backend with `createMockBackend({ autoTick: false, latencyMs: 0 })`.
 */
import type { AddProjectItem } from "@/bindings/AddProjectItem";
import type { AddProjectResult } from "@/bindings/AddProjectResult";
import type { AppEvent } from "@/bindings/AppEvent";
import type { Discovery } from "@/bindings/Discovery";
import type { LocalRepo } from "@/bindings/LocalRepo";
import type { PostingMode } from "@/bindings/PostingMode";
import type { AppSettings } from "@/bindings/AppSettings";
import type { AuditEvent } from "@/bindings/AuditEvent";
import type { CiJob } from "@/bindings/CiJob";
import type { CiStatus } from "@/bindings/CiStatus";
import type { Finding } from "@/bindings/Finding";
import type { FindingStatus } from "@/bindings/FindingStatus";
import type { ForgeKind } from "@/bindings/ForgeKind";
import type { MergeRequest } from "@/bindings/MergeRequest";
import type { MrRow } from "@/bindings/MrRow";
import type { PostReport } from "@/bindings/PostReport";
import type { Project } from "@/bindings/Project";
import type { ReviewRun } from "@/bindings/ReviewRun";
import type { RunDetail } from "@/bindings/RunDetail";
import type { RunStatus } from "@/bindings/RunStatus";
import type { WardenConfig } from "@/bindings/WardenConfig";
import type { Severity } from "@/bindings/Severity";
import type { Stats } from "@/bindings/Stats";
import type { RuleStat } from "@/bindings/RuleStat";
import type { SimulatedFinding } from "@/bindings/SimulatedFinding";
import type { PendingFinding } from "@/bindings/PendingFinding";
import type { RunRow } from "@/bindings/RunRow";

import { LOG_GITLAB_BUILD, LOG_UNIT_FAIL, createSeed, fakeFingerprint, makeFile, makeHunk, type Seed } from "./mock-data";

type Listener = (e: AppEvent) => void;
type Sim = Generator<void, void, void>;

export type MockOptions = { autoTick?: boolean; tickMs?: number; latencyMs?: number; now?: number };

const clone = <T>(v: T): T => (v === undefined ? v : (JSON.parse(JSON.stringify(v)) as T));

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** JSON deep merge (objects merge recursively, everything else replaces) — mirrors merge_overrides. */
export function deepMerge<T>(base: T, patch: unknown): T {
  if (!isPlainObject(base) || !isPlainObject(patch)) return (patch === undefined ? base : (patch as T));
  const out: Record<string, unknown> = { ...base };
  for (const [k, v] of Object.entries(patch)) out[k] = k in out ? deepMerge(out[k], v) : v;
  return out as T;
}

export class MockBackend {
  private s: Seed;
  private listeners = new Set<Listener>();
  private sims: Sim[] = [];
  private timer: ReturnType<typeof setInterval> | null = null;
  private nextId = { run: 200, job: 700, finding: 3000, audit: 20000, project: 10, mr: 100 };
  private ciReruns = new Map<string, number>();
  readonly opts: Required<MockOptions>;

  constructor(opts: MockOptions = {}) {
    this.opts = { autoTick: true, tickMs: 1200, latencyMs: 80, now: Date.now(), ...opts };
    this.s = createSeed(this.opts.now);
    // Built-in simulations: the !29 review keeps going, !28's gitlab-ci:build streams its log.
    this.sims.push(this.simReview(102, { startTurn: 9, findingsAt: 14, submitAt: 19 }));
    this.sims.push(this.simCiJob(513, unitLog29(), "passed", 2));
    this.sims.push(this.simCiJob(525, LOG_GITLAB_BUILD.slice(4), "passed", 1));
  }

  // ---- plumbing ------------------------------------------------------------------------------

  subscribe(cb: Listener): () => void {
    this.listeners.add(cb);
    if (this.opts.autoTick && !this.timer) this.timer = setInterval(() => this.step(), this.opts.tickMs);
    return () => {
      this.listeners.delete(cb);
      if (this.listeners.size === 0 && this.timer) {
        clearInterval(this.timer);
        this.timer = null;
      }
    };
  }

  /** Advance every running simulation by one step. Returns the number still running. */
  step(): number {
    this.sims = this.sims.filter((g) => !g.next().done);
    return this.sims.length;
  }

  /** Run simulations until they finish (tests). */
  runToCompletion(maxSteps = 500): void {
    for (let i = 0; i < maxSteps && this.step() > 0; i++);
  }

  get activeSimulations(): number {
    return this.sims.length;
  }

  private emit(e: AppEvent) {
    for (const l of [...this.listeners]) {
      try {
        l(clone(e));
      } catch (err) {
        console.error(err);
      }
    }
  }

  async invoke(cmd: string, args: Record<string, unknown> = {}): Promise<unknown> {
    if (this.opts.latencyMs > 0) await new Promise((r) => setTimeout(r, this.opts.latencyMs));
    const handler = (this as unknown as Record<string, unknown>)[`cmd_${cmd}`];
    if (typeof handler !== "function") throw new Error(`mock backend: unknown command "${cmd}"`);
    return clone(await (handler as (a: Record<string, unknown>) => unknown).call(this, args));
  }

  private nowIso() {
    return new Date().toISOString();
  }

  private audit(run_id: number | null, kind: string, payload: unknown) {
    const ev: AuditEvent = { id: ++this.nextId.audit, run_id, kind, payload, created_at: this.nowIso() };
    this.s.audit.push(ev);
  }

  private run(id: number): ReviewRun {
    const r = this.s.runs.find((x) => x.id === id);
    if (!r) throw new Error(`run ${id} not found`);
    return r;
  }

  private mr(id: number): MergeRequest {
    const m = this.s.mrs.find((x) => x.id === id);
    if (!m) throw new Error(`merge request ${id} not found`);
    return m;
  }

  private finding(id: number): Finding {
    const f = this.s.findings.find((x) => x.id === id);
    if (!f) throw new Error(`finding ${id} not found`);
    return f;
  }

  private setRunStatus(run: ReviewRun, status: RunStatus) {
    run.status = status;
    if (["awaiting_approval", "posted", "failed", "skipped", "cancelled"].includes(status) && !run.ended_at) {
      run.ended_at = this.nowIso();
      run.duration_ms = Math.max(run.duration_ms, Date.parse(run.ended_at) - Date.parse(run.started_at));
    }
    this.emit({ type: "run_updated", run: clone(run) });
    this.emit({ type: "mrs_updated", project_id: this.mr(run.mr_id).project_id });
  }

  private projectForRun(run: ReviewRun): Project | undefined {
    const mr = this.mr(run.mr_id);
    return this.s.projects.find((p) => p.id === mr.project_id);
  }

  // ---- simulations ---------------------------------------------------------------------------

  private *simReview(runId: number, o: { startTurn: number; findingsAt: number; submitAt: number }): Sim {
    const run = this.run(runId);
    const tools = [
      "mr_get_diff · entries_crm/ai/audit.py",
      "repo_read_file · entries_crm/ai/agent.py",
      "repo_search · record_tool_call",
      "repo_read_file · entries_crm/ai/doctype/ecrm_ai_tool_call/ecrm_ai_tool_call.json",
      "ci_get_results",
      "repo_search · ignore_permissions=True",
      "review_add_finding · entries_crm/ai/audit.py:13",
      "repo_read_file · entries_crm/hooks.py",
      "review_add_finding · entries_crm/ai/agent.py:66",
      "mr_get_overview",
    ];
    let turn = o.startTurn;
    while (turn < o.submitAt) {
      if (isCancelled(run)) return;
      turn++;
      run.turns = turn;
      const msg = tools[turn % tools.length];
      this.emit({ type: "run_progress", run_id: runId, message: msg, turn });
      if (turn % 3 === 0) this.audit(runId, "tool_call", { tool: msg.split(" · ")[0], turn });
      if (turn === o.findingsAt) this.addSimFindings(runId);
      yield;
    }
    if (isCancelled(run)) return;
    run.summary =
      "Audit trail records every tool call, but inserts bypass permissions without a comment and the agent records full argument payloads, which can include customer PII.";
    run.verdict = "comment";
    this.audit(runId, "engine", { event: "result", turns: run.turns, verdict: run.verdict });
    this.setRunStatus(run, "filtering");
    yield;
    this.setRunStatus(run, "awaiting_approval");
    this.emit({ type: "notice", level: "info", message: `!${this.mr(run.mr_id).iid} is ready for approval` });
  }

  private addSimFindings(runId: number) {
    const run = this.run(runId);
    const mk = (f: Omit<Finding, "id" | "run_id" | "fingerprint" | "position" | "forge_note_id">): Finding => ({
      ...f,
      id: ++this.nextId.finding,
      run_id: runId,
      forge_note_id: null,
      fingerprint: fakeFingerprint(f.file, f.title, f.rule_id),
      position: { base_sha: run.base_sha, start_sha: run.base_sha, head_sha: run.head_sha, old_path: f.file, new_path: f.file, old_line: null, new_line: f.line },
    });
    this.s.findings.push(
      mk({ file: "entries_crm/ai/audit.py", line: 13, side: "new", severity: "major", category: "frappe-permissions", rule_id: "FR-004", title: "ignore_permissions=True without a reason", body: "The audit insert bypasses permissions with no comment saying why. Add a comment (audit rows are system-owned) or insert as the session user.", suggestion: "    # audit rows are system-owned; users never write them directly\n    }).insert(ignore_permissions=True)", evidence: null, confidence: 0.86, source: "claude", status: "draft", guardrail_reason: null }),
      mk({ file: "entries_crm/ai/agent.py", line: 66, side: "new", severity: "minor", category: "privacy", rule_id: null, title: "Full tool arguments stored in the audit log", body: "args can contain customer names, emails and free text from the chat. Store argument keys and sizes, or redact values, so the audit table does not become a second copy of customer data.", suggestion: null, evidence: "record_tool_call(self.session, name, args, ...) passes the raw dict", confidence: 0.77, source: "claude", status: "draft", guardrail_reason: null }),
      mk({ file: "entries_crm/ai/audit.py", line: 2, side: "new", severity: "nit", category: "style", rule_id: null, title: "Unused import style", body: "now_datetime is fine, but the module elsewhere uses frappe.utils.now(); pick one for consistency.", suggestion: null, evidence: null, confidence: 0.55, source: "claude", status: "rejected", guardrail_reason: "nits off" }),
    );
    this.emit({ type: "findings_updated", run_id: runId });
    this.emit({ type: "mrs_updated", project_id: this.mr(run.mr_id).project_id });
  }

  private *simCiJob(jobId: number, lines: string[], final: CiStatus, linesPerTick: number): Sim {
    const job = this.s.ciJobs.find((j) => j.id === jobId);
    if (!job) return;
    const started = Date.now();
    job.status = "running";
    this.emit({ type: "ci_job_updated", job: clone(job) });
    for (let i = 0; i < lines.length; i += linesPerTick) {
      if (job.status !== "running") return;
      for (const line of lines.slice(i, i + linesPerTick)) {
        this.s.logs[jobId] = (this.s.logs[jobId] ? `${this.s.logs[jobId]}\n` : "") + line;
        this.emit({ type: "ci_log", run_id: job.run_id, job_id: jobId, line });
      }
      yield;
    }
    job.status = final;
    job.duration_ms = Math.max(Date.now() - started, 1000) + (final === "failed" ? 240_000 : 60_000);
    if (final === "failed") job.failure_excerpt = lines.filter((l) => /FAIL|Error|File "/.test(l)).join("\n") || lines.slice(-5).join("\n");
    this.emit({ type: "ci_job_updated", job: clone(job) });
    this.emit({ type: "mrs_updated", project_id: this.mr(this.run(job.run_id).mr_id).project_id });
  }

  /** A fresh review started from the UI: queued → preparing → checking → reviewing → awaiting approval. */
  private *simNewRun(runId: number): Sim {
    const run = this.run(runId);
    const stages: RunStatus[] = ["preparing", "checking"];
    for (const st of stages) {
      if (isCancelled(run)) return;
      this.setRunStatus(run, st);
      this.emit({ type: "run_progress", run_id: runId, message: st === "preparing" ? "git worktree add --detach" : "running deterministic checks", turn: 0 });
      yield;
    }
    const lint: CiJob = { id: ++this.nextId.job, run_id: runId, name: "lint", kind: "recipe", command: "ruff check {changed_py}", status: "running", duration_ms: 0, log_path: `~/.warden/logs/${runId}/lint.log`, failure_excerpt: null };
    this.s.ciJobs.push(lint);
    this.sims.push(this.simCiJob(lint.id, ["$ ruff check {changed_py}", "All checks passed!"], "passed", 1));
    if (isCancelled(run)) return;
    this.setRunStatus(run, "reviewing");
    for (let turn = 1; turn <= 6; turn++) {
      if (isCancelled(run)) return;
      run.turns = turn;
      this.emit({ type: "run_progress", run_id: runId, message: ["mr_get_overview", "mr_get_diff", "repo_read_file", "repo_search", "ci_get_results", "review_submit"][turn - 1], turn });
      yield;
    }
    run.summary = "No blocking issues found in the reviewed range.";
    run.verdict = "approve";
    this.setRunStatus(run, "filtering");
    yield;
    this.setRunStatus(run, "posted");
  }

  private *simRerun(job: CiJob, fail: boolean): Sim {
    const lines =
      job.name === "unit" && fail
        ? LOG_UNIT_FAIL
        : [`$ ${job.command}`, "Running checks…", job.kind === "gitlab_ci_local" ? "PASS build" : "All checks passed!"];
    yield* this.simCiJob(job.id, lines, fail ? "failed" : "passed", 2);
    if (fail) this.emit({ type: "notice", level: "warn", message: `${job.name} failed again on re-run` });
  }

  // ---- commands: engine / tools / usage ------------------------------------------------------

  cmd_get_engine_status() {
    return { ...this.s.engine, cli_path: this.s.settings.claude_cli_path ?? this.s.engine.cli_path };
  }

  async cmd_check_engine_deep() {
    if (this.opts.latencyMs > 0) await new Promise((r) => setTimeout(r, 900));
    this.audit(null, "engine", { event: "deep_check", ok: true });
    return { ...this.cmd_get_engine_status(), message: "Connected · subscription login confirmed · 8 Warden tools visible" };
  }

  cmd_detect_tools() {
    return this.s.tools;
  }

  cmd_get_usage() {
    const hourAgo = Date.now() - 3600_000;
    const dayStart = new Date();
    dayStart.setHours(0, 0, 0, 0);
    const started = this.s.runs.filter((r) => r.status !== "skipped").map((r) => Date.parse(r.started_at));
    return {
      reviews_last_hour: started.filter((t) => t >= hourAgo).length,
      reviews_today: Math.max(started.filter((t) => t >= dayStart.getTime()).length, started.filter((t) => t >= hourAgo).length),
      max_reviews_per_hour: this.s.settings.max_reviews_per_hour,
      paused: this.s.settings.posting_paused,
    };
  }

  // ---- settings ------------------------------------------------------------------------------

  cmd_get_settings() {
    return this.s.settings;
  }

  cmd_save_settings({ settings }: { settings: AppSettings }) {
    if (!settings || typeof settings.max_turns !== "number") throw new Error("invalid settings");
    this.s.settings = { ...settings };
  }

  cmd_set_posting_paused({ paused }: { paused: boolean }) {
    this.s.settings.posting_paused = !!paused;
    this.audit(null, "post", { posting_paused: !!paused });
    this.emit({ type: "notice", level: paused ? "warn" : "info", message: paused ? "Posting paused for all projects" : "Posting resumed" });
  }

  // ---- forges --------------------------------------------------------------------------------

  cmd_set_forge_token({ forge, baseUrl, token }: { forge: ForgeKind; baseUrl: string; token: string }) {
    if (!token || token.trim().length < 8) throw new Error("Token rejected by forge (401 Unauthorized)");
    const user = forge === "gitlab" ? "warden-bot" : "warden-bot-gh";
    const base = (baseUrl || (forge === "gitlab" ? "https://gitlab.com" : "https://api.github.com")).replace(/\/+$/, "");
    this.s.forges = this.s.forges.filter((f) => !(f.forge === forge && f.base_url === base));
    this.s.forges.push({ forge, base_url: base, user, ok: true });
    // The token itself is discarded: the real backend stores it in the keychain, never in the UI.
    return user;
  }

  cmd_forge_status() {
    return this.s.forges;
  }

  cmd_search_remote_projects({ forge, query }: { forge: ForgeKind; baseUrl: string; query: string }) {
    if (!this.s.forges.some((f) => f.forge === forge && f.ok)) throw new Error(`No ${forge} token configured`);
    const q = (query ?? "").toLowerCase();
    return this.s.remoteProjects.filter(([id, name]) => id.toLowerCase().includes(q) || name.toLowerCase().includes(q));
  }

  // ---- project discovery ---------------------------------------------------------------------

  cmd_discover_projects({ forge, baseUrl, roots }: { forge: ForgeKind; baseUrl: string; roots: string[] }): Discovery {
    if (!this.s.forges.some((f) => f.forge === forge && f.ok)) throw new Error(`No ${forge} token saved for ${baseUrl}`);
    const root = roots?.[0] || "~/Desktop";
    const bench = root.includes("benches") ? root : `${root}/Entries/benches`;
    const repo = (path: string, url: string, remote_id: string | null, duplicate_of: string | null = null): LocalRepo => ({
      path,
      remote_url: url,
      remote_id,
      duplicate_of,
    });
    const local: LocalRepo[] = [
      repo(`${bench}/crmv15/apps/entries_crm`, "git@gitlab.com:entries/entries_crm.git", "entries/entries_crm"),
      repo(`${bench}/crmv15/apps/entries_platform`, "git@gitlab.com:entries/entries_platform.git", "entries/entries_platform"),
      repo(`${bench}/crmv15/apps/entries_lnd`, "git@gitlab.com:entries/entries_lnd.git", "entries/entries_lnd"),
      repo(`${bench}/crmv15/apps/entries_desk`, "git@gitlab.com:entries/entries_desk.git", "entries/entries_desk"),
      repo(`${bench}/version16/apps/entries_crm`, "git@gitlab.com:entries/entries_crm.git", "entries/entries_crm", `${bench}/crmv15/apps/entries_crm`),
      repo(`${bench}/crmv15/apps/frappe`, "https://github.com/frappe/frappe.git", null),
    ];
    const branches: Record<string, string> = { "entries/entries_crm": "develop", "entries/entries_lnd": "develop" };
    const projects = this.s.remoteProjects.map(([remote_id, name], i) => ({
      remote: {
        remote_id,
        name,
        default_branch: branches[remote_id] ?? "main",
        last_activity_at: new Date(Date.now() - (i * 7 + 2) * 3600_000).toISOString(),
        web_url: `https://gitlab.com/${remote_id}`,
        clone_url: `https://gitlab.com/${remote_id}.git`,
      },
      local_paths: local.filter((l) => l.remote_id === remote_id).map((l) => l.path),
      existing_project_id: this.s.projects.find((p) => p.remote_id === remote_id)?.id ?? null,
    }));
    projects.sort((a, b) => Number(a.local_paths.length === 0) - Number(b.local_paths.length === 0));
    return { projects, local_repos: local, scanned_roots: roots?.length ? roots : ["~/Desktop", "~/code"], scanned_dirs: 4127 };
  }

  cmd_add_projects({
    forge,
    baseUrl,
    items,
    mode,
    pollIntervalSecs,
  }: {
    forge: ForgeKind;
    baseUrl: string;
    items: AddProjectItem[];
    mode: PostingMode;
    pollIntervalSecs: number;
  }): AddProjectResult[] {
    return items.map((it) => {
      const existing = this.s.projects.find((p) => p.remote_id === it.remote_id);
      if (existing) return { remote_id: it.remote_id, project_id: existing.id, error: null };
      const id = ++this.nextId.project;
      const name = it.name || it.remote_id.split("/").pop() || it.remote_id;
      this.s.projects.push({
        id,
        forge,
        base_url: baseUrl,
        remote_id: it.remote_id,
        name,
        local_path: it.local_path ?? `~/Library/Application Support/Warden/repos/${name}`,
        target_branches: it.target_branches ?? ["main"],
        mode,
        poll_interval_secs: pollIntervalSecs,
        enabled: true,
      });
      this.s.configs[id] = createSeed().configs[2];
      this.emit({ type: "mrs_updated", project_id: id });
      return { remote_id: it.remote_id, project_id: id, error: null };
    });
  }

  cmd_open_mr_link({ url }: { url: string }) {
    const m = /^https?:\/\/[^/]+\/(.+?)\/-\/merge_requests\/(\d+)/.exec(url.trim());
    if (!m) throw new Error("Not a merge request or pull request link");
    const [, remoteId, iid] = m;
    let project = this.s.projects.find((p) => p.remote_id === remoteId);
    let added = false;
    if (!project) {
      const [res] = this.cmd_add_projects({ forge: "gitlab", baseUrl: "https://gitlab.com", items: [{ remote_id: remoteId, name: "", local_path: null, target_branches: null }], mode: "draft", pollIntervalSecs: 300 });
      project = this.s.projects.find((p) => p.id === res.project_id)!;
      added = true;
    }
    const mr = this.s.mrs.find((x) => x.project_id === project!.id && x.iid === Number(iid));
    if (!mr) throw new Error(`!${iid} not found in ${remoteId}`);
    const runId = this.cmd_start_review({ mrId: mr.id }) as number;
    return { project_id: project.id, mr_id: mr.id, run_id: runId, added_project: added };
  }

  cmd_pick_folder() {
    return "~/Desktop/Entries/benches";
  }

  // ---- projects ------------------------------------------------------------------------------

  cmd_list_projects() {
    return this.s.projects;
  }

  cmd_save_project({ project }: { project: Project }) {
    if (!project.remote_id) throw new Error("remote_id is required");
    if (!project.local_path) throw new Error("local_path is required");
    if (project.id && this.s.projects.some((p) => p.id === project.id)) {
      this.s.projects = this.s.projects.map((p) => (p.id === project.id ? { ...project } : p));
      return project.id;
    }
    const id = ++this.nextId.project;
    const name = project.name || project.remote_id.split("/").pop() || project.remote_id;
    this.s.projects.push({ ...project, id, name });
    this.s.configs[id] = createSeed().configs[2];
    this.emit({ type: "mrs_updated", project_id: id });
    return id;
  }

  cmd_delete_project({ id }: { id: number }) {
    this.s.projects = this.s.projects.filter((p) => p.id !== id);
    this.emit({ type: "mrs_updated", project_id: id });
  }

  cmd_get_project_config({ id }: { id: number }) {
    const base = this.s.configs[id];
    if (!base) throw new Error(`project ${id} not found`);
    const ov = this.s.overrides[id] ?? {};
    const { disabled_rules, ...rest } = ov as { disabled_rules?: string[] };
    const merged = deepMerge<WardenConfig>(clone(base), rest);
    return { ...merged, disabled_rules: disabled_rules ?? [] };
  }

  cmd_get_project_overrides({ id }: { id: number }) {
    if (!this.s.configs[id]) throw new Error(`project ${id} not found`);
    return clone(this.s.overrides[id] ?? {});
  }

  cmd_save_project_overrides({ id, overrides }: { id: number; overrides: Record<string, unknown> }) {
    if (!this.s.configs[id]) throw new Error(`project ${id} not found`);
    if (!isPlainObject(overrides)) throw new Error("overrides must be an object");
    this.s.overrides[id] = clone(overrides);
  }

  // ---- MRs and runs --------------------------------------------------------------------------

  cmd_poll_now() {
    for (const p of this.s.projects.filter((p) => p.enabled)) this.emit({ type: "mrs_updated", project_id: p.id });
    this.emit({ type: "notice", level: "info", message: `Polled ${this.s.projects.filter((p) => p.enabled).length} projects · no new MRs` });
  }

  private latestRun(mrId: number): ReviewRun | null {
    const runs = this.s.runs.filter((r) => r.mr_id === mrId);
    if (!runs.length) return null;
    return runs.reduce((a, b) => (b.id > a.id ? b : a));
  }

  cmd_list_mrs({ projectId }: { projectId: number | null }): MrRow[] {
    return this.s.mrs
      .filter((m) => m.state === "opened" && (projectId == null || m.project_id === projectId))
      .filter((m) => m.project_id === 0 || this.s.projects.some((p) => p.id === m.project_id))
      .map((m) => {
        const run = this.latestRun(m.id);
        const fs = run ? this.s.findings.filter((f) => f.run_id === run.id) : [];
        const jobs = run ? this.s.ciJobs.filter((j) => j.run_id === run.id) : [];
        return {
          ...m,
          project_name: this.s.projects.find((p) => p.id === m.project_id)?.name ?? "local",
          latest_run: run,
          finding_counts: {
            draft: fs.filter((f) => f.status === "draft" || f.status === "approved").length,
            posted: fs.filter((f) => f.status === "posted" || f.status === "resolved").length,
            rejected: fs.filter((f) => f.status === "rejected").length,
          },
          ci_summary: {
            passed: jobs.filter((j) => j.status === "passed").length,
            failed: jobs.filter((j) => j.status === "failed" || j.status === "timed_out").length,
            running: jobs.filter((j) => j.status === "running" || j.status === "pending").length,
            total: jobs.length,
          },
        };
      })
      .sort((a, b) => b.iid - a.iid);
  }

  private createRun(mr: MergeRequest, since: string | null): ReviewRun {
    const run: ReviewRun = {
      id: ++this.nextId.run,
      mr_id: mr.id,
      base_sha: mr.base_sha,
      head_sha: mr.head_sha,
      since_sha: since,
      status: "queued",
      model: this.s.settings.model,
      turns: 0,
      duration_ms: 0,
      cli_session_id: null,
      summary: null,
      verdict: null,
      error: null,
      started_at: this.nowIso(),
      ended_at: null,
    };
    this.s.runs.push(run);
    const prev = this.s.files[this.latestRun(mr.id)?.id ?? -1];
    this.s.files[run.id] = prev ? clone(prev) : [];
    this.audit(run.id, "engine", { event: "queued", model: run.model });
    return run;
  }

  cmd_start_review({ mrId }: { mrId: number }) {
    const mr = this.mr(mrId);
    const usage = this.cmd_get_usage();
    const run = this.createRun(mr, mr.last_reviewed_sha && mr.last_reviewed_sha !== mr.head_sha ? mr.last_reviewed_sha : null);
    this.emit({ type: "run_updated", run: clone(run) });
    this.emit({ type: "mrs_updated", project_id: mr.project_id });
    if (usage.reviews_last_hour >= usage.max_reviews_per_hour) {
      this.emit({ type: "notice", level: "warn", message: `Hourly review limit reached (${usage.max_reviews_per_hour}); !${mr.iid} stays queued` });
      return run.id;
    }
    this.sims.push(this.simNewRun(run.id));
    return run.id;
  }

  cmd_review_local({ repoPath, baseRef, headRef }: { repoPath: string; baseRef: string; headRef: string }) {
    if (!repoPath || !baseRef || !headRef) throw new Error("repo path, base ref and head ref are required");
    const mr: MergeRequest = {
      id: ++this.nextId.mr,
      project_id: 0,
      iid: 0,
      title: `Local: ${baseRef}…${headRef}`,
      description: `Local review of ${repoPath}`,
      author: "you",
      source_branch: headRef,
      target_branch: baseRef,
      web_url: "",
      draft: false,
      labels: [],
      head_sha: "0000000000000000000000000000000000000000",
      base_sha: "0000000000000000000000000000000000000000",
      start_sha: "0000000000000000000000000000000000000000",
      last_reviewed_sha: null,
      additions: 7,
      deletions: 1,
      state: "opened",
      updated_at: this.nowIso(),
    };
    this.s.mrs.push(mr);
    const run = this.createRun(mr, null);
    this.s.files[run.id] = [
      makeFile("app/hooks.py", [makeHunk(1, 1, "", [" app_name = \"demo\"", "-app_version = \"0.1.0\"", "+app_version = \"0.2.0\"", "+doc_events = {", "+    \"ToDo\": {\"on_update\": \"demo.events.todo_updated\"},", "+}"])]),
    ];
    this.emit({ type: "run_updated", run: clone(run) });
    this.sims.push(this.simNewRun(run.id));
    return run.id;
  }

  cmd_cancel_run({ runId }: { runId: number }) {
    const run = this.run(runId);
    if (["posted", "failed", "skipped", "cancelled"].includes(run.status)) return;
    for (const j of this.s.ciJobs.filter((j) => j.run_id === runId && j.status === "running")) {
      j.status = "cancelled";
      this.emit({ type: "ci_job_updated", job: clone(j) });
    }
    this.audit(runId, "engine", { event: "cancelled" });
    this.setRunStatus(run, "cancelled");
  }

  cmd_get_run({ runId }: { runId: number }): RunDetail {
    const run = this.run(runId);
    return {
      run,
      mr: this.mr(run.mr_id),
      findings: this.s.findings.filter((f) => f.run_id === runId),
      ci_jobs: this.s.ciJobs.filter((j) => j.run_id === runId),
      files: this.s.files[runId] ?? [],
    };
  }

  cmd_list_runs({ mrId, limit }: { mrId: number | null; limit: number }) {
    return this.s.runs
      .filter((r) => mrId == null || r.mr_id === mrId)
      .sort((a, b) => Date.parse(b.started_at) - Date.parse(a.started_at) || b.id - a.id)
      .slice(0, limit || 50);
  }

  /** Mirrors `Db::list_run_rows`: every run with its MR, project and per-run counts. */
  cmd_list_run_rows({ limit }: { limit: number }): RunRow[] {
    return this.cmd_list_runs({ mrId: null, limit: limit || 500 }).map((run) => {
      const mr = this.mr(run.mr_id);
      const fs = this.s.findings.filter((f) => f.run_id === run.id);
      const jobs = this.s.ciJobs.filter((j) => j.run_id === run.id);
      return {
        run,
        mr_iid: mr.iid,
        mr_title: mr.title,
        author: mr.author,
        web_url: mr.web_url,
        draft: mr.draft,
        project_id: mr.project_id,
        project_name: this.s.projects.find((p) => p.id === mr.project_id)?.name ?? "local",
        finding_counts: {
          draft: fs.filter((f) => f.status === "draft" || f.status === "approved").length,
          posted: fs.filter((f) => f.status === "posted" || f.status === "resolved").length,
          rejected: fs.filter((f) => f.status === "rejected").length,
        },
        ci_summary: {
          passed: jobs.filter((j) => j.status === "passed").length,
          failed: jobs.filter((j) => j.status === "failed" || j.status === "timed_out").length,
          running: jobs.filter((j) => j.status === "running" || j.status === "pending").length,
          total: jobs.length,
        },
      };
    });
  }

  // ---- findings ------------------------------------------------------------------------------

  cmd_update_finding({ id, title, body, suggestion }: { id: number; title: string; body: string; suggestion: string | null }) {
    if (!title?.trim() || !body?.trim()) throw new Error("title and body are required");
    const f = this.finding(id);
    f.title = title.trim();
    f.body = body.trim();
    f.suggestion = suggestion && suggestion.trim() ? suggestion : null;
    f.fingerprint = fakeFingerprint(f.file, f.title, f.rule_id);
    this.audit(f.run_id, "guardrail", { finding_id: id, edited: true });
    this.emit({ type: "findings_updated", run_id: f.run_id });
  }

  cmd_set_finding_status({ id, status }: { id: number; status: FindingStatus }) {
    const f = this.finding(id);
    if (f.status === "posted" || f.status === "resolved") throw new Error("finding is already posted");
    f.status = status;
    this.audit(f.run_id, "guardrail", { finding_id: id, status, by: "user" });
    this.emit({ type: "findings_updated", run_id: f.run_id });
    this.emit({ type: "mrs_updated", project_id: this.mr(this.run(f.run_id).mr_id).project_id });
  }

  cmd_post_findings({ runId, findingIds }: { runId: number; findingIds: number[] }): PostReport {
    const run = this.run(runId);
    if (this.s.settings.posting_paused) throw new Error("Posting is paused. Resume posting in Guardrails first.");
    const report: PostReport = { posted: 0, fallback: 0, failed: 0, skipped: 0 };
    for (const id of findingIds) {
      const f = this.s.findings.find((x) => x.id === id && x.run_id === runId);
      if (!f || f.status === "posted" || f.status === "resolved" || f.status === "dismissed") {
        report.skipped++;
        continue;
      }
      f.status = "posted";
      f.forge_note_id = `disc-${(++this.nextId.audit).toString(16)}`;
      // Same semantics as Rust PostReport: `posted` counts every note, `fallback` is the subset
      // posted as a file-level note.
      report.posted++;
      if (!f.position) report.fallback++;
      this.audit(runId, "post", { finding_id: id, note: f.forge_note_id, kind: f.position ? "line" : "file" });
    }
    if (report.posted > 0) {
      this.audit(runId, "post", { summary: true, note: `note-${this.nextId.audit}` });
      const mr = this.mr(run.mr_id);
      if (!mr.labels.includes("ai-reviewed")) mr.labels.push("ai-reviewed");
      mr.last_reviewed_sha = run.head_sha;
    }
    const left = this.s.findings.some((f) => f.run_id === runId && (f.status === "draft" || f.status === "approved"));
    this.emit({ type: "findings_updated", run_id: runId });
    if (!left && run.status === "awaiting_approval") this.setRunStatus(run, "posted");
    else this.emit({ type: "mrs_updated", project_id: this.mr(run.mr_id).project_id });
    return report;
  }

  // ---- CI ------------------------------------------------------------------------------------

  cmd_rerun_ci_job({ runId, name }: { runId: number; name: string }): CiJob {
    const job = this.s.ciJobs.find((j) => j.run_id === runId && j.name === name);
    if (!job) throw new Error(`no CI job "${name}" in run ${runId}`);
    if (job.status === "running") throw new Error(`${name} is already running`);
    const key = `${runId}:${name}`;
    this.ciReruns.set(key, (this.ciReruns.get(key) ?? 0) + 1);
    job.status = "running";
    job.duration_ms = 0;
    job.failure_excerpt = null;
    this.s.logs[job.id] = "";
    this.audit(runId, "tool_call", { tool: "ci_run_recipe", name, by: "user" });
    // The SLA bug is real, so !28's unit job keeps failing on re-run.
    const fail = name === "unit" && runId === 103;
    this.sims.push(this.simRerun(job, fail));
    this.emit({ type: "ci_job_updated", job: clone(job) });
    return job;
  }

  cmd_read_ci_log({ jobId }: { jobId: number }) {
    if (!this.s.ciJobs.some((j) => j.id === jobId)) throw new Error(`CI job ${jobId} not found`);
    return this.s.logs[jobId] ?? "";
  }

  // ---- stats / rule catalogue / guardrail preview ---------------------------------------------

  /** Mirrors warden-core `stats::get_stats`: UTC days, notes counted on their run's start day. */
  cmd_get_stats({ days }: { days: number }): Stats {
    const n = Math.max(1, Math.min(366, Math.floor(days || 14)));
    const dayKey = (t: number) => new Date(t).toISOString().slice(0, 10);
    const todayUtc = Date.UTC(new Date(this.opts.now).getUTCFullYear(), new Date(this.opts.now).getUTCMonth(), new Date(this.opts.now).getUTCDate());
    const start = todayUtc - (n - 1) * 86_400_000;
    const runs = this.s.runs.filter((r) => Date.parse(r.started_at) >= start && r.status !== "skipped");
    const runIds = new Set(runs.map((r) => r.id));
    const fs = this.s.findings.filter((f) => runIds.has(f.run_id));
    const posted = fs.filter((f) => f.status === "posted" || f.status === "resolved");
    const resolved = fs.filter((f) => f.status === "resolved").length;
    const durations = runs.filter((r) => (r.status === "posted" || r.status === "awaiting_approval") && r.ended_at && r.duration_ms > 0).map((r) => r.duration_ms).sort((a, b) => a - b);
    const median =
      durations.length === 0
        ? 0
        : durations.length % 2
          ? durations[(durations.length - 1) / 2]
          : Math.round((durations[durations.length / 2 - 1] + durations[durations.length / 2]) / 2);
    const per_day = Array.from({ length: n }, (_, i) => ({ date: dayKey(start + i * 86_400_000), reviews: 0, notes: 0 }));
    const byDate = new Map(per_day.map((d) => [d.date, d]));
    const runDate = new Map(runs.map((r) => [r.id, dayKey(Date.parse(r.started_at))]));
    for (const r of runs) {
      const d = byDate.get(runDate.get(r.id)!);
      if (d) d.reviews++;
    }
    for (const f of posted) {
      const d = byDate.get(runDate.get(f.run_id)!);
      if (d) d.notes++;
    }
    return {
      reviews: runs.length,
      notes_posted: posted.length,
      resolved,
      resolved_rate: posted.length ? resolved / posted.length : 0,
      median_run_ms: median,
      per_day,
    };
  }

  cmd_rule_stats({ projectId, days }: { projectId: number | null; days: number }): RuleStat[] {
    const since = this.opts.now - Math.max(1, days || 30) * 86_400_000;
    const runById = new Map(this.s.runs.map((r) => [r.id, r]));
    const rows = new Map<string, RuleStat & { _last: number }>();
    for (const f of this.s.findings) {
      const run = runById.get(f.run_id);
      if (!run || Date.parse(run.started_at) < since) continue;
      if (projectId != null && this.mr(run.mr_id).project_id !== projectId) continue;
      const key = f.rule_id ?? f.category;
      const t = Date.parse(run.started_at);
      let row = rows.get(key);
      if (!row) {
        row = { rule_id: key, title: f.title, hits: 0, posted: 0, dismissed: 0, rejected: 0, last_hit_at: null, _last: 0 };
        rows.set(key, row);
      }
      row.hits++;
      if (f.status === "posted" || f.status === "resolved") row.posted++;
      if (f.status === "dismissed") row.dismissed++;
      if (f.status === "rejected") row.rejected++;
      if (t >= row._last) {
        row._last = t;
        row.last_hit_at = run.started_at;
        row.title = f.title;
      }
    }
    // Prefer the configured rule title when the rule exists in a project config.
    const configs = projectId != null ? [this.s.configs[projectId]].filter(Boolean) : Object.values(this.s.configs);
    for (const row of rows.values()) {
      const rule = configs.flatMap((c) => c.rules).find((r) => r.id === row.rule_id);
      if (rule) row.title = rule.title;
    }
    return [...rows.values()]
      .sort((a, b) => b.hits - a.hits || a.rule_id.localeCompare(b.rule_id))
      .map(({ _last: _ignored, ...r }) => r);
  }

  /** Mirrors warden-core `stats::simulate_guardrails`. */
  cmd_simulate_guardrails({ projectId, overrides, limitRuns }: { projectId: number; overrides: Record<string, unknown>; limitRuns: number }): SimulatedFinding[] {
    const project = this.s.projects.find((p) => p.id === projectId);
    if (!project || !this.s.configs[projectId]) throw new Error(`project ${projectId} not found`);
    const stored = (this.s.overrides[projectId] ?? {}) as Record<string, unknown>;
    const cand = (isPlainObject(overrides) ? overrides : {}) as Record<string, unknown>;
    const { disabled_rules: storedDisabled, ...storedRest } = stored as { disabled_rules?: string[] };
    const { disabled_rules: candDisabled, ...candRest } = cand as { disabled_rules?: string[] };
    const cfg = deepMerge<WardenConfig>(deepMerge<WardenConfig>(clone(this.s.configs[projectId]), storedRest), candRest);
    const mode = (((candRest as Record<string, unknown>).posting as { mode?: PostingMode } | undefined)?.mode ?? project.mode) as PostingMode;
    const disabled = new Set(candDisabled ?? storedDisabled ?? []);
    const rank: Record<Severity, number> = { blocker: 3, major: 2, minor: 1, nit: 0 };
    const policy = (reason: string | null) => !reason || /nits|below the floor|below the threshold|over note budget|disabled/.test(reason);

    const mrIds = new Set(this.s.mrs.filter((m) => m.project_id === projectId).map((m) => m.id));
    const runs = this.s.runs
      .filter((r) => mrIds.has(r.mr_id) && this.s.findings.some((f) => f.run_id === r.id))
      .sort((a, b) => Date.parse(b.started_at) - Date.parse(a.started_at) || b.id - a.id)
      .slice(0, Math.max(1, limitRuns || 10));
    const out: SimulatedFinding[] = [];
    for (const run of runs) {
      const mr = this.mr(run.mr_id);
      const label = `!${mr.iid} ${mr.title}`;
      const fs = this.s.findings.filter((f) => f.run_id === run.id);
      const kept: { f: Finding; i: number }[] = [];
      const rows: SimulatedFinding[] = fs.map((f, i) => {
        const row = (outcome: "post" | "draft" | "drop", reason: string): SimulatedFinding => ({ finding: f, mr_label: label, outcome, reason });
        if (f.rule_id && disabled.has(f.rule_id)) return row("drop", `rule ${f.rule_id} is disabled`);
        if (f.severity === "nit" && !cfg.review.nits) return row("drop", "nits are disabled");
        if (rank[f.severity] < rank[cfg.review.severity_floor]) return row("drop", `${f.severity} is below the floor (${cfg.review.severity_floor})`);
        if (f.source !== "rule" && f.confidence < cfg.review.min_confidence)
          return row("drop", `confidence ${f.confidence.toFixed(2)} is below ${cfg.review.min_confidence.toFixed(2)}`);
        if (f.status === "rejected" && !policy(f.guardrail_reason)) return row("drop", f.guardrail_reason ?? "rejected by the guardrails");
        kept.push({ f, i });
        return row("draft", "");
      });
      // Note budget: keep the top max_notes by severity, then confidence.
      const ranked = [...kept].sort((a, b) => rank[b.f.severity] - rank[a.f.severity] || b.f.confidence - a.f.confidence || a.i - b.i);
      ranked.forEach(({ f, i }, n) => {
        if (n >= cfg.review.max_notes) {
          rows[i] = { ...rows[i], outcome: "drop", reason: `over the note budget (${cfg.review.max_notes})` };
          return;
        }
        const ap = cfg.posting.auto_post;
        const auto = mode === "auto" || (mode === "assist" && ap.severities.includes(f.severity) && f.confidence >= ap.min_confidence);
        rows[i] = auto
          ? { ...rows[i], outcome: "post", reason: mode === "auto" ? "auto mode posts everything that passes" : `auto-posts: ${f.severity}, confidence ${f.confidence.toFixed(2)} ≥ ${ap.min_confidence.toFixed(2)}` }
          : { ...rows[i], outcome: "draft", reason: mode === "draft" ? "draft mode: waits for approval" : "waits for approval (below the auto-post bar)" };
      });
      out.push(...rows);
    }
    return out;
  }

  // ---- approval queue ------------------------------------------------------------------------

  /** Mirrors `Poster::format_note` / `format_fallback_note`. */
  cmd_preview_note({ findingId }: { findingId: number }): string {
    const f = this.finding(findingId);
    const badge: Record<Severity, string> = { blocker: "🔴 Blocker", major: "🟠 Major", minor: "🟡 Minor", nit: "⚪ Nit" };
    let out = `${badge[f.severity]} · **${f.title.split(/\s+/).filter(Boolean).join(" ")}**\n\n${f.body.trim()}\n`;
    const sug = f.suggestion?.replace(/[\r\n]+$/, "");
    if (sug && sug.trim()) {
      const longest = Math.max(0, ...(sug.match(/`+/g) ?? []).map((m) => m.length));
      const fence = "`".repeat(Math.max(3, longest + 1));
      out += !sug.includes("\n") && f.side !== "old" ? `\n${fence}suggestion:-0+0\n${sug}\n${fence}\n` : `\n${fence}\n${sug}\n${fence}\n`;
    }
    const footer = ["— Warden"];
    if (f.rule_id?.trim()) footer.push(`rule ${f.rule_id.trim()}`);
    footer.push(`confidence ${f.confidence.toFixed(2)}`);
    out += `\n${footer.join(" · ")}`;
    if (!f.position) return `**${f.file}:${f.line}**\n\n${out.replace("suggestion:-0+0\n", "\n")}`;
    return out;
  }

  cmd_list_pending_findings(): PendingFinding[] {
    const out: PendingFinding[] = [];
    const mrs = this.s.mrs
      .filter((m) => m.state === "opened" && this.s.projects.some((p) => p.id === m.project_id))
      .sort((a, b) => b.iid - a.iid);
    for (const m of mrs) {
      const run = this.latestRun(m.id);
      if (!run) continue;
      const project = this.s.projects.find((p) => p.id === m.project_id)!;
      for (const f of this.s.findings.filter((x) => x.run_id === run.id && (x.status === "draft" || x.status === "approved"))) {
        out.push({ finding: f, run_id: run.id, mr_id: m.id, mr_iid: m.iid, mr_title: m.title, project_name: project.name, web_url: m.web_url });
      }
    }
    return out;
  }

  cmd_post_findings_bulk({ findingIds }: { findingIds: number[] }): PostReport[] {
    const byRun = new Map<number, number[]>();
    for (const id of findingIds ?? []) {
      const f = this.finding(id);
      byRun.set(f.run_id, [...(byRun.get(f.run_id) ?? []), id]);
    }
    return [...byRun.entries()].sort((a, b) => a[0] - b[0]).map(([runId, ids]) => this.cmd_post_findings({ runId, findingIds: ids }));
  }

  // ---- windows -------------------------------------------------------------------------------

  /** The browser shell renders Settings in-app, so there is no window to open. */
  cmd_open_settings_window(_args: { pane: string | null }) {
    return null;
  }

  // ---- audit ---------------------------------------------------------------------------------

  cmd_list_audit({ runId, limit }: { runId: number | null; limit: number }) {
    return this.s.audit
      .filter((a) => runId == null || a.run_id === runId)
      .sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at) || b.id - a.id)
      .slice(0, limit || 100);
  }
}

function unitLog29(): string[] {
  return [
    "test_records_tool_call ... ok",
    "test_records_result_size ... ok",
    "test_no_record_when_disabled ... ok",
    "test_session_link ... ok",
    "test_args_serialised ... ok",
    "test_insert_as_guest ... ok",
    "Ran 6 tests in 41.7s · OK",
  ];
}

export function createMockBackend(opts?: MockOptions): MockBackend {
  return new MockBackend(opts);
}

/** Shared instance used by api.ts in the browser. */
export const mockBackend = createMockBackend();

function isCancelled(run: ReviewRun): boolean {
  return run.status === "cancelled";
}
