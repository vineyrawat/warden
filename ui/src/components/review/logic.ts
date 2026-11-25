// Pure helpers for the Review screen (unit tested in logic.test.ts).
import type { BadgeTone } from "@/components/ui/badge";
import type { CiJob } from "@/bindings/CiJob";
import type { DiffLine } from "@/bindings/DiffLine";
import type { FileDiff } from "@/bindings/FileDiff";
import type { Finding } from "@/bindings/Finding";
import type { FindingStatus } from "@/bindings/FindingStatus";
import type { RuleConfig } from "@/bindings/RuleConfig";
import type { Severity } from "@/bindings/Severity";
import type { DiffRow } from "@/lib/diff";
import { filePath, ruleKind, sortFindings } from "@/lib/format";

export type ReviewTab = "findings" | "diff" | "ci" | "summary";
export const REVIEW_TABS: ReviewTab[] = ["findings", "diff", "ci", "summary"];

/** The three states a reviewer moves a finding between. Post sends the approved ones. */
export type Decision = "approved" | "dismissed" | "draft";

export type FindingLike = Pick<Finding, "id" | "status" | "severity" | "confidence" | "file" | "line">;

/** Draft, approved and dismissed findings are the reviewer's to decide; posted/resolved are final. */
export function isDecidable(status: FindingStatus): boolean {
  return status === "draft" || status === "approved" || status === "dismissed";
}

export function isFinal(status: FindingStatus): boolean {
  return status === "posted" || status === "resolved";
}

// ---- ordering + navigation -------------------------------------------------------------------

/** Findings the walkthrough steps through: everything guardrails kept, most severe first. */
export function walkthroughFindings<T extends FindingLike>(findings: T[]): T[] {
  return sortFindings(findings.filter((f) => f.status !== "rejected"));
}

/** Findings guardrails dropped (status `rejected`), most severe first. */
export function droppedFindings<T extends FindingLike>(findings: T[]): T[] {
  return sortFindings(findings.filter((f) => f.status === "rejected"));
}

export type Progress = { total: number; approved: number; dismissed: number; undecided: number; posted: number };

export function progressCounts(list: Pick<Finding, "status">[]): Progress {
  const p: Progress = { total: list.length, approved: 0, dismissed: 0, undecided: 0, posted: 0 };
  for (const f of list) {
    if (f.status === "approved") p.approved++;
    else if (f.status === "dismissed") p.dismissed++;
    else if (f.status === "draft") p.undecided++;
    else if (isFinal(f.status)) p.posted++;
  }
  return p;
}

/** "Finding 3 of 5 · 2 approved · 0 dismissed" (+ " · 1 posted" when some are already on the forge). */
export function progressLine(index: number, p: Progress): string {
  const parts = [`Finding ${Math.min(index + 1, p.total)} of ${p.total}`, `${p.approved} approved`, `${p.dismissed} dismissed`];
  if (p.posted) parts.push(`${p.posted} posted`);
  return parts.join(" · ");
}

/**
 * Next undecided finding after `currentId`, wrapping around; the current one is never returned.
 * With `currentId` null or unknown the search starts at the top.
 */
export function nextUndecidedId(list: Pick<Finding, "id" | "status">[], currentId: number | null, dir: 1 | -1 = 1): number | null {
  const n = list.length;
  if (n === 0) return null;
  const at = currentId == null ? -1 : list.findIndex((f) => f.id === currentId);
  for (let step = 1; step <= n; step++) {
    const i = at < 0 ? (dir === 1 ? step - 1 : n - step) : (((at + dir * step) % n) + n) % n;
    if (i === at) continue;
    if (list[i].status === "draft") return list[i].id;
  }
  return null;
}

/** The card to open first: the requested finding if present, else the first undecided, else the first. */
export function initialFindingId(list: Pick<Finding, "id" | "status">[], focusId?: number | null): number | null {
  if (focusId != null && list.some((f) => f.id === focusId)) return focusId;
  return list.find((f) => f.status === "draft")?.id ?? list[0]?.id ?? null;
}

/** Previous / next finding by position, clamped to the ends. */
export function stepId(list: Pick<Finding, "id">[], currentId: number | null, delta: number): number | null {
  if (list.length === 0) return null;
  const at = list.findIndex((f) => f.id === currentId);
  if (at < 0) return list[0].id;
  const i = Math.max(0, Math.min(list.length - 1, at + delta));
  return list[i].id;
}

/** Where to go after deciding `decidedId`: the next undecided one, or stay put when none remain. */
export function afterDecision(list: Pick<Finding, "id" | "status">[], decidedId: number): number {
  return nextUndecidedId(list, decidedId) ?? decidedId;
}

/**
 * Which finding U reverts: the current one when it is approved/dismissed, otherwise the most recent
 * decision in `history` (newest last) that is still approved/dismissed.
 */
export function undoTarget(list: Pick<Finding, "id" | "status">[], currentId: number | null, history: number[]): number | null {
  const byId = new Map(list.map((f) => [f.id, f]));
  const cur = currentId == null ? undefined : byId.get(currentId);
  if (cur && (cur.status === "approved" || cur.status === "dismissed")) return cur.id;
  for (let i = history.length - 1; i >= 0; i--) {
    const f = byId.get(history[i]);
    if (f && (f.status === "approved" || f.status === "dismissed")) return f.id;
  }
  return null;
}

