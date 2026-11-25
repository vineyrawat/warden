// Pure helpers for the History screen: row assembly, filters, week-over-week KPIs, chart scaling and
// audit-event grouping. No React here; unit tested in history-logic.test.ts.
import type { AuditEvent } from "@/bindings/AuditEvent";
import type { CiJob } from "@/bindings/CiJob";
import type { CiSummary } from "@/bindings/CiSummary";
import type { DayStat } from "@/bindings/DayStat";
import type { Finding } from "@/bindings/Finding";
import type { MergeRequest } from "@/bindings/MergeRequest";
import type { ReviewRun } from "@/bindings/ReviewRun";
import type { RunRow as RunRowDto } from "@/bindings/RunRow";
import type { RunStatus } from "@/bindings/RunStatus";
import type { Stats } from "@/bindings/Stats";
import { formatDuration } from "@/lib/format";

export const DAY_MS = 86_400_000;
const MINUS = "−";

// ---- rows ------------------------------------------------------------------------------------

export type RunMr = Pick<MergeRequest, "id" | "iid" | "title" | "author" | "project_id" | "web_url" | "draft">;
export type FindingTally = { posted: number; drafted: number; dropped: number };
export type RunRow = {
  run: ReviewRun;
  /** null only when the run is not in the loaded rows (drawer fallback before get_run answers). */
  mr: RunMr | null;
  projectName: string | null;
  /** null when the counts are not known yet. */
  findings: FindingTally | null;
  ci: CiSummary | null;
};

/** posted = posted or resolved, drafted = draft or approved, dropped = rejected by guardrails. */
export function tallyFindings(fs: Pick<Finding, "status">[]): FindingTally {
  const t = { posted: 0, drafted: 0, dropped: 0 };
  for (const f of fs) {
    if (f.status === "posted" || f.status === "resolved") t.posted++;
    else if (f.status === "draft" || f.status === "approved") t.drafted++;
    else if (f.status === "rejected") t.dropped++;
  }
  return t;
}

export function summarizeCi(jobs: Pick<CiJob, "status">[]): CiSummary {
  const s = { passed: 0, failed: 0, running: 0, total: jobs.length };
  for (const j of jobs) {
    if (j.status === "passed") s.passed++;
    else if (j.status === "failed" || j.status === "timed_out") s.failed++;
    else if (j.status === "running" || j.status === "pending") s.running++;
  }
  return s;
}

/** History rows from `list_run_rows` (one SQL query, no git work; merged/closed MRs and local runs included). */
export function buildRows(rows: RunRowDto[]): RunRow[] {
  return rows.map((r) => ({
    run: r.run,
    mr: { id: r.run.mr_id, iid: r.mr_iid, title: r.mr_title, author: r.author, project_id: r.project_id, web_url: r.web_url, draft: r.draft },
    projectName: r.project_id === 0 ? "local" : r.project_name || null,
    findings: { posted: r.finding_counts.posted, drafted: r.finding_counts.draft, dropped: r.finding_counts.rejected },
    ci: r.ci_summary,
  }));
}

// ---- filters ---------------------------------------------------------------------------------

export type DateRange = "today" | "7d" | "30d" | "all";
export const DATE_RANGES: { value: DateRange; label: string }[] = [
  { value: "today", label: "Today" },
  { value: "7d", label: "7 days" },
  { value: "30d", label: "30 days" },
  { value: "all", label: "All" },
];

export type StatusKey = "active" | "awaiting_approval" | "posted" | "failed" | "skipped" | "cancelled";
export const STATUS_OPTIONS: { value: StatusKey; label: string }[] = [
  { value: "awaiting_approval", label: "Awaiting approval" },
  { value: "posted", label: "Posted" },
  { value: "failed", label: "Failed" },
  { value: "skipped", label: "Skipped" },
  { value: "cancelled", label: "Cancelled" },
  { value: "active", label: "In progress" },
];

export function statusKey(s: RunStatus): StatusKey {
  switch (s) {
    case "awaiting_approval":
    case "posted":
    case "failed":
    case "skipped":
    case "cancelled":
      return s;
    default:
      return "active";
  }
}

export type HistoryFilters = { projectId: number | null; statuses: StatusKey[]; range: DateRange; query: string };
export const DEFAULT_FILTERS: HistoryFilters = { projectId: null, statuses: [], range: "7d", query: "" };

