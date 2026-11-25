import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { Badge } from "@/components/ui/badge";
import { FindingCard } from "@/components/finding-card";
import type { FileDiff } from "@/bindings/FileDiff";
import type { Finding } from "@/bindings/Finding";
import type { RuleConfig } from "@/bindings/RuleConfig";
import type { Severity } from "@/bindings/Severity";
import { buildDiffRows, lineMarker } from "@/lib/diff";
import { filePath, fileStats } from "@/lib/format";
import { cn } from "@/lib/utils";
import { stripMarkers, type Decision } from "@/components/review/logic";

const ROW_BG = { add: "bg-diff-add", del: "bg-diff-del", context: "" } as const;

const MARK_BG: Record<Severity, string> = {
  blocker: "bg-red",
  major: "bg-orange",
  minor: "bg-yellow",
  nit: "bg-muted-foreground",
};

type Row = Extract<ReturnType<typeof buildDiffRows>["rows"][number], { type: "line" }>;

/**
 * Hand-rolled unified diff table: old/new line numbers, add/del/context/hunk rows, horizontal scroll
 * for long lines, and finding threads inserted under the anchored line. Threads stay pinned to the
 * visible width of the pane while the code scrolls sideways. A severity strip on the right edge
 * jumps between the file's findings.
 */
export function DiffView({
  file,
  findings,
  focusFindingId,
  onDecide,
  onFocusFinding,
  rules,
  header,
  bottomPad,
}: {
  file: FileDiff;
  findings: Finding[];
  focusFindingId?: number | null;
  onDecide?: (f: Finding, to: Decision) => void;
  /** Called when a strip marker is clicked. */
  onFocusFinding?: (id: number) => void;
  rules?: RuleConfig[];
  /** Replaces the default file header row (path + stats) when given. */
  header?: React.ReactNode;
  /** Leave room at the bottom for a floating bar. */
  bottomPad?: boolean;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  const [paneWidth, setPaneWidth] = useState<number>(0);
  const { rows, unanchored } = useMemo(() => buildDiffRows(file, findings), [file, findings]);
  const marks = useMemo(() => stripMarkers(rows, unanchored), [rows, unanchored]);
  const stats = fileStats(file);
  const path = filePath(file);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setPaneWidth(el.clientWidth));
    ro.observe(el);
    setPaneWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    if (focusFindingId == null) return;
    const el = document.getElementById(`finding-${focusFindingId}`);
    el?.scrollIntoView({ block: "center", behavior: "smooth" });
  }, [focusFindingId, file]);

  const cardStyle = paneWidth ? { width: paneWidth } : undefined;
  const cardProps = { onDecide, rules };

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      {header ?? (
        <div className="flex shrink-0 items-center gap-2.5 border-b bg-surface-2 px-3 py-2 font-mono text-xs">
          <span className="min-w-0 truncate font-semibold">{path}</span>
          {file.old_path !== file.new_path && !file.is_new && !file.is_deleted && <span className="truncate text-muted-foreground">(from {file.old_path})</span>}
          <span className="text-green">+{stats.additions}</span>
          <span className="text-red">−{stats.deletions}</span>
          {file.is_new && <Badge tone="green">new file</Badge>}
          {file.is_deleted && <Badge tone="red">deleted</Badge>}
        </div>
      )}
      <div className="flex min-h-0 flex-1">
        <div ref={scroller} className={cn("min-h-0 min-w-0 flex-1 overflow-auto", bottomPad && "pb-16")} tabIndex={0} aria-label={`Diff of ${path}`}>
          {unanchored.length > 0 && (
            <div className="sticky left-0 space-y-2 border-b bg-surface-2/60 px-3 py-2" style={cardStyle}>
              <div className="text-2xs font-medium uppercase tracking-wide text-muted-foreground">Outside the visible diff</div>
              {unanchored.map((f) => (
                <FindingCard key={f.id} id={`finding-${f.id}`} finding={f} showLocation className={cn(focusFindingId === f.id && "ring-2 ring-ring")} {...cardProps} />
              ))}
            </div>
          )}
          {file.is_binary ? (
            <div className="px-4 py-6 text-xs text-muted-foreground">Binary file not shown.</div>
          ) : file.hunks.length === 0 ? (
            <div className="px-4 py-6 text-xs text-muted-foreground">No textual changes in this run's diff.</div>
          ) : (
            <table className="min-w-full border-collapse font-mono text-xs leading-[1.7]">
              <colgroup>
                <col className="w-10" />
                <col className="w-10" />
                <col />
              </colgroup>
              <tbody>
                {rows.map((r) =>
                  r.type === "hunk" ? (
                    <tr key={r.key} className="bg-brand-soft text-muted-foreground">
                      <td className="select-none px-2" />
                      <td className="select-none px-2" />
                      <td className="whitespace-pre px-2">{r.header}</td>
                    </tr>
                  ) : (
                    <DiffLineRows key={r.key} row={r} cardStyle={cardStyle} focusFindingId={focusFindingId} cardProps={cardProps} />
                  ),
                )}
              </tbody>
            </table>
          )}
        </div>
        {marks.length > 0 && (
          <div className="relative w-2.5 shrink-0 border-l bg-surface-2" aria-label="Findings in this file">
            {marks.map((m) => (
              <button
                key={m.id}
                type="button"
                title={`Jump to ${m.severity} finding`}
                aria-label={`Jump to ${m.severity} finding`}
                onClick={() => {
                  onFocusFinding?.(m.id);
                  document.getElementById(`finding-${m.id}`)?.scrollIntoView({ block: "center", behavior: "smooth" });
                }}
                className={cn(
                  "absolute left-0.5 h-1.5 w-1.5 rounded-sm",
                  MARK_BG[m.severity],
                  (m.status === "dismissed" || m.status === "rejected") && "opacity-40",
                  focusFindingId === m.id && "ring-2 ring-ring",
                )}
                style={{ top: `calc(${m.pct}% * 0.96 + 3px)` }}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function DiffLineRows({
  row,
  cardStyle,
  focusFindingId,
  cardProps,
}: {
  row: Row;
  cardStyle?: React.CSSProperties;
  focusFindingId?: number | null;
  cardProps: { onDecide?: (f: Finding, to: Decision) => void; rules?: RuleConfig[] };
}) {
  const l = row.line;
  const bg = ROW_BG[l.kind];
  const hl = row.findings.some((f) => f.id === focusFindingId);
  return (
    <>
      <tr className={cn(bg, hl && "shadow-[inset_3px_0_0_hsl(var(--brand))]")} data-kind={l.kind}>
        <td className="select-none px-2 text-right text-muted-foreground">{l.old_line ?? ""}</td>
        <td className="select-none px-2 text-right text-muted-foreground">{l.new_line ?? ""}</td>
        <td className="whitespace-pre px-2">
          <span className="select-none">{lineMarker(l.kind)}</span>
          {l.text}
        </td>
      </tr>
      {row.findings.map((f) => (
        <tr key={f.id}>
          <td colSpan={3} className="p-0 font-sans">
            <div className="sticky left-0 py-1 pl-12 pr-3 xl:pl-[84px]" style={cardStyle}>
              <FindingCard id={`finding-${f.id}`} finding={f} before={l.text} className={cn(focusFindingId === f.id && "ring-2 ring-ring")} {...cardProps} />
            </div>
          </td>
        </tr>
      ))}
    </>
  );
}
