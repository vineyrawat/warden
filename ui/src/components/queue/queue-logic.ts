// Pure helpers for the Approval queue (unit tested in queue-logic.test.ts): filtering, grouping,
// selection and range logic, copy, and keyboard mapping.
import type { PendingFinding } from "@/bindings/PendingFinding";
import type { PostReport } from "@/bindings/PostReport";
import type { Severity } from "@/bindings/Severity";
import { SEVERITIES, capitalize, severityRank } from "@/lib/format";

export type SortMode = "mr" | "severity";

export type QueueFilter = { project: string | null; severity: Severity | null; search: string };

export const EMPTY_FILTER: QueueFilter = { project: null, severity: null, search: "" };

export function isFiltered(f: QueueFilter): boolean {
  return f.project != null || f.severity != null || f.search.trim() !== "";
}

export function plural(n: number, word: string, many = `${word}s`): string {
  return `${n} ${n === 1 ? word : many}`;
}

/** "importer.py:17" (or just the path when the line is unknown). */
export function location(f: PendingFinding["finding"]): string {
  return f.line > 0 ? `${f.file}:${f.line}` : f.file;
}

/** Last path segment plus line, for narrow columns. */
export function shortLocation(f: PendingFinding["finding"]): string {
  const name = f.file.slice(f.file.lastIndexOf("/") + 1);
  return f.line > 0 ? `${name}:${f.line}` : name;
}

/** "Claude", "rule", "CI". */
export function sourceLabel(source: string): string {
  if (source === "ci") return "CI";
  if (source === "rule") return "rule";
  return "Claude";
}

/** Distinct project names, sorted. */
export function projectNames(rows: PendingFinding[]): string[] {
  return [...new Set(rows.map((r) => r.project_name))].sort((a, b) => a.localeCompare(b));
}

/** Every whitespace-separated term must appear in the title, body, file, rule, MR or project. */
export function filterPending(rows: PendingFinding[], f: QueueFilter): PendingFinding[] {
  const terms = f.search.toLowerCase().split(/\s+/).filter(Boolean);
  return rows.filter((r) => {
    if (f.project != null && r.project_name !== f.project) return false;
    if (f.severity != null && r.finding.severity !== f.severity) return false;
    if (terms.length === 0) return true;
    const hay = [
      r.finding.title,
      r.finding.body,
      r.finding.file,
      `${r.finding.file}:${r.finding.line}`,
      r.finding.rule_id ?? "",
      r.finding.category,
      r.mr_title,
      `!${r.mr_iid}`,
      r.project_name,
    ]
      .join("\n")
      .toLowerCase();
    return terms.every((t) => hay.includes(t));
  });
}

/** Most severe first, then highest confidence, then file and line. */
export function compareFindings(a: PendingFinding, b: PendingFinding): number {
  return (
    severityRank(b.finding.severity) - severityRank(a.finding.severity) ||
    b.finding.confidence - a.finding.confidence ||
    a.finding.file.localeCompare(b.finding.file) ||
    a.finding.line - b.finding.line ||
    a.finding.id - b.finding.id
  );
}

export type QueueGroupMr = { id: number; iid: number; title: string; project: string; runId: number; webUrl: string };

export type QueueGroup = {
  key: string;
  label: string;
  rows: PendingFinding[];
  mr?: QueueGroupMr;
  severity?: Severity;
};

/**
 * By MR: one group per merge request in the order the backend returned them (newest first), rows
 * most severe first. By severity: blocker → nit, rows by confidence then MR.
 */
export function groupPending(rows: PendingFinding[], mode: SortMode): QueueGroup[] {
  if (mode === "severity") {
    return SEVERITIES.map((s) => ({
      key: `sev:${s}`,
      label: capitalize(s),
      severity: s,
      rows: rows
        .filter((r) => r.finding.severity === s)
        .sort(
          (a, b) =>
            b.finding.confidence - a.finding.confidence ||
            b.mr_iid - a.mr_iid ||
            a.finding.file.localeCompare(b.finding.file) ||
            a.finding.line - b.finding.line ||
            a.finding.id - b.finding.id,
        ),
    })).filter((g) => g.rows.length > 0);
  }
  const groups: QueueGroup[] = [];
  const byMr = new Map<number, QueueGroup>();
  for (const r of rows) {
    let g = byMr.get(r.mr_id);
    if (!g) {
      g = {
        key: `mr:${r.mr_id}`,
        label: `!${r.mr_iid} ${r.mr_title}`,
        mr: { id: r.mr_id, iid: r.mr_iid, title: r.mr_title, project: r.project_name, runId: r.run_id, webUrl: r.web_url },
        rows: [],
      };
      byMr.set(r.mr_id, g);
      groups.push(g);
    }
    g.rows.push(r);
  }
  for (const g of groups) g.rows.sort(compareFindings);
  return groups;
}

/** Finding ids in display order, skipping collapsed groups. */
export function visibleIds(groups: QueueGroup[], collapsed: ReadonlySet<string>): number[] {
  const out: number[] = [];
  for (const g of groups) if (!collapsed.has(g.key)) for (const r of g.rows) out.push(r.finding.id);
  return out;
}