export function isDefaultFilters(f: HistoryFilters): boolean {
  return f.projectId === null && f.statuses.length === 0 && f.range === DEFAULT_FILTERS.range && f.query.trim() === "";
}

/** Earliest start time included by the range (Today = local midnight), or null for All. */
export function rangeStart(range: DateRange, now: number): number | null {
  switch (range) {
    case "today": {
      const d = new Date(now);
      d.setHours(0, 0, 0, 0);
      return d.getTime();
    }
    case "7d":
      return now - 7 * DAY_MS;
    case "30d":
      return now - 30 * DAY_MS;
    case "all":
      return null;
  }
}

/**
 * Text search over MR title, `!iid`, author, project, `#run` and shas. `!41` and `#212` match the
 * iid / run id exactly; a bare number matches either; hex of 4+ chars matches a sha prefix.
 */
export function matchesQuery(row: RunRow, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const { run, mr } = row;
  if (/^![0-9]+$/.test(q)) return mr != null && String(mr.iid) === q.slice(1);
  if (/^#[0-9]+$/.test(q)) return String(run.id) === q.slice(1);
  if (/^[0-9]+$/.test(q) && ((mr && String(mr.iid) === q) || String(run.id) === q)) return true;
  if (/^[0-9a-f]{4,40}$/.test(q) && [run.head_sha, run.base_sha, run.since_sha].some((s) => s?.toLowerCase().startsWith(q))) return true;
  const hay = [mr?.title, mr?.author, row.projectName, mr ? `!${mr.iid}` : null].filter(Boolean).join(" \u0000 ").toLowerCase();
  return hay.includes(q);
}

export function filterRows(rows: RunRow[], f: HistoryFilters, now: number): RunRow[] {
  const from = rangeStart(f.range, now);
  const statuses = new Set(f.statuses);
  return rows.filter((r) => {
    if (from != null && Date.parse(r.run.started_at) < from) return false;
    if (statuses.size && !statuses.has(statusKey(r.run.status))) return false;
    if (f.projectId != null && (r.mr == null || r.mr.project_id !== f.projectId)) return false;
    return matchesQuery(r, f.query);
  });
}

/** Index after moving the keyboard cursor by `step` (clamped; -1 when the list is empty). */
export function moveCursor(ids: number[], current: number | null, step: number): number | null {
  if (!ids.length) return null;
  const i = current == null ? -1 : ids.indexOf(current);
  if (i === -1) return step > 0 ? ids[0] : ids[ids.length - 1];
  return ids[Math.max(0, Math.min(ids.length - 1, i + step))];
}

// ---- display helpers -------------------------------------------------------------------------

export function verdictLabel(v: string | null | undefined): string {
  if (!v) return "—";
  return { approve: "Approve", comment: "Comment", request_changes: "Request changes" }[v] ?? v.replace(/_/g, " ");
}

export function verdictTone(v: string | null | undefined): "good" | "bad" | "neutral" {
  return v === "approve" ? "good" : v === "request_changes" ? "bad" : "neutral";
}

export function ciLabel(ci: CiSummary | null): { label: string; tone: "good" | "bad" | "neutral" | "active" } {
  if (!ci || ci.total === 0) return { label: "—", tone: "neutral" };
  if (ci.failed > 0) return { label: `${ci.failed} failed`, tone: "bad" };
  if (ci.running > 0) return { label: "running", tone: "active" };
  return { label: `${ci.passed}/${ci.total} passed`, tone: ci.passed === ci.total ? "good" : "neutral" };
}

/** Status pill text: "Posted 2", "Skipped: draft MR", else the plain run status label. */
export function statusPillText(row: Pick<RunRow, "run" | "findings">, label: string): string {
  const { run, findings } = row;
  if (run.status === "posted" && findings) return `Posted ${findings.posted}`;
  if (run.status === "skipped" && run.error) return `Skipped: ${run.error.length > 24 ? `${run.error.slice(0, 23)}…` : run.error}`;
  return label;
}

// ---- KPIs ------------------------------------------------------------------------------------

export function medianOf(ns: number[]): number {
  if (!ns.length) return 0;
  const s = [...ns].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : Math.round((s[m - 1] + s[m]) / 2);
}

/** Median duration of finished runs started in [from, to), same definition as `get_stats`. */
export function medianRunMs(runs: ReviewRun[], from: number, to: number): number {
  return medianOf(
    runs
      .filter((r) => {
        const t = Date.parse(r.started_at);
        return t >= from && t < to && (r.status === "posted" || r.status === "awaiting_approval") && !!r.ended_at && r.duration_ms > 0;
      })
      .map((r) => r.duration_ms),
  );
}

/** UTC midnight of a `YYYY-MM-DD` per-day row. */
export function dayStart(date: string): number {
  return Date.parse(`${date}T00:00:00Z`);
}

export type WeekCompare = {
  reviews: [number, number];
  notes: [number, number];
  /** null when nothing was posted that week. */
  resolvedRate: [number | null, number | null];
  medianMs: [number, number];
};

const sum = (days: DayStat[], k: "reviews" | "notes") => days.reduce((a, d) => a + d[k], 0);

/**
 * Last 7 days vs the 7 before, from `get_stats(14)` (per-day counts), `get_stats(7)` (resolved
 * totals and current median) and the run list (previous week's median, which stats can't split).
 */
export function compareWeeks(s14: Stats, s7: Stats, runs: ReviewRun[]): WeekCompare {
  const days = s14.per_day;
  const split = Math.max(0, days.length - 7);
  const prevDays = days.slice(0, split);
  const curDays = days.slice(split);
  const prevPosted = Math.max(0, s14.notes_posted - s7.notes_posted);
  const prevResolved = Math.max(0, s14.resolved - s7.resolved);
  const prevMedian = prevDays.length ? medianRunMs(runs, dayStart(prevDays[0].date), dayStart(curDays[0]?.date ?? prevDays[0].date)) : 0;
  return {
    reviews: [sum(curDays, "reviews"), sum(prevDays, "reviews")],
    notes: [sum(curDays, "notes"), sum(prevDays, "notes")],
    resolvedRate: [s7.notes_posted ? s7.resolved / s7.notes_posted : null, prevPosted ? prevResolved / prevPosted : null],
    medianMs: [s7.median_run_ms, prevMedian],
  };
}

export type KpiTone = "good" | "bad" | "neutral";
export type Kpi = { key: "reviews" | "notes" | "resolved" | "median"; value: string; label: string; delta: string; tone: KpiTone };

const signed = (n: number, text: string) => (n > 0 ? `+${text}` : n < 0 ? `${MINUS}${text}` : text);

export function countDelta(cur: number, prev: number): { text: string; diff: number } {
  const diff = cur - prev;
  return { text: diff === 0 ? "same as last week" : `${signed(diff, String(Math.abs(diff)))} vs last week`, diff };
}

export function rateDelta(cur: number | null, prev: number | null): { text: string; diff: number } {
  if (cur == null && prev == null) return { text: "nothing posted either week", diff: 0 };
  if (prev == null) return { text: "nothing posted last week", diff: 0 };
  if (cur == null) return { text: "nothing posted this week", diff: 0 };
  const pts = Math.round((cur - prev) * 100);
  return { text: pts === 0 ? "same as last week" : `${signed(pts, String(Math.abs(pts)))} pts vs last week`, diff: pts };
}

export function durationDelta(cur: number, prev: number): { text: string; diff: number } {
  if (!prev) return { text: "no finished runs last week", diff: 0 };
  if (!cur) return { text: "no finished runs this week", diff: 0 };
  const diff = Math.round((cur - prev) / 1000) * 1000;
  return { text: diff === 0 ? "same as last week" : `${signed(diff, formatDuration(Math.abs(diff)))} vs last week`, diff };
}

const toneFor = (diff: number, higherIsBetter: boolean): KpiTone => (diff === 0 ? "neutral" : diff > 0 === higherIsBetter ? "good" : "bad");

export function buildKpis(c: WeekCompare): Kpi[] {
  const r = countDelta(...c.reviews);
  const n = countDelta(...c.notes);
  const rr = rateDelta(...c.resolvedRate);
  const md = durationDelta(...c.medianMs);
  return [
    { key: "reviews", value: String(c.reviews[0]), label: "reviews this week", delta: r.text, tone: "neutral" },
    { key: "notes", value: String(c.notes[0]), label: "notes posted", delta: n.text, tone: "neutral" },
    {
      key: "resolved",
      value: c.resolvedRate[0] == null ? "—" : `${Math.round(c.resolvedRate[0] * 100)}%`,
      label: "threads resolved",
      delta: rr.text,
      tone: toneFor(rr.diff, true),
    },
    { key: "median", value: c.medianMs[0] ? formatDuration(c.medianMs[0]) : "—", label: "median review", delta: md.text, tone: toneFor(md.diff, false) },
  ];
}

// ---- chart -----------------------------------------------------------------------------------

/** Round an axis maximum up to 1–5 or a 1/2/2.5/5 × 10ⁿ step (never below 1). */
export function niceMax(n: number): number {
  if (!(n > 0)) return 1;
  if (n <= 5) return Math.ceil(n);
  const p = 10 ** Math.floor(Math.log10(n));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * p >= n) return m * p;
  return 10 * p;
}

