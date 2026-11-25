// Pure helpers for the Inbox list + preview (UX section 3, variant C with A's groups).
import type { CiSummary } from "@/bindings/CiSummary";
import type { Finding } from "@/bindings/Finding";
import type { MrRow } from "@/bindings/MrRow";
import type { Severity } from "@/bindings/Severity";
import { SEVERITIES, formatRelative, isRunActive, sortFindings } from "@/lib/format";

export type GroupKey = "needs" | "reviewing" | "done" | "skipped";

export const GROUPS: { key: GroupKey; label: string }[] = [
  { key: "needs", label: "Needs you" },
  { key: "reviewing", label: "Reviewing" },
  { key: "done", label: "Done" },
  { key: "skipped", label: "Skipped" },
];

/**
 * Which list group an MR belongs to, from its latest run:
 * - Needs you: awaiting approval with findings to decide, failed, findings left after posting, never reviewed
 * - Reviewing: queued … posting
 * - Done: posted, or a review with nothing left to decide (clean)
 * - Skipped: skipped or cancelled runs, and draft MRs that were never reviewed
 */
export function groupOf(row: Pick<MrRow, "latest_run" | "finding_counts" | "draft">): GroupKey {
  const run = row.latest_run;
  if (!run) return row.draft ? "skipped" : "needs";
  if (isRunActive(run.status)) return "reviewing";
  switch (run.status) {
    case "failed":
      return "needs";
    case "awaiting_approval":
    case "posted":
      return row.finding_counts.draft > 0 ? "needs" : "done";
    default:
      return "skipped";
  }
}

function time(iso: string | null | undefined): number {
  const t = iso ? Date.parse(iso) : NaN;
  return Number.isNaN(t) ? 0 : t;
}

/** Lower sorts first inside "Needs you": findings to decide, then failures, then never reviewed. */
function needsRank(row: Pick<MrRow, "latest_run">): number {
  const s = row.latest_run?.status;
  if (s === "awaiting_approval" || s === "posted") return 0;
  if (s === "failed") return 1;
  return 2;
}

/** Order inside a group. Needs you: by kind, then most findings to decide; others: most recent first. */
export function compareInGroup(group: GroupKey, a: MrRow, b: MrRow): number {
  if (group === "needs") {
    const d = needsRank(a) - needsRank(b) || b.finding_counts.draft - a.finding_counts.draft;
    if (d) return d;
  }
  if (group === "reviewing") {
    const d = time(b.latest_run?.started_at) - time(a.latest_run?.started_at);
    if (d) return d;
  }
  return time(b.updated_at) - time(a.updated_at) || b.id - a.id;
}

export type InboxGroup = { key: GroupKey; label: string; rows: MrRow[] };

/** All four groups in display order (empty ones included, so counts can show 0). */
export function buildGroups(rows: MrRow[]): InboxGroup[] {
  return GROUPS.map((g) => ({
    ...g,
    rows: rows.filter((r) => groupOf(r) === g.key).sort((a, b) => compareInGroup(g.key, a, b)),
  }));
}

/** Free-text filter over !iid, title, project, author and branches. */
export function matchesFilter(row: Pick<MrRow, "iid" | "title" | "project_name" | "author" | "source_branch" | "target_branch">, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const iid = q.replace(/^!/, "");
  if (/^\d+$/.test(iid) && String(row.iid) === iid) return true;
  return [row.title, row.project_name, row.author, row.source_branch, row.target_branch].some((s) => s.toLowerCase().includes(q));
}

/** MR ids in on-screen order, skipping collapsed groups. */
export function visibleIds(groups: InboxGroup[], collapsed: ReadonlySet<GroupKey>): number[] {
  return groups.flatMap((g) => (collapsed.has(g.key) ? [] : g.rows.map((r) => r.id)));
}

/** ↑/↓ selection: clamps at the ends; an unknown or missing current id starts at the first (↓) or last (↑) row. */
export function moveSelection(ids: number[], current: number | null, delta: number): number | null {
  if (ids.length === 0) return null;
  const i = current == null ? -1 : ids.indexOf(current);
  if (i < 0) return delta < 0 ? ids[ids.length - 1] : ids[0];
  return ids[Math.max(0, Math.min(ids.length - 1, i + delta))];
}