/** Apply local (optimistic) status overrides on top of server data. */
export function applyOverrides<T extends Pick<Finding, "id" | "status">>(findings: T[], overrides: ReadonlyMap<number, FindingStatus>): T[] {
  if (overrides.size === 0) return findings;
  return findings.map((f) => {
    const s = overrides.get(f.id);
    return s && s !== f.status && !isFinal(f.status) ? { ...f, status: s } : f;
  });
}

/** Drop overrides the server has caught up with (or that no longer apply). Returns the same map when unchanged. */
export function pruneOverrides(findings: Pick<Finding, "id" | "status">[], overrides: Map<number, FindingStatus>): Map<number, FindingStatus> {
  if (overrides.size === 0) return overrides;
  const byId = new Map(findings.map((f) => [f.id, f.status]));
  let changed = false;
  const next = new Map(overrides);
  for (const [id, s] of overrides) {
    const server = byId.get(id);
    if (server === undefined || server === s || isFinal(server)) {
      next.delete(id);
      changed = true;
    }
  }
  return changed ? next : overrides;
}

// ---- keyboard --------------------------------------------------------------------------------

export type ReviewKey = "approve" | "edit" | "dismiss" | "prev" | "next" | "undo";

/** Map a keydown to a walkthrough action. Ignores modified keys and keys typed into form fields. */
export function reviewKeyAction(e: { key: string; metaKey?: boolean; ctrlKey?: boolean; altKey?: boolean; editable?: boolean }): ReviewKey | null {
  if (e.editable || e.metaKey || e.ctrlKey || e.altKey) return null;
  switch (e.key) {
    case "a":
    case "A":
      return "approve";
    case "e":
    case "E":
      return "edit";
    case "d":
    case "D":
      return "dismiss";
    case "u":
    case "U":
      return "undo";
    case "ArrowLeft":
    case "k":
    case "K":
      return "prev";
    case "ArrowRight":
    case "j":
    case "J":
      return "next";
    default:
      return null;
  }
}

/** Whether a keydown happened inside something the user types into or a menu that owns the keys. */
export function isEditableTarget(t: EventTarget | null): boolean {
  if (!t || typeof (t as HTMLElement).tagName !== "string") return false;
  const el = t as HTMLElement;
  const tag = el.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if (el.isContentEditable) return true;
  const role = el.getAttribute?.("role");
  return role === "combobox" || role === "listbox" || role === "option" || role === "menu" || role === "menuitem" || role === "textbox";
}

// ---- code context ----------------------------------------------------------------------------

export function findFile(files: FileDiff[], path: string): FileDiff | undefined {
  return files.find((f) => filePath(f) === path || f.new_path === path || f.old_path === path);
}

function anchors(l: DiffLine, f: Pick<Finding, "line" | "side">): boolean {
  if (f.side === "old") return l.kind !== "add" && l.old_line === f.line;
  return l.kind !== "del" && l.new_line === f.line;
}

export type CodeContext = { header: string; lines: DiffLine[]; anchor: number };

/**
 * ±`radius` diff rows around the finding's line, taken from the hunk that contains it.
 * Returns null when the line is not in the diff (binary file, file not changed, line outside hunks).
 */
export function codeContext(file: FileDiff | undefined, f: Pick<Finding, "line" | "side">, radius = 6): CodeContext | null {
  if (!file || file.is_binary) return null;
  for (const h of file.hunks) {
    const i = h.lines.findIndex((l) => anchors(l, f));
    if (i < 0) continue;
    const start = Math.max(0, i - radius);
    const end = Math.min(h.lines.length, i + radius + 1);
    return { header: h.header, lines: h.lines.slice(start, end), anchor: i - start };
  }
  return null;
}

/** The anchored line's text, used as the "before" side of a suggestion. */
export function anchorText(file: FileDiff | undefined, f: Pick<Finding, "line" | "side">): string | null {
  const ctx = codeContext(file, f, 0);
  return ctx ? ctx.lines[ctx.anchor].text : null;
}

export type SuggestionLine = { kind: "del" | "add"; text: string };

/** A suggestion replaces the commented line (GitLab ```suggestion semantics): − old line, + new lines. */
export function suggestionDiff(before: string | null, suggestion: string): SuggestionLine[] {
  const out: SuggestionLine[] = [];
  if (before != null) out.push({ kind: "del", text: before });
  const body = suggestion.replace(/\n+$/, "");
  for (const t of body.split("\n")) out.push({ kind: "add", text: t });
  return out;
}

// ---- diff tab --------------------------------------------------------------------------------

/** Diff files plus empty entries for findings on files outside the diff, in diff order. */
export function reviewFiles(files: FileDiff[], findings: Pick<Finding, "file">[]): FileDiff[] {
  const out = [...files];
  const known = new Set(files.map(filePath));
  for (const f of findings) {
    if (known.has(f.file)) continue;
    known.add(f.file);
    out.push({ old_path: f.file, new_path: f.file, is_new: false, is_deleted: false, is_binary: false, hunks: [] });
  }
  return out;
}

