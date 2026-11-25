import { beforeEach, describe, expect, it } from "vitest";
import { MockBackend, createMockBackend, deepMerge } from "../mock";
import type { AppEvent } from "@/bindings/AppEvent";
import type { MrRow } from "@/bindings/MrRow";
import type { RunDetail } from "@/bindings/RunDetail";
import type { CiJob } from "@/bindings/CiJob";

let be: MockBackend;
let events: AppEvent[];

const call = <T>(cmd: string, args: Record<string, unknown> = {}) => be.invoke(cmd, args) as Promise<T>;

beforeEach(() => {
  be = createMockBackend({ autoTick: false, latencyMs: 0 });
  events = [];
  be.subscribe((e) => events.push(e));
});

describe("mock backend: sample data", () => {
  it("lists the five mockup MRs in the inbox order", async () => {
    const rows = await call<MrRow[]>("list_mrs", { projectId: null });
    expect(rows.map((r) => r.iid)).toEqual([31, 29, 28, 27, 26]);
    const mr31 = rows[0];
    expect(mr31.project_name).toBe("entries_crm");
    expect(mr31.latest_run?.status).toBe("awaiting_approval");
    expect(mr31.finding_counts).toEqual({ draft: 4, posted: 0, rejected: 2 });
    expect(mr31.ci_summary).toEqual({ passed: 3, failed: 0, running: 0, total: 3 });
    expect(rows[2].ci_summary.failed).toBe(1);
    expect(rows[4].draft).toBe(true);
    expect(rows[4].latest_run?.status).toBe("skipped");
  });

  it("filters MRs by project", async () => {
    const rows = await call<MrRow[]>("list_mrs", { projectId: 3 });
    expect(rows.map((r) => r.iid)).toEqual([28]);
  });

  it("returns run detail with diff files and the f-string SQL finding", async () => {
    const d = await call<RunDetail>("get_run", { runId: 101 });
    expect(d.mr.iid).toBe(31);
    expect(d.files.map((f) => f.new_path)).toContain("entries_crm/api/import_links.py");
    const sql = d.findings.find((f) => f.rule_id === "FR-002")!;
    expect(sql.severity).toBe("blocker");
    expect(sql.line).toBe(16);
    const line16 = d.files[0].hunks[0].lines.find((l) => l.new_line === 16)!;
    expect(line16.kind).toBe("add");
    expect(line16.text).toContain('frappe.db.sql(f"update');
  });

  it("reports the API key as absent and optional tools as not installed", async () => {
    const st = await call<{ api_key_in_env: boolean; ok: boolean }>("get_engine_status");
    expect(st.api_key_in_env).toBe(false);
    const tools = await call<{ name: string; installed: boolean }[]>("detect_tools");
    expect(tools.find((t) => t.name === "semgrep")?.installed).toBe(false);
    expect(tools.find((t) => t.name === "git")?.installed).toBe(true);
  });

  it("rejects unknown commands", async () => {
    await expect(call("drop_database")).rejects.toThrow(/unknown command/);
  });
});

