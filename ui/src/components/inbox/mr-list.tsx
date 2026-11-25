// Left column of the Inbox: filter box + grouped, compact MR rows (UX 3C list with 3A groups).
import { forwardRef, useEffect, useRef } from "react";
import { ChevronRight, Search } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import type { MrRow } from "@/bindings/MrRow";
import { isRunActive, runStatusLabel } from "@/lib/format";
import { useRun, useRunProgress } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { Dot, SevChip } from "./bits";
import { ciDot, groupOf, pendingFindings, severityCounts, shortRelative, type GroupKey, type InboxGroup } from "./inbox-logic";

export const MrFilterInput = forwardRef<HTMLInputElement, { value: string; onChange: (v: string) => void }>(function MrFilterInput({ value, onChange }, ref) {
  return (
    <label className="flex h-7 items-center gap-2 rounded-[5px] border bg-surface-2 px-2 text-xs focus-within:ring-2 focus-within:ring-ring">
      <Search className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
      <input
        ref={ref}
        data-inbox-filter
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape" && value) {
            e.preventDefault();
            onChange("");
          }
        }}
        placeholder="Filter MRs"
        aria-label="Filter merge requests"
        className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground"
      />
    </label>
  );
});

export function MrList({
  groups,
  collapsed,
  onToggleGroup,
  selectedId,
  onSelect,
  onOpen,
  maxTurns,
}: {
  groups: InboxGroup[];
  collapsed: ReadonlySet<GroupKey>;
  onToggleGroup: (k: GroupKey) => void;
  selectedId: number | null;
  onSelect: (id: number) => void;
  onOpen: (row: MrRow) => void;
  maxTurns: number;
}) {
  return (
    <div role="listbox" aria-label="Merge requests" aria-activedescendant={selectedId != null ? `inbox-mr-${selectedId}` : undefined}>
      {groups.map((g) => {
        const isCollapsed = collapsed.has(g.key);
        return (
          <div key={g.key} role="group" aria-label={g.label}>
            <button
              type="button"
              onClick={() => onToggleGroup(g.key)}
              aria-expanded={!isCollapsed}
              className="sticky top-0 z-[1] flex w-full items-center gap-1.5 border-b bg-surface-2 px-3 py-1.5 text-left text-2xs font-semibold uppercase tracking-[.04em] text-muted-foreground hover:text-foreground"
            >
              <ChevronRight className={cn("size-3 transition-transform", !isCollapsed && "rotate-90")} aria-hidden />
              {g.label}
              <span className="font-mono font-medium">· {g.rows.length}</span>
            </button>
            {!isCollapsed &&
              g.rows.map((r) => (
                <MrListRow key={r.id} row={r} selected={r.id === selectedId} onSelect={() => onSelect(r.id)} onOpen={() => onOpen(r)} maxTurns={maxTurns} />
              ))}
          </div>
        );
      })}
    </div>
  );
}

function MrListRow({ row, selected, onSelect, onOpen, maxTurns }: { row: MrRow; selected: boolean; onSelect: () => void; onOpen: () => void; maxTurns: number }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (selected) ref.current?.scrollIntoView({ block: "nearest" });
  }, [selected]);
  const ci = ciDot(row.ci_summary);
  return (
    <div
      ref={ref}
      id={`inbox-mr-${row.id}`}
      role="option"
      aria-selected={selected}
      tabIndex={-1}
      onClick={onSelect}
      onDoubleClick={onOpen}
      className={cn(
        "grid cursor-default gap-[3px] border-b px-3 py-[9px] text-[12.5px]",
        selected ? "bg-brand-soft shadow-[inset_3px_0_0_hsl(var(--brand))]" : "hover:bg-surface-2/60",
      )}
    >
      <div className="flex items-center gap-1.5">
        <span className="font-mono text-xs text-muted-foreground">{row.iid ? `!${row.iid}` : "local"}</span>
        {row.draft && <Badge tone="grey">draft</Badge>}
        <span className="flex-1" />
        <RowStatus row={row} maxTurns={maxTurns} />
        <Dot tone={ci.tone} label={ci.label} className="ml-0.5" />
      </div>
      <b className="truncate font-semibold">{row.title}</b>
      <span className="truncate font-mono text-2xs text-muted-foreground">
        {row.project_name} · {row.author} · {shortRelative(row.latest_run?.started_at ?? row.updated_at)}
      </span>
    </div>
  );
}

/** Right side of a row's first line: severity chips, live turn, or a short status pill. */
function RowStatus({ row, maxTurns }: { row: MrRow; maxTurns: number }) {
  const run = row.latest_run;
  const group = groupOf(row);
  const wantsDetail = !!run && !isRunActive(run.status) && row.finding_counts.draft > 0;
  const detail = useRun(wantsDetail ? run!.id : null).data;
  const live = useRunProgress(run && isRunActive(run.status) ? run.id : null);

  if (!run) return <Badge tone="grey">{row.draft ? "draft" : "new"}</Badge>;
  if (group === "reviewing") {
    if (run.status === "reviewing") return <Badge tone="blue">{`${live?.turn ?? run.turns}/${maxTurns}`}</Badge>;
    return <Badge tone="blue">{runStatusLabel(run.status).toLowerCase()}</Badge>;
  }
  if (run.status === "failed") return <Badge tone="red">failed</Badge>;
  if (wantsDetail) {
    if (!detail) return <Badge tone="orange">{row.finding_counts.draft}</Badge>;
    return (
      <span className="flex items-center gap-1">
        {severityCounts(pendingFindings(detail.findings)).map((c) => (
          <SevChip key={c.severity} severity={c.severity} count={c.count} />
        ))}
      </span>
    );
  }
  if (group === "done") return <Badge tone="green">{row.finding_counts.posted > 0 ? `${row.finding_counts.posted} posted` : "clean"}</Badge>;
  return <Badge tone="grey">{run.status === "cancelled" ? "cancelled" : "skipped"}</Badge>;
}
