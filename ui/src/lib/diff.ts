// Pure helpers for the hand-rolled diff table (unit tested).
import type { DiffLine } from "@/bindings/DiffLine";
import type { FileDiff } from "@/bindings/FileDiff";
import type { Finding } from "@/bindings/Finding";

export type DiffRow =
  | { type: "hunk"; key: string; header: string }
  | { type: "line"; key: string; line: DiffLine; findings: Finding[] };

/**
 * Flatten a file's hunks into table rows, attaching each finding to the first row it anchors to:
 * side "new" → an add/context row with that new_line; side "old" → a del/context row with that old_line.
 * Findings that do not land on any row are returned in `unanchored`.
 */
export function buildDiffRows(file: FileDiff, findings: Finding[]): { rows: DiffRow[]; unanchored: Finding[] } {
  const rows: DiffRow[] = [];
  const placed = new Set<number>();
  file.hunks.forEach((h, hi) => {
    rows.push({ type: "hunk", key: `h${hi}`, header: h.header });
    h.lines.forEach((l, li) => {
      const mine = findings.filter((f) => {
        if (placed.has(f.id)) return false;
        if (f.side === "old") return l.kind !== "add" && l.old_line === f.line;
        return l.kind !== "del" && l.new_line === f.line;
      });
      mine.forEach((f) => placed.add(f.id));
      rows.push({ type: "line", key: `h${hi}l${li}`, line: l, findings: mine });
    });
  });
  return { rows, unanchored: findings.filter((f) => !placed.has(f.id)) };
}

export function lineMarker(kind: DiffLine["kind"]): string {
  return kind === "add" ? "+" : kind === "del" ? "-" : " ";
}