describe("mock backend: mutations", () => {
  it("approves, edits and dismisses findings", async () => {
    await call("set_finding_status", { id: 1002, status: "approved" });
    await call("update_finding", { id: 1004, title: "Selection lost on page change", body: "Keep selection keyed by row name across pages.", suggestion: null });
    await call("set_finding_status", { id: 1003, status: "dismissed" });
    const d = await call<RunDetail>("get_run", { runId: 101 });
    expect(d.findings.find((f) => f.id === 1002)?.status).toBe("approved");
    expect(d.findings.find((f) => f.id === 1004)?.title).toBe("Selection lost on page change");
    expect(d.findings.find((f) => f.id === 1003)?.status).toBe("dismissed");
    expect(events.filter((e) => e.type === "findings_updated").length).toBe(3);
  });

  it("posts findings and moves the run to Posted when nothing is left", async () => {
    await call("set_finding_status", { id: 1003, status: "dismissed" });
    const report = await call<{ posted: number; skipped: number }>("post_findings", { runId: 101, findingIds: [1001, 1002, 1004, 1003] });
    expect(report.posted).toBe(3);
    expect(report.skipped).toBe(1);
    const d = await call<RunDetail>("get_run", { runId: 101 });
    expect(d.run.status).toBe("posted");
    expect(d.findings.filter((f) => f.status === "posted").every((f) => f.forge_note_id)).toBe(true);
    expect(events.some((e) => e.type === "run_updated" && e.run.status === "posted")).toBe(true);
    const audit = await call<{ kind: string }[]>("list_audit", { runId: 101, limit: 50 });
    expect(audit.filter((a) => a.kind === "post").length).toBeGreaterThanOrEqual(4);
  });

  it("refuses to post while posting is paused", async () => {
    await call("set_posting_paused", { paused: true });
    expect((await call<{ paused: boolean }>("get_usage")).paused).toBe(true);
    await expect(call("post_findings", { runId: 101, findingIds: [1001] })).rejects.toThrow(/paused/);
  });

  it("saves settings and project overrides (merged into get_project_config)", async () => {
    const s = await call<Record<string, unknown>>("get_settings");
    await call("save_settings", { settings: { ...s, max_turns: 40 } });
    expect((await call<{ max_turns: number }>("get_settings")).max_turns).toBe(40);

    await call("save_project_overrides", { id: 1, overrides: { review: { min_confidence: 0.8 }, disabled_rules: ["FR-004"] } });
    const cfg = await call<{ review: { min_confidence: number; max_notes: number }; disabled_rules: string[] }>("get_project_config", { id: 1 });
    expect(cfg.review.min_confidence).toBe(0.8);
    expect(cfg.review.max_notes).toBe(10);
    expect(cfg.disabled_rules).toEqual(["FR-004"]);
  });

  it("adds a project and never returns tokens", async () => {
    const who = await call<string>("set_forge_token", { forge: "github", baseUrl: "https://api.github.com", token: "ghp_test_token_123" });
    expect(who).toBe("warden-bot-gh");
    const forges = await call<unknown[]>("forge_status");
    expect(JSON.stringify(forges)).not.toContain("ghp_test_token_123");
    const found = await call<[string, string][]>("search_remote_projects", { forge: "gitlab", baseUrl: "https://gitlab.com", query: "lnd" });
    expect(found).toEqual([["entries/entries_lnd", "entries_lnd"]]);
    const id = await call<number>("save_project", {
      project: { id: 0, forge: "gitlab", base_url: "https://gitlab.com", remote_id: "entries/entries_lnd", name: "", local_path: "~/code/lnd", target_branches: ["develop"], mode: "draft", poll_interval_secs: 300, enabled: true },
    });
    const projects = await call<{ id: number; name: string }[]>("list_projects");
    expect(projects.find((p) => p.id === id)?.name).toBe("entries_lnd");
  });

  it("rejects a short forge token", async () => {
    await expect(call("set_forge_token", { forge: "gitlab", baseUrl: "https://gitlab.com", token: "x" })).rejects.toThrow(/401/);
  });
});

describe("mock backend: simulations", () => {
  it("advances the !29 review turn by turn until it awaits approval", async () => {
    be.step();
    const progress = events.filter((e) => e.type === "run_progress");
    expect(progress.length).toBe(1);
    expect(progress[0]).toMatchObject({ run_id: 102, turn: 10 });
    be.runToCompletion();
    const d = await call<RunDetail>("get_run", { runId: 102 });
    expect(d.run.status).toBe("awaiting_approval");
    expect(d.findings.length).toBe(3);
    expect(d.run.summary).toBeTruthy();
    expect(events.some((e) => e.type === "findings_updated" && e.run_id === 102)).toBe(true);
  });

  it("streams CI log lines and finishes the gitlab-ci:build job", async () => {
    be.runToCompletion();
    const lines = events.filter((e): e is Extract<AppEvent, { type: "ci_log" }> => e.type === "ci_log" && e.job_id === 525).map((e) => e.line);
    expect(lines.length).toBeGreaterThan(5);
    expect(lines[lines.length - 1]).toBe("PASS build");
    const log = await call<string>("read_ci_log", { jobId: 525 });
    expect(log.split("\n").pop()).toBe("PASS build");
    const d = await call<RunDetail>("get_run", { runId: 103 });
    expect(d.ci_jobs.find((j) => j.id === 525)?.status).toBe("passed");
  });

  it("re-runs the failing SLA unit job and it fails again", async () => {
    const job = await call<CiJob>("rerun_ci_job", { runId: 103, name: "unit" });
    expect(job.status).toBe("running");
    be.runToCompletion();
    const d = await call<RunDetail>("get_run", { runId: 103 });
    const unit = d.ci_jobs.find((j) => j.name === "unit")!;
    expect(unit.status).toBe("failed");
    expect(unit.failure_excerpt).toContain("AssertionError");
  });

  it("starts a review for the skipped draft MR and completes it", async () => {
    const runId = await call<number>("start_review", { mrId: 5 });
    be.runToCompletion();
    const d = await call<RunDetail>("get_run", { runId });
    expect(d.run.status).toBe("posted");
    const rows = await call<MrRow[]>("list_mrs", { projectId: null });
    expect(rows.find((r) => r.iid === 26)?.latest_run?.id).toBe(runId);
  });

  it("cancels a running review", async () => {
    await call("cancel_run", { runId: 102 });
    be.runToCompletion();
    const d = await call<RunDetail>("get_run", { runId: 102 });
    expect(d.run.status).toBe("cancelled");
  });
});

