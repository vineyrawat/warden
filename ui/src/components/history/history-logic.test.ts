import { describe, expect, it } from "vitest";

import type { AuditEvent } from "@/bindings/AuditEvent";
import type { ReviewRun } from "@/bindings/ReviewRun";
import type { RunRow as RunRowDto } from "@/bindings/RunRow";
import type { Stats } from "@/bindings/Stats";
import {
  buildKpis,
  buildRows,
  chartBars,
  ciLabel,
  compareWeeks,
  countDelta,
  DAY_MS,
  DEFAULT_FILTERS,
  durationDelta,
  eventPhase,
  filterRows,
  groupEvents,
  isDefaultFilters,
  isErrorEvent,
  matchesQuery,
  medianOf,
  medianRunMs,
  moveCursor,
  niceMax,
  phaseCounts,
  rangeStart,
  rateDelta,
  statusKey,
  statusPillText,
  tallyFindings,
  type RunRow,
} from "./history-logic";
import { summarize } from "./summarize";

const NOW = Date.parse("2026-11-25T12:00:00Z");

function run(p: Partial<ReviewRun> & { id: number }): ReviewRun {
  return {
    mr_id: 1,
    base_sha: "aaaa111122223333",
    head_sha: "bbbb444455556666",
    since_sha: null,
    status: "posted",
    model: "opus",
    turns: 10,
    duration_ms: 60_000,
    cli_session_id: null,
    summary: null,
    verdict: null,
    error: null,
    started_at: new Date(NOW - DAY_MS).toISOString(),
    ended_at: new Date(NOW - DAY_MS + 60_000).toISOString(),
    ...p,
  };
}

function dto(p: Partial<RunRowDto> & { run: ReviewRun }): RunRowDto {
  return {
    mr_iid: 41,
    mr_title: "Import review",
    author: "vinay",
    web_url: "https://gitlab.com/x/-/merge_requests/41",
    draft: false,
    project_id: 1,
    project_name: "entries_crm",
    finding_counts: { draft: 0, posted: 0, rejected: 0 },
    ci_summary: { passed: 0, failed: 0, running: 0, total: 0 },
    ...p,
  };
}

const row = (p: Partial<RunRow> & { run: ReviewRun }): RunRow => ({ mr: null, projectName: null, findings: null, ci: null, ...p });

describe("summarize", () => {
  it("leads with the tool / event / decision", () => {
    expect(summarize({ tool: "mr_get_diff", args: { page: 1 } })).toBe('mr_get_diff · args={"page":1}');
    expect(summarize({ event: "start", model: "opus" })).toBe("start · model=opus");
    expect(summarize({ summary: true, note: "n1" })).toBe("summary note · summary=true note=n1");
    expect(summarize(null)).toBe("");
    expect(summarize("plain")).toBe("plain");
  });
});

describe("rows", () => {
  it("tallies findings and CI", () => {
    expect(tallyFindings([{ status: "posted" }, { status: "resolved" }, { status: "draft" }, { status: "approved" }, { status: "rejected" }, { status: "dismissed" }])).toEqual({
      posted: 2,
      drafted: 2,
      dropped: 1,
    });
  });

  it("maps list_run_rows rows (merged MRs and local runs included)", () => {
    const r1 = run({ id: 1, mr_id: 10 });
    const r2 = run({ id: 2, mr_id: 20 });
    const rows = buildRows([
      dto({ run: r1, finding_counts: { draft: 1, posted: 2, rejected: 3 }, ci_summary: { passed: 1, failed: 0, running: 0, total: 1 } }),
      dto({ run: r2, mr_iid: 0, mr_title: "local: main..feature", project_id: 0, project_name: "repo" }),
    ]);
    expect(rows[0].mr).toEqual({ id: 10, iid: 41, title: "Import review", author: "vinay", project_id: 1, web_url: "https://gitlab.com/x/-/merge_requests/41", draft: false });
    expect(rows[0]).toMatchObject({ projectName: "entries_crm", findings: { posted: 2, drafted: 1, dropped: 3 }, ci: { total: 1 } });
    expect(rows[1].projectName).toBe("local");
    expect(rows[1].mr?.project_id).toBe(0);
    expect(filterRows(rows, { ...DEFAULT_FILTERS, projectId: 0, range: "all" }, NOW).map((r) => r.run.id)).toEqual([2]);
  });

  it("labels CI, verdict-independent status pills", () => {
    expect(ciLabel(null).label).toBe("—");
    expect(ciLabel({ passed: 2, failed: 1, running: 0, total: 3 })).toEqual({ label: "1 failed", tone: "bad" });
    expect(ciLabel({ passed: 3, failed: 0, running: 0, total: 3 })).toEqual({ label: "3/3 passed", tone: "good" });
    expect(statusPillText({ run: run({ id: 1 }), findings: { posted: 2, drafted: 0, dropped: 0 } }, "Posted")).toBe("Posted 2");
    expect(statusPillText({ run: run({ id: 1, status: "skipped", error: "draft MR" }), findings: null }, "Skipped")).toBe("Skipped: draft MR");
  });
});