/** Keep the current selection while it is visible, else fall back to the first visible row. */
export function resolveSelection(ids: number[], current: number | null): number | null {
  if (current != null && ids.includes(current)) return current;
  return ids[0] ?? null;
}

// ---- findings ---------------------------------------------------------------------------------

/** "Approve N & post": undecided drafts get approved, then drafts + already-approved get posted. */
export function approvePostPlan(findings: Pick<Finding, "id" | "status">[]): { approve: number[]; post: number[] } {
  const approve = findings.filter((f) => f.status === "draft").map((f) => f.id);
  const already = findings.filter((f) => f.status === "approved").map((f) => f.id);
  return { approve, post: [...approve, ...already] };
}

/** Findings still waiting on a decision (shown as severity chips on the list row). */
export function pendingFindings<T extends Pick<Finding, "status">>(findings: T[]): T[] {
  return findings.filter((f) => f.status === "draft" || f.status === "approved");
}

export function severityCounts(findings: Pick<Finding, "severity">[]): { severity: Severity; count: number }[] {
  return SEVERITIES.map((severity) => ({ severity, count: findings.filter((f) => f.severity === severity).length })).filter((c) => c.count > 0);
}

/** Preview list: severity groups (most severe first), sorted inside; dismissed findings go last in their group. */
export function findingsBySeverity<T extends Pick<Finding, "severity" | "confidence" | "file" | "line" | "status">>(findings: T[]): { severity: Severity; findings: T[] }[] {
  return SEVERITIES.map((severity) => {
    const list = sortFindings(findings.filter((f) => f.severity === severity));
    return { severity, findings: [...list.filter((f) => f.status !== "dismissed"), ...list.filter((f) => f.status === "dismissed")] };
  }).filter((g) => g.findings.length > 0);
}

// ---- display ----------------------------------------------------------------------------------

export type DotTone = "green" | "red" | "blue" | "orange" | "none";

/** CI dot on a list row. */
export function ciDot(s: CiSummary): { tone: DotTone; label: string } {
  if (s.total === 0) return { tone: "none", label: "No CI jobs" };
  if (s.failed > 0) return { tone: "red", label: `CI: ${s.failed} failed` };
  if (s.running > 0) return { tone: "blue", label: "CI running" };
  if (s.passed === s.total) return { tone: "green", label: "CI passed" };
  return { tone: "orange", label: `CI: ${s.passed} of ${s.total} passed, rest skipped or missing` };
}

export function verdictMeta(verdict: string | null | undefined): { label: string; tone: "orange" | "green" | "blue" } | null {
  switch (verdict) {
    case "request_changes":
      return { label: "Request changes", tone: "orange" };
    case "approve":
      return { label: "Approve", tone: "green" };
    case "comment":
      return { label: "Comment", tone: "blue" };
    default:
      return null;
  }
}

/** "2h", "20m", "1d", "now" (list rows are narrow). */
export function shortRelative(iso: string | null | undefined, now: number = Date.now()): string {
  const r = formatRelative(iso, now);
  return r === "just now" ? "now" : r.replace(/ ago$/, "");
}

export function basename(path: string): string {
  const i = path.lastIndexOf("/");
  return i >= 0 ? path.slice(i + 1) : path;
}

/** Default base branch for a local review: the project's first literal (non-glob) target branch. */
export function defaultBaseBranch(targetBranches: string[] | undefined): string {
  return targetBranches?.find((b) => !/[*?[]/.test(b)) ?? "main";
}

/** Start/re-run button label for an MR. */
export function reviewActionLabel(row: Pick<MrRow, "latest_run">): string {
  const s = row.latest_run?.status;
  if (!s) return "Start review";
  if (s === "skipped") return "Review anyway";
  if (s === "failed" || s === "cancelled") return "Retry";
  return "Re-run";
}