describe("deepMerge", () => {
  it("merges objects and replaces arrays/scalars", () => {
    expect(deepMerge({ a: { b: 1, c: 2 }, d: [1, 2] }, { a: { c: 3 }, d: [9] })).toEqual({ a: { b: 1, c: 3 }, d: [9] });
  });
});

describe("mock backend: add projects", () => {
  it("discovers projects with local clones first and flags watched ones", async () => {
    const d = await call<import("@/bindings/Discovery").Discovery>("discover_projects", { forge: "gitlab", baseUrl: "https://gitlab.com", roots: [] });
    expect(d.projects[0].local_paths.length).toBeGreaterThan(0);
    const crm = d.projects.find((p) => p.remote.remote_id === "entries/entries_crm")!;
    expect(crm.existing_project_id).toBe(1);
    expect(crm.local_paths).toHaveLength(2);
    expect(d.local_repos.some((r) => r.duplicate_of)).toBe(true);
  });

  it("adds several projects at once, managed clone when no path", async () => {
    const res = await call<import("@/bindings/AddProjectResult").AddProjectResult[]>("add_projects", {
      forge: "gitlab",
      baseUrl: "https://gitlab.com",
      items: [
        { remote_id: "entries/entries_ui", name: "entries_ui", local_path: null, target_branches: null },
        { remote_id: "entries/beacon", name: "beacon", local_path: "~/code/beacon", target_branches: ["main"] },
      ],
      mode: "draft",
      pollIntervalSecs: 300,
    });
    expect(res.every((r) => r.project_id != null)).toBe(true);
    const projects = await call<import("@/bindings/Project").Project[]>("list_projects");
    expect(projects.find((p) => p.remote_id === "entries/entries_ui")?.local_path).toContain("Warden/repos");
    expect(projects.find((p) => p.remote_id === "entries/beacon")?.local_path).toBe("~/code/beacon");
  });

  it("rejects a link that is not an MR", async () => {
    await expect(call("open_mr_link", { url: "https://gitlab.com/entries/entries_crm" })).rejects.toThrow(/merge request/);
  });
});