describe("filters", () => {
  const mr = { id: 10, iid: 41, title: "Import review", author: "priya", project_id: 1, web_url: "", draft: false };
  const rows = [
    row({ run: run({ id: 212, status: "awaiting_approval", started_at: new Date(NOW - 3600_000).toISOString() }), mr, projectName: "entries_crm" }),
    row({ run: run({ id: 209, status: "posted", started_at: new Date(NOW - 3 * DAY_MS).toISOString(), head_sha: "a1f9c2e00" }), mr: { ...mr, iid: 38, title: "Duplicate merge", project_id: 2 }, projectName: "entries_desk" }),
    row({ run: run({ id: 150, status: "reviewing", started_at: new Date(NOW - 20 * DAY_MS).toISOString() }) }),
  ];

  it("maps statuses and ranges", () => {
    expect(statusKey("reviewing")).toBe("active");
    expect(statusKey("posted")).toBe("posted");
    expect(rangeStart("all", NOW)).toBeNull();
    expect(rangeStart("7d", NOW)).toBe(NOW - 7 * DAY_MS);
    expect(rangeStart("today", NOW)! <= NOW).toBe(true);
    expect(NOW - rangeStart("today", NOW)!).toBeLessThan(DAY_MS);
  });

  it("searches title, !iid, #run, author and sha", () => {
    expect(matchesQuery(rows[0], "import")).toBe(true);
    expect(matchesQuery(rows[0], "!41")).toBe(true);
    expect(matchesQuery(rows[0], "!4")).toBe(false);
    expect(matchesQuery(rows[0], "#212")).toBe(true);
    expect(matchesQuery(rows[0], "212")).toBe(true);
    expect(matchesQuery(rows[0], "PRIYA")).toBe(true);
    expect(matchesQuery(rows[1], "a1f9")).toBe(true);
    expect(matchesQuery(rows[1], "zzz")).toBe(false);
  });

  it("combines range, status, project and text", () => {
    expect(filterRows(rows, DEFAULT_FILTERS, NOW).map((r) => r.run.id)).toEqual([212, 209]);
    expect(filterRows(rows, { ...DEFAULT_FILTERS, range: "all" }, NOW)).toHaveLength(3);
    expect(filterRows(rows, { ...DEFAULT_FILTERS, range: "all", statuses: ["active", "posted"] }, NOW).map((r) => r.run.id)).toEqual([209, 150]);
    expect(filterRows(rows, { ...DEFAULT_FILTERS, range: "all", projectId: 2 }, NOW).map((r) => r.run.id)).toEqual([209]);
    expect(filterRows(rows, { ...DEFAULT_FILTERS, query: "dup" }, NOW).map((r) => r.run.id)).toEqual([209]);
    expect(isDefaultFilters(DEFAULT_FILTERS)).toBe(true);
    expect(isDefaultFilters({ ...DEFAULT_FILTERS, query: "  " })).toBe(true);
    expect(isDefaultFilters({ ...DEFAULT_FILTERS, statuses: ["failed"] })).toBe(false);
  });

  it("moves the keyboard cursor within bounds", () => {
    expect(moveCursor([], null, 1)).toBeNull();
    expect(moveCursor([5, 6, 7], null, 1)).toBe(5);
    expect(moveCursor([5, 6, 7], null, -1)).toBe(7);
    expect(moveCursor([5, 6, 7], 6, 1)).toBe(7);
    expect(moveCursor([5, 6, 7], 7, 1)).toBe(7);
    expect(moveCursor([5, 6, 7], 5, -1)).toBe(5);
    expect(moveCursor([5, 6, 7], 99, 1)).toBe(5);
  });
});