// ---- selection -------------------------------------------------------------------------------

export function toggleId(sel: ReadonlySet<number>, id: number): Set<number> {
  const next = new Set(sel);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/** Ids from `anchor` to `target` inclusive, in display order. Unknown anchor → just the target. */
export function rangeIds(order: number[], anchor: number | null, target: number): number[] {
  const t = order.indexOf(target);
  if (t < 0) return [];
  const a = anchor == null ? -1 : order.indexOf(anchor);
  if (a < 0) return [target];
  const [lo, hi] = a <= t ? [a, t] : [t, a];
  return order.slice(lo, hi + 1);
}

/** Shift+click: add the anchor…target range to the selection. */
export function selectRange(sel: ReadonlySet<number>, order: number[], anchor: number | null, target: number): Set<number> {
  const next = new Set(sel);
  for (const id of rangeIds(order, anchor, target)) next.add(id);
  return next;
}

/** ⌘A: select every visible row, or clear them when all are already selected. */
export function toggleAll(sel: ReadonlySet<number>, visible: number[]): Set<number> {
  const all = visible.length > 0 && visible.every((id) => sel.has(id));
  const next = new Set(sel);
  for (const id of visible) {
    if (all) next.delete(id);
    else next.add(id);
  }
  return next;
}

/** Group checkbox state. */
export function groupChecked(sel: ReadonlySet<number>, ids: number[]): boolean | "indeterminate" {
  const n = ids.filter((id) => sel.has(id)).length;
  if (n === 0) return false;
  return n === ids.length ? true : "indeterminate";
}

/** Group checkbox click: select the whole group unless it is fully selected already. */
export function toggleGroup(sel: ReadonlySet<number>, ids: number[]): Set<number> {
  return groupChecked(sel, ids) === true ? new Set([...sel].filter((id) => !ids.includes(id))) : new Set([...sel, ...ids]);
}

/** Only ids that still exist (after a refetch or a filter change). */
export function pruneSelection(sel: ReadonlySet<number>, existing: Iterable<number>): Set<number> {
  const keep = new Set(existing);
  return new Set([...sel].filter((id) => keep.has(id)));
}

/** ↑/↓: previous / next visible row, clamped. Unknown current → first row. */
export function moveCursor(order: number[], current: number | null, delta: number): number | null {
  if (order.length === 0) return null;
  const at = current == null ? -1 : order.indexOf(current);
  if (at < 0) return order[0];
  return order[Math.max(0, Math.min(order.length - 1, at + delta))];
}

/** A cursor that still points at a visible row: the same one, else the row now at its old index. */
export function settleCursor(order: number[], current: number | null, lastIndex: number): number | null {
  if (order.length === 0) return null;
  if (current != null && order.includes(current)) return current;
  return order[Math.max(0, Math.min(order.length - 1, lastIndex))];
}

/** What A / D act on: the selection when there is one, else the cursor row. */
export function actionTargets(sel: ReadonlySet<number>, cursor: number | null, order: number[]): number[] {
  if (sel.size > 0) {
    const inOrder = order.filter((id) => sel.has(id));
    const rest = [...sel].filter((id) => !order.includes(id));
    return [...inOrder, ...rest];
  }
  return cursor == null ? [] : [cursor];
}

/** Where the cursor goes after `removed` rows leave the list: the next row after the last one, else the one before. */
export function cursorAfterRemoval(order: number[], cursor: number | null, removed: number[]): number | null {
  const gone = new Set(removed);
  const left = order.filter((id) => !gone.has(id));
  if (left.length === 0) return null;
  if (cursor != null && !gone.has(cursor)) return cursor;
  const at = cursor == null ? 0 : order.indexOf(cursor);
  for (let i = at + 1; i < order.length; i++) if (!gone.has(order[i])) return order[i];
  for (let i = at - 1; i >= 0; i--) if (!gone.has(order[i])) return order[i];
  return left[0];
}

/** Next undecided (draft) row after `current`, wrapping; null when there is none. */
export function nextUndecided(order: number[], byId: ReadonlyMap<number, PendingFinding>, current: number | null, skip: ReadonlySet<number> = new Set()): number | null {
  const n = order.length;
  const at = current == null ? -1 : order.indexOf(current);
  for (let step = 1; step <= n; step++) {
    const id = order[(((at + step) % n) + n) % n];
    if (id === current || skip.has(id)) continue;
    if (byId.get(id)?.finding.status === "draft") return id;
  }
  return null;
}

// ---- posting ---------------------------------------------------------------------------------

export type PostSet = { ids: number[]; mrs: number };

/** Every approved finding, across all MRs. */
export function approvedSet(rows: PendingFinding[]): PostSet {
  const list = rows.filter((r) => r.finding.status === "approved");
  return { ids: list.map((r) => r.finding.id), mrs: new Set(list.map((r) => r.mr_id)).size };
}

/** The given ids (that exist in `rows`) and how many MRs they touch. */
export function postSet(rows: PendingFinding[], ids: Iterable<number>): PostSet {
  const want = new Set(ids);
  const list = rows.filter((r) => want.has(r.finding.id));
  return { ids: list.map((r) => r.finding.id), mrs: new Set(list.map((r) => r.mr_id)).size };
}

/** "GitLab", "GitHub" or "GitLab and GitHub", from the MRs' web URLs. */
export function forgeName(rows: Pick<PendingFinding, "web_url">[]): string {
  const names = new Set(rows.map((r) => (/github\.com/i.test(r.web_url) ? "GitHub" : "GitLab")));
  if (names.size === 0) return "GitLab";
  return [...names].sort((a, b) => b.localeCompare(a)).join(" and ");
}

/** "Post 7 notes to 3 merge requests on GitLab?" */
export function postConfirmText(n: number, mrs: number, forge = "GitLab"): string {
  return `Post ${plural(n, "note")} to ${plural(mrs, "merge request")} on ${forge}?`;
}

/** "Post all approved (7 across 3 MRs)". */
export function postAllLabel(s: PostSet): string {
  return `Post all approved (${s.ids.length} across ${plural(s.mrs, "MR")})`;
}

/** "6 findings · 3 merge requests". */
export function queueMeta(rows: PendingFinding[]): string {
  const mrs = new Set(rows.map((r) => r.mr_id)).size;
  return `${plural(rows.length, "finding")} · ${plural(mrs, "merge request")}`;
}

export type LabeledReport = { runId: number; label: string; report: PostReport };

/**
 * The backend groups the posted ids by run and returns one report per run in ascending run id
 * order; pair each report with its MR for display.
 */
export function labelReports(ids: number[], rows: PendingFinding[], reports: PostReport[]): LabeledReport[] {
  const byId = new Map(rows.map((r) => [r.finding.id, r]));
  const runs = [...new Set(ids.map((id) => byId.get(id)?.run_id).filter((x): x is number => x != null))].sort((a, b) => a - b);
  return reports.map((report, i) => {
    const runId = runs[i] ?? -1;
    const row = rows.find((r) => r.run_id === runId);
    return { runId, label: row ? `!${row.mr_iid} ${row.mr_title}` : `Run ${runId}`, report };
  });
}

/** "2 posted · 1 as file note · 1 failed". */
export function reportLine(r: PostReport): string {
  const parts = [`${r.posted} posted`];
  if (r.fallback) parts.push(`${r.fallback} as file ${r.fallback === 1 ? "note" : "notes"}`);
  if (r.failed) parts.push(`${r.failed} failed`);
  if (r.skipped) parts.push(`${r.skipped} skipped`);
  return parts.join(" · ");
}

export function reportTotals(list: LabeledReport[]): PostReport {
  return list.reduce(
    (a, { report: r }) => ({ posted: a.posted + r.posted, fallback: a.fallback + r.fallback, failed: a.failed + r.failed, skipped: a.skipped + r.skipped }),
    { posted: 0, fallback: 0, failed: 0, skipped: 0 },
  );
}

// ---- keyboard --------------------------------------------------------------------------------

export type QueueKey = "up" | "down" | "toggle" | "approve" | "dismiss" | "edit" | "preview" | "selectAll" | "escape";

/**
 * Map a keydown to a queue action. Keys typed into fields are ignored; ⌘A / Ctrl+A is the only
 * modified key handled. Space on a focused button or checkbox is left to the control.
 */
export function queueKeyAction(e: {
  key: string;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
  editable?: boolean;
  onControl?: boolean;
}): QueueKey | null {
  if (e.editable) return null;
  if (e.metaKey || e.ctrlKey) return !e.altKey && (e.key === "a" || e.key === "A") ? "selectAll" : null;
  if (e.altKey) return null;
  switch (e.key) {
    case "ArrowUp":
    case "k":
    case "K":
      return "up";
    case "ArrowDown":
    case "j":
    case "J":
      return "down";
    case " ":
      return e.onControl ? null : "toggle";
    case "x":
    case "X":
      return "toggle";
    case "a":
    case "A":
      return "approve";
    case "d":
    case "D":
      return "dismiss";
    case "e":
    case "E":
      return "edit";
    case "p":
    case "P":
      return "preview";
    case "Escape":
      return "escape";
    default:
      return null;
  }
}

/** Fields and menus own their keys. */
export function isEditableTarget(t: EventTarget | null): boolean {
  if (!t || typeof (t as HTMLElement).tagName !== "string") return false;
  const el = t as HTMLElement;
  const tag = el.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if (el.isContentEditable) return true;
  const role = el.getAttribute?.("role");
  return role === "combobox" || role === "listbox" || role === "option" || role === "menu" || role === "menuitem" || role === "textbox";
}

/** Buttons, links and checkboxes handle Space themselves. */
export function isControlTarget(t: EventTarget | null): boolean {
  if (!t || typeof (t as HTMLElement).tagName !== "string") return false;
  const el = t as HTMLElement;
  if (el.tagName === "BUTTON" || el.tagName === "A") return true;
  const role = el.getAttribute?.("role");
  return role === "button" || role === "checkbox" || role === "radio" || role === "switch" || role === "tab";
}