export type Bar = { date: string; reviews: number; notes: number; x: number; w: number; y: number; h: number; current: boolean };

/**
 * Bars in a `width × height` viewBox, drawn to scale against `niceMax(max reviews)`. Days at or
 * after index `currentFrom` belong to the current week.
 */
export function chartBars(perDay: DayStat[], width: number, height: number, gap: number, currentFrom: number): { bars: Bar[]; max: number } {
  const max = niceMax(Math.max(0, ...perDay.map((d) => d.reviews)));
  const n = perDay.length || 1;
  const slot = width / n;
  const w = Math.max(1, slot - gap);
  const bars = perDay.map((d, i) => {
    const h = (d.reviews / max) * height;
    return { date: d.date, reviews: d.reviews, notes: d.notes, x: i * slot + (slot - w) / 2, w, y: height - h, h, current: i >= currentFrom };
  });
  return { bars, max };
}

export function dayLabel(date: string, opts: Intl.DateTimeFormatOptions = { month: "short", day: "numeric" }): string {
  const t = dayStart(date);
  return Number.isNaN(t) ? date : new Date(t).toLocaleDateString(undefined, { ...opts, timeZone: "UTC" });
}

// ---- audit events ----------------------------------------------------------------------------

export type Phase = "engine" | "tool_call" | "guardrail" | "post" | "error" | "other";
export const PHASES: { value: Phase; label: string }[] = [
  { value: "engine", label: "Engine" },
  { value: "tool_call", label: "Tool calls" },
  { value: "guardrail", label: "Guardrail decisions" },
  { value: "post", label: "Posts" },
  { value: "error", label: "Errors" },
  { value: "other", label: "Other" },
];