describe("kpis", () => {
  const perDay = Array.from({ length: 14 }, (_, i) => ({
    date: new Date(Date.parse("2026-11-12T00:00:00Z") + i * DAY_MS).toISOString().slice(0, 10),
    reviews: i < 7 ? 1 : 2,
    notes: i < 7 ? 2 : 3,
  }));
  const s14: Stats = { reviews: 21, notes_posted: 35, resolved: 20, resolved_rate: 20 / 35, median_run_ms: 50_000, per_day: perDay };
  const s7: Stats = { reviews: 14, notes_posted: 21, resolved: 14, resolved_rate: 14 / 21, median_run_ms: 40_000, per_day: perDay.slice(7) };

  it("computes medians like get_stats", () => {
    expect(medianOf([])).toBe(0);
    expect(medianOf([3, 1, 2])).toBe(2);
    expect(medianOf([1, 2, 3, 4])).toBe(3);
    const from = Date.parse("2026-11-12T00:00:00Z");
    const to = from + 7 * DAY_MS;
    const runs = [
      run({ id: 1, started_at: new Date(from + 1000).toISOString(), duration_ms: 30_000 }),
      run({ id: 2, started_at: new Date(from + 2000).toISOString(), duration_ms: 90_000, status: "awaiting_approval" }),
      run({ id: 3, started_at: new Date(from + 3000).toISOString(), duration_ms: 999_000, status: "failed" }),
      run({ id: 4, started_at: new Date(to).toISOString(), duration_ms: 999_000 }),
    ];
    expect(medianRunMs(runs, from, to)).toBe(60_000);
  });

  it("splits 14 days into this week vs last", () => {
    const runs = [run({ id: 1, started_at: "2026-11-13T10:00:00Z", duration_ms: 70_000 })];
    const c = compareWeeks(s14, s7, runs);
    expect(c.reviews).toEqual([14, 7]);
    expect(c.notes).toEqual([21, 14]);
    expect(c.resolvedRate[0]).toBeCloseTo(14 / 21);
    expect(c.resolvedRate[1]).toBeCloseTo(6 / 14);
    expect(c.medianMs).toEqual([40_000, 70_000]);
    const k = buildKpis(c);
    expect(k.map((x) => x.value)).toEqual(["14", "21", "67%", "40s"]);
    expect(k[0].delta).toBe("+7 vs last week");
    expect(k[2]).toMatchObject({ delta: "+24 pts vs last week", tone: "good" });
    expect(k[3]).toMatchObject({ delta: "−30s vs last week", tone: "good" });
  });

  it("phrases deltas", () => {
    expect(countDelta(3, 5).text).toBe("−2 vs last week");
    expect(countDelta(4, 4).text).toBe("same as last week");
    expect(rateDelta(0.5, null).text).toBe("nothing posted last week");
    expect(rateDelta(null, null).text).toBe("nothing posted either week");
    expect(durationDelta(40_000, 0).text).toBe("no finished runs last week");
    expect(durationDelta(100_000, 40_000)).toEqual({ text: "+1m vs last week", diff: 60_000 });
  });
});

describe("chart", () => {
  it("rounds the axis to a nice maximum", () => {
    expect(niceMax(0)).toBe(1);
    expect(niceMax(3)).toBe(3);
    expect(niceMax(6)).toBe(10);
    expect(niceMax(12)).toBe(20);
    expect(niceMax(23)).toBe(25);
    expect(niceMax(51)).toBe(100);
  });

  it("draws bars to scale", () => {
    const days = [
      { date: "2026-11-24", reviews: 0, notes: 0 },
      { date: "2026-11-25", reviews: 2, notes: 1 },
      { date: "2026-11-26", reviews: 4, notes: 0 },
      { date: "2026-11-27", reviews: 1, notes: 0 },
    ];
    const { bars, max } = chartBars(days, 100, 40, 4, 2);
    expect(max).toBe(4);
    expect(bars.map((b) => b.h)).toEqual([0, 20, 40, 10]);
    expect(bars.map((b) => b.y)).toEqual([40, 20, 0, 30]);
    expect(bars[0]).toMatchObject({ x: 2, w: 21, current: false });
    expect(bars[3]).toMatchObject({ x: 77, current: true });
  });
});

describe("audit events", () => {
  let id = 0;
  const ev = (kind: string, payload: unknown, sec: number): AuditEvent => ({ id: ++id, run_id: 1, kind, payload, created_at: new Date(NOW + sec * 1000).toISOString() });

  it("classifies errors", () => {
    expect(isErrorEvent({ kind: "error", payload: {} })).toBe(true);
    expect(isErrorEvent({ kind: "engine", payload: { event: "timeout" } })).toBe(true);
    expect(isErrorEvent({ kind: "tool_call", payload: { tool: "x", error: "boom" } })).toBe(true);
    expect(isErrorEvent({ kind: "engine", payload: { event: "start" } })).toBe(false);
    expect(eventPhase({ kind: "weird", payload: null })).toBe("other");
  });

  it("groups consecutive phases oldest first and filters", () => {
    const events = [
      ev("engine", { event: "result" }, 30),
      ev("tool_call", { tool: "b" }, 20),
      ev("engine", { event: "start" }, 0),
      ev("tool_call", { tool: "a" }, 10),
      ev("guardrail", { decision: "rejected" }, 25),
      ev("error", { message: "x" }, 40),
    ];
    const g = groupEvents(events);
    expect(g.map((x) => [x.phase, x.events.length])).toEqual([
      ["engine", 1],
      ["tool_call", 2],
      ["guardrail", 1],
      ["engine", 1],
      ["error", 1],
    ]);
    expect(g[1].events.map((e) => (e.payload as { tool: string }).tool)).toEqual(["a", "b"]);
    expect(groupEvents(events, ["engine"]).map((x) => x.events.length)).toEqual([2]);
    expect(phaseCounts(events)).toMatchObject({ engine: 2, tool_call: 2, guardrail: 1, error: 1, post: 0 });
  });
});