describe("mock backend: stats, rules, guardrail preview, queue", () => {
  type Stats = import("@/bindings/Stats").Stats;
  type RuleStat = import("@/bindings/RuleStat").RuleStat;
  type SimulatedFinding = import("@/bindings/SimulatedFinding").SimulatedFinding;
  type PendingFinding = import("@/bindings/PendingFinding").PendingFinding;
  type PostReport = import("@/bindings/PostReport").PostReport;

  it("get_stats returns one row per day with history spread over 14 days", async () => {
    const st = await call<Stats>("get_stats", { days: 14 });
    expect(st.per_day).toHaveLength(14);
    expect(st.per_day[13].date).toBe(new Date(be.opts.now).toISOString().slice(0, 10));
    expect(st.per_day.filter((d) => d.reviews > 0).length).toBeGreaterThanOrEqual(10);
    expect(st.per_day.some((d) => d.reviews === 0)).toBe(true);
    expect(st.reviews).toBe(st.per_day.reduce((a, d) => a + d.reviews, 0));
    expect(st.notes_posted).toBe(st.per_day.reduce((a, d) => a + d.notes, 0));
    expect(st.resolved).toBeGreaterThan(0);
    expect(st.resolved_rate).toBeCloseTo(st.resolved / st.notes_posted);
    expect(st.median_run_ms).toBeGreaterThan(60_000);
  });

  it("rule_stats shows FR-007 as noisy (dismissed about 55% of the time)", async () => {
    const rows = await call<RuleStat[]>("rule_stats", { projectId: null, days: 30 });
    const fr7 = rows.find((r) => r.rule_id === "FR-007")!;
    expect(fr7.title).toBe("No frappe.db.commit() in request handlers");
    expect(fr7.hits).toBeGreaterThanOrEqual(15);
    expect(fr7.dismissed / fr7.hits).toBeGreaterThan(0.45);
    expect(fr7.dismissed / fr7.hits).toBeLessThan(0.65);
    expect(fr7.last_hit_at).not.toBeNull();
    expect(rows.length).toBeGreaterThanOrEqual(6);
    for (const r of rows) expect(r.posted + r.dismissed + r.rejected).toBeLessThanOrEqual(r.hits);
    const desk = await call<RuleStat[]>("rule_stats", { projectId: 3, days: 30 });
    expect(desk.reduce((a, r) => a + r.hits, 0)).toBeLessThan(rows.reduce((a, r) => a + r.hits, 0));
  });

  it("simulate_guardrails drops a disabled rule and posts everything in auto mode", async () => {
    const base = await call<SimulatedFinding[]>("simulate_guardrails", { projectId: 1, overrides: {}, limitRuns: 10 });
    expect(base.length).toBeGreaterThan(0);
    expect(new Set(base.map((r) => r.outcome)).size).toBeGreaterThan(1);
    expect(base.every((r) => r.reason.length > 0 && r.mr_label.startsWith("!"))).toBe(true);

    const off = await call<SimulatedFinding[]>("simulate_guardrails", { projectId: 1, overrides: { disabled_rules: ["FR-007"] }, limitRuns: 10 });
    const fr7 = off.filter((r) => r.finding.rule_id === "FR-007");
    expect(fr7.length).toBeGreaterThan(0);
    expect(fr7.every((r) => r.outcome === "drop" && r.reason.includes("disabled"))).toBe(true);

    const auto = await call<SimulatedFinding[]>("simulate_guardrails", { projectId: 1, overrides: { posting: { mode: "auto" }, review: { min_confidence: 0 } }, limitRuns: 10 });
    expect(auto.some((r) => r.outcome === "post")).toBe(true);
    expect(auto.filter((r) => r.outcome === "draft")).toHaveLength(0);

    const tight = await call<SimulatedFinding[]>("simulate_guardrails", { projectId: 1, overrides: { review: { max_notes: 1, min_confidence: 0 } }, limitRuns: 10 });
    expect(tight.some((r) => r.reason.includes("note budget"))).toBe(true);
  });

  it("preview_note renders the note as the poster would", async () => {
    const md = await call<string>("preview_note", { findingId: 1001 });
    expect(md.startsWith("🔴 Blocker · **SQL built with an f-string from request input**")).toBe(true);
    expect(md).toMatch(/— Warden · rule FR-002 · confidence \d\.\d\d$/);
  });

  it("list_pending_findings returns drafts from the latest run of open MRs only", async () => {
    const rows = await call<PendingFinding[]>("list_pending_findings");
    expect(rows.length).toBeGreaterThan(0);
    expect(rows.every((r) => r.finding.status === "draft" || r.finding.status === "approved")).toBe(true);
    expect(new Set(rows.map((r) => r.mr_iid))).toEqual(new Set([31, 28]));
    expect(rows[0].project_name).toBe("entries_crm");
    expect(rows[0].web_url).toContain("/merge_requests/31");
  });

  it("post_findings_bulk posts across runs and returns one report per run", async () => {
    const reports = await call<PostReport[]>("post_findings_bulk", { findingIds: [1001, 1002, 1101] });
    expect(reports).toHaveLength(2);
    expect(reports.reduce((a, r) => a + r.posted, 0)).toBe(3);
    const left = await call<PendingFinding[]>("list_pending_findings");
    expect(left.some((r) => [1001, 1002, 1101].includes(r.finding.id))).toBe(false);
  });

  it("open_settings_window is a no-op in the browser", async () => {
    await expect(call("open_settings_window", { pane: "projects" })).resolves.toBeNull();
  });
});

describe("mock backend: history rows", () => {
  it("list_run_rows covers merged MRs with per-run counts", async () => {
    type RunRow = import("@/bindings/RunRow").RunRow;
    const rows = await call<RunRow[]>("list_run_rows", { limit: 500 });
    const runs = await call<{ id: number }[]>("list_runs", { mrId: null, limit: 500 });
    expect(rows.map((r) => r.run.id)).toEqual(runs.map((r) => r.id));
    const r101 = rows.find((r) => r.run.id === 101)!;
    expect(r101).toMatchObject({ mr_iid: 31, project_name: "entries_crm", project_id: 1 });
    expect(r101.finding_counts).toEqual({ draft: 4, posted: 0, rejected: 2 });
    expect(r101.ci_summary).toEqual({ passed: 3, failed: 0, running: 0, total: 3 });
    const merged = rows.filter((r) => r.run.id >= 60 && r.run.id < 100);
    expect(merged.length).toBe(22);
    expect(merged.some((r) => r.finding_counts.posted > 0)).toBe(true);
    expect((await call<RunRow[]>("list_run_rows", { limit: 3 })).length).toBe(3);
  });
});
