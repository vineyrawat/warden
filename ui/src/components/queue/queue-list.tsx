// The dense Approval queue table (mockup 5A `.tbl`): collapsible group rows, one row per finding
// with checkbox, severity, title, location, rule · source · confidence and state.
import { Fragment } from "react";
import { ArrowRight, ChevronRight } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import type { PendingFinding } from "@/bindings/PendingFinding";
import { cn } from "@/lib/utils";
import { SevChip } from "./bits";
import { groupChecked, location, plural, shortLocation, sourceLabel, type QueueGroup } from "./queue-logic";

export const rowDomId = (id: number) => `queue-row-${id}`;

export type RowClick = { shift: boolean; toggle: boolean };

export function QueueTable({
  groups,
  collapsed,
  onToggleCollapse,
  cursorId,
  selected,
  onRowClick,
  onCheck,
  onGroupCheck,
  onOpenReview,
  onUndoApprove,
  editingId,
  renderEditor,
  compact,
}: {
  groups: QueueGroup[];
  collapsed: ReadonlySet<string>;
  onToggleCollapse: (key: string) => void;
  cursorId: number | null;
  selected: ReadonlySet<number>;
  onRowClick: (id: number, how: RowClick) => void;
  onCheck: (id: number, shift: boolean) => void;
  onGroupCheck: (ids: number[]) => void;
  onOpenReview: (runId: number, findingId?: number) => void;
  onUndoApprove: (row: PendingFinding) => void;
  /** Finding being edited inline in the list (null when editing happens in the preview). */
  editingId: number | null;
  renderEditor: (row: PendingFinding) => React.ReactNode;
  /** Preview pane open: drop the rule column and shorten paths. */
  compact: boolean;
}) {
  const cols = compact ? 4 : 5;
  return (
    <table className="w-full table-fixed border-collapse text-xs" aria-label="Pending findings">
      <colgroup>
        <col className="w-8" />
        <col />
        <col className={compact ? "w-[132px]" : "w-[190px]"} />
        {!compact && <col className="w-[180px]" />}
        <col className="w-[96px]" />
      </colgroup>
      <thead className="sr-only">
        <tr>
          <th>Selected</th>
          <th>Finding</th>
          <th>Where</th>
          {!compact && <th>Rule</th>}
          <th>State</th>
        </tr>
      </thead>
      <tbody>
        {groups.map((g) => {
          const ids = g.rows.map((r) => r.finding.id);
          const isCollapsed = collapsed.has(g.key);
          const checked = groupChecked(selected, ids);
          return (
            <Fragment key={g.key}>
              <tr>
                <td className="sticky top-0 z-[1] border-b bg-surface-2 py-1.5 pl-3">
                  <Checkbox
                    checked={checked}
                    aria-label={`Select all in ${g.label}`}
                    onClick={(e) => {
                      e.preventDefault();
                      onGroupCheck(ids);
                    }}
                  />
                </td>
                <td colSpan={cols - 1} className="sticky top-0 z-[1] border-b bg-surface-2 py-1 pr-3">
                  <div className="flex min-w-0 items-center gap-2">
                    <button
                      type="button"
                      onClick={() => onToggleCollapse(g.key)}
                      aria-expanded={!isCollapsed}
                      aria-label={`${isCollapsed ? "Expand" : "Collapse"} ${g.label}`}
                      className="flex min-w-0 items-center gap-1.5 text-left hover:text-foreground"
                    >
                      <ChevronRight className={cn("size-3 shrink-0 text-muted-foreground transition-transform", !isCollapsed && "rotate-90")} aria-hidden />
                      {g.mr ? (
                        <>
                          <span className="shrink-0 font-mono text-muted-foreground">!{g.mr.iid}</span>
                          <b className="min-w-0 truncate font-semibold">{g.mr.title}</b>
                          <span className="shrink-0 text-muted-foreground">· {g.mr.project}</span>
                        </>
                      ) : (
                        g.severity && <SevChip severity={g.severity} />
                      )}
                    </button>
                    <Badge tone={g.mr ? "orange" : "grey"} className="shrink-0">
                      {g.mr ? `${g.rows.length} pending` : plural(g.rows.length, "finding")}
                    </Badge>
                    <span className="flex-1" />
                    {g.mr && (
                      <Button size="sm" variant="ghost" className="shrink-0" onClick={() => onOpenReview(g.mr!.runId)}>
                        Open review <ArrowRight />
                      </Button>
                    )}
                  </div>
                </td>
              </tr>
              {!isCollapsed &&
                g.rows.map((r) => (
                  <QueueRow
                    key={r.finding.id}
                    row={r}
                    cols={cols}
                    compact={compact}
                    showMr={!g.mr}
                    isCursor={r.finding.id === cursorId}
                    isSelected={selected.has(r.finding.id)}
                    onClick={(how) => onRowClick(r.finding.id, how)}
                    onCheck={(shift) => onCheck(r.finding.id, shift)}
                    onUndoApprove={() => onUndoApprove(r)}
                    editor={editingId === r.finding.id ? renderEditor(r) : null}
                  />
                ))}
            </Fragment>
          );
        })}
      </tbody>
    </table>
  );
}