export function phaseLabel(p: Phase): string {
  return PHASES.find((x) => x.value === p)?.label ?? p;
}

const ERROR_EVENTS = new Set(["error", "failed", "timeout", "timed_out"]);

export function isErrorEvent(ev: Pick<AuditEvent, "kind" | "payload">): boolean {
  if (ev.kind === "error") return true;
  if (ev.payload && typeof ev.payload === "object") {
    const p = ev.payload as Record<string, unknown>;
    if (p.error != null && p.error !== false) return true;
    if (typeof p.event === "string" && ERROR_EVENTS.has(p.event)) return true;
  }
  return false;
}

export function eventPhase(ev: Pick<AuditEvent, "kind" | "payload">): Phase {
  if (isErrorEvent(ev)) return "error";
  return ev.kind === "engine" || ev.kind === "tool_call" || ev.kind === "guardrail" || ev.kind === "post" ? ev.kind : "other";
}

export function sortEventsAsc<T extends Pick<AuditEvent, "id" | "created_at">>(events: T[]): T[] {
  return [...events].sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at) || a.id - b.id);
}

export function phaseCounts(events: Pick<AuditEvent, "kind" | "payload">[]): Record<Phase, number> {
  const c: Record<Phase, number> = { engine: 0, tool_call: 0, guardrail: 0, post: 0, error: 0, other: 0 };
  for (const e of events) c[eventPhase(e)]++;
  return c;
}

export type EventGroup = { key: string; phase: Phase; events: AuditEvent[] };

/** Oldest first, consecutive events of the same phase folded into one group; `only` filters phases. */
export function groupEvents(events: AuditEvent[], only: Phase[] = []): EventGroup[] {
  const keep = new Set(only);
  const groups: EventGroup[] = [];
  for (const ev of sortEventsAsc(events)) {
    const phase = eventPhase(ev);
    if (keep.size && !keep.has(phase)) continue;
    const last = groups[groups.length - 1];
    if (last && last.phase === phase) last.events.push(ev);
    else groups.push({ key: `${phase}-${ev.id}`, phase, events: [ev] });
  }
  return groups;
}

export function toggleIn<T>(list: T[], v: T): T[] {
  return list.includes(v) ? list.filter((x) => x !== v) : [...list, v];
}