/** Findings ordered the way they appear in the Diff tab: by file (diff order), then line. */
export function diffOrder<T extends Pick<Finding, "id" | "file" | "line">>(files: FileDiff[], findings: T[]): T[] {
  const idx = new Map(files.map((f, i) => [filePath(f), i]));
  const at = (p: string) => idx.get(p) ?? Number.MAX_SAFE_INTEGER;
  return [...findings].sort((a, b) => at(a.file) - at(b.file) || a.file.localeCompare(b.file) || a.line - b.line || a.id - b.id);
}

export type StripMarker = { id: number; severity: Severity; status: FindingStatus; pct: number };

/** Severity strip markers: each finding's row position as a percentage of the file's rows. */
export function stripMarkers(rows: DiffRow[], unanchored: Pick<Finding, "id" | "severity" | "status">[]): StripMarker[] {
  const out: StripMarker[] = unanchored.map((f) => ({ id: f.id, severity: f.severity, status: f.status, pct: 0 }));
  const n = Math.max(rows.length, 1);
  rows.forEach((r, i) => {
    if (r.type !== "line") return;
    for (const f of r.findings) out.push({ id: f.id, severity: f.severity, status: f.status, pct: Math.round((i / n) * 1000) / 10 });
  });
  return out;
}

// ---- labels ----------------------------------------------------------------------------------

/** "regex" / "semgrep" / "Claude" / "CI" for the card meta line. */
export function sourceTag(f: Pick<Finding, "source" | "rule_id">, rules?: RuleConfig[]): string {
  if (f.source === "ci") return "CI";
  if (f.source === "rule") {
    const r = f.rule_id ? rules?.find((x) => x.id === f.rule_id) : undefined;
    return r ? ruleKind(r) : "rule";
  }
  return "Claude";
}

export function verdictInfo(verdict: string | null | undefined): { label: string; tone: BadgeTone } | null {
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

/** Tab badge for CI: "1 failed", "running", "3 passed", "none". */
export function ciTabLabel(jobs: Pick<CiJob, "status">[]): { label: string; tone: BadgeTone } {
  if (jobs.length === 0) return { label: "none", tone: "grey" };
  const failed = jobs.filter((j) => j.status === "failed" || j.status === "timed_out").length;
  if (failed) return { label: `${failed} failed`, tone: "red" };
  if (jobs.some((j) => j.status === "running" || j.status === "pending")) return { label: "running", tone: "blue" };
  const passed = jobs.filter((j) => j.status === "passed").length;
  return passed ? { label: `${passed} passed`, tone: "green" } : { label: `${jobs.length} skipped`, tone: "grey" };
}

const WORDS = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten"];
const word = (n: number) => WORDS[n] ?? String(n);

type DropKind = "duplicate" | "confidence" | "nit" | "floor" | "budget" | "anchor" | "other";

export function dropKind(reason: string | null | undefined): DropKind {
  const r = (reason ?? "").toLowerCase();
  if (/duplicate/.test(r)) return "duplicate";
  if (/conf/.test(r)) return "confidence";
  if (/\bnits?\b/.test(r)) return "nit";
  if (/floor/.test(r)) return "floor";
  if (/budget/.test(r)) return "budget";
  if (/not in diff|outside the diff|context line|not a deleted line/.test(r)) return "anchor";
  return "other";
}

const DROP_PHRASE: Record<DropKind, [string, string]> = {
  duplicate: ["duplicate of an open thread", "duplicates of open threads"],
  confidence: ["below the confidence threshold", "below the confidence threshold"],
  nit: ["nit (nits are off)", "nits (nits are off)"],
  floor: ["below the severity floor", "below the severity floor"],
  budget: ["over the note budget", "over the note budget"],
  anchor: ["not on a changed line", "not on a changed line"],
  other: ["failed a quality check", "failed a quality check"],
};

/** "Guardrails dropped 2: one duplicate of an open thread, one below the confidence threshold." */
export function droppedSummary(dropped: Pick<Finding, "guardrail_reason">[]): string {
  if (dropped.length === 0) return "Guardrails dropped nothing.";
  const counts = new Map<DropKind, number>();
  for (const f of dropped) {
    const k = dropKind(f.guardrail_reason);
    counts.set(k, (counts.get(k) ?? 0) + 1);
  }
  const parts = [...counts].map(([k, n]) => `${word(n)} ${DROP_PHRASE[k][n === 1 ? 0 : 1]}`);
  return `Guardrails dropped ${dropped.length}: ${parts.join(", ")}.`;
}

/** Why the Post button is disabled, or null when posting is allowed. */
export function postDisabledReason(o: { isLocal: boolean; paused: boolean; approved: number; active: boolean; forgeName: string }): string | null {
  if (o.isLocal) return "Local review: there is no merge request to post to";
  if (o.paused) return "Posting is paused. Resume it in Settings → Advanced";
  if (o.active) return "Wait for the review to finish";
  if (o.approved === 0) return "Approve at least one finding (A) to post it";
  return null;
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}