function QueueRow({
  row,
  cols,
  compact,
  showMr,
  isCursor,
  isSelected,
  onClick,
  onCheck,
  onUndoApprove,
  editor,
}: {
  row: PendingFinding;
  cols: number;
  compact: boolean;
  showMr: boolean;
  isCursor: boolean;
  isSelected: boolean;
  onClick: (how: RowClick) => void;
  onCheck: (shift: boolean) => void;
  onUndoApprove: () => void;
  editor: React.ReactNode;
}) {
  const f = row.finding;
  const meta = [f.rule_id ?? f.category, sourceLabel(f.source), f.confidence.toFixed(2)].join(" · ");
  return (
    <>
      <tr
        id={rowDomId(f.id)}
        aria-selected={isSelected}
        aria-current={isCursor ? "true" : undefined}
        onMouseDown={(e) => {
          if (e.shiftKey) e.preventDefault(); // no text selection on shift+click
        }}
        onClick={(e) => onClick({ shift: e.shiftKey, toggle: e.metaKey || e.ctrlKey })}
        className={cn(
          "cursor-default border-b",
          isSelected ? "bg-brand-soft/60" : "hover:bg-accent/60",
          isCursor && "shadow-[inset_2px_0_0_hsl(var(--brand))]",
          isCursor && !isSelected && "bg-accent/70",
        )}
      >
        <td className="py-[7px] pl-3 align-middle">
          <Checkbox
            checked={isSelected}
            aria-label={`Select ${f.title}`}
            onClick={(e) => {
              e.preventDefault();
              e.stopPropagation();
              onCheck(e.shiftKey);
            }}
          />
        </td>
        <td className="py-[7px] pr-3">
          <div className="flex min-w-0 items-center gap-2">
            <SevChip severity={f.severity} className="shrink-0" />
            <span className="min-w-0 truncate" title={f.title}>
              {f.title}
            </span>
            {showMr && <span className="shrink-0 font-mono text-2xs text-muted-foreground">!{row.mr_iid}</span>}
          </div>
          {compact && <div className="truncate pl-0.5 pt-0.5 font-mono text-[10.5px] text-muted-foreground">{meta}</div>}
        </td>
        <td className="truncate py-[7px] pr-3 font-mono text-2xs text-muted-foreground" title={location(f)}>
          {compact ? shortLocation(f) : location(f)}
        </td>
        {!compact && (
          <td className="truncate py-[7px] pr-3 font-mono text-2xs text-muted-foreground" title={meta}>
            {meta}
          </td>
        )}
        <td className="py-[7px] pr-3">
          {f.status === "approved" ? (
            <button
              type="button"
              title="Undo approval"
              aria-label={`Approved. Undo approval of ${f.title}`}
              onClick={(e) => {
                e.stopPropagation();
                onUndoApprove();
              }}
              className="rounded-full focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <Badge tone="blue">Approved</Badge>
            </button>
          ) : (
            <Badge tone="grey">Undecided</Badge>
          )}
        </td>
      </tr>
      {editor && (
        <tr className="border-b bg-surface-2/50">
          <td />
          <td colSpan={cols - 1} className="py-2.5 pr-3">
            {editor}
          </td>
        </tr>
      )}
    </>
  );
}
