import { useEffect, useRef, useState } from "react";
import { ChevronDown, Search, X } from "lucide-react";

import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Segmented } from "@/components/segmented";
import { cn } from "@/lib/utils";
import { DATE_RANGES, DEFAULT_FILTERS, isDefaultFilters, STATUS_OPTIONS, toggleIn, type HistoryFilters, type StatusKey } from "./history-logic";

export type ProjectOption = { id: number; name: string };

export function FilterBar({
  filters,
  onChange,
  projects,
  count,
  total,
  searchRef,
}: {
  filters: HistoryFilters;
  onChange: (f: HistoryFilters) => void;
  projects: ProjectOption[];
  count: number;
  total: number;
  searchRef?: React.Ref<HTMLInputElement>;
}) {
  const set = (patch: Partial<HistoryFilters>) => onChange({ ...filters, ...patch });
  return (
    <div className="flex flex-wrap items-center gap-2">
      <div className="relative w-[230px]">
        <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" aria-hidden />
        <Input
          ref={searchRef}
          value={filters.query}
          onChange={(e) => set({ query: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Escape" && filters.query) {
              e.preventDefault();
              e.stopPropagation();
              set({ query: "" });
            }
          }}
          placeholder="Search title, !iid, author, sha"
          aria-label="Search runs"
          className="pl-7 font-sans"
        />
      </div>
      <Select value={filters.projectId == null ? "all" : String(filters.projectId)} onValueChange={(v) => set({ projectId: v === "all" ? null : Number(v) })}>
        <SelectTrigger className="h-7 w-[160px] rounded-full font-mono text-[11.5px]" aria-label="Project">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="all">All projects</SelectItem>
          {projects.map((p) => (
            <SelectItem key={p.id} value={String(p.id)}>
              {p.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <StatusFilter value={filters.statuses} onChange={(statuses) => set({ statuses })} />
      <Segmented label="Date range" options={DATE_RANGES} value={filters.range} onChange={(range) => set({ range })} />
      <div className="ml-auto flex items-center gap-2 text-xs text-muted-foreground">
        <span aria-live="polite">
          <b className="font-mono font-semibold text-foreground">{count}</b> {count === 1 ? "run" : "runs"}
          {count !== total && <span> of {total}</span>}
        </span>
        {!isDefaultFilters(filters) && (
          <button
            type="button"
            className="inline-flex items-center gap-1 rounded text-primary hover:underline"
            onClick={() => onChange(DEFAULT_FILTERS)}
          >
            <X className="h-3 w-3" aria-hidden />
            Clear filters
          </button>
        )}
      </div>
    </div>
  );
}

function statusSummary(v: StatusKey[]): string {
  if (!v.length) return "Any status";
  if (v.length === 1) return STATUS_OPTIONS.find((o) => o.value === v[0])?.label ?? v[0];
  return `${v.length} statuses`;
}

/** Multi-select status dropdown (chip trigger + checkbox list). */
function StatusFilter({ value, onChange }: { value: StatusKey[]; onChange: (v: StatusKey[]) => void }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, [open]);

  return (
    <div
      ref={root}
      className="relative"
      onKeyDown={(e) => {
        if (e.key === "Escape" && open) {
          e.preventDefault();
          e.stopPropagation();
          setOpen(false);
          trigger.current?.focus();
        }
      }}
    >
      <button
        ref={trigger}
        type="button"
        aria-haspopup="true"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        className={cn(
          "inline-flex h-7 items-center gap-1.5 rounded-full border px-2.5 font-mono text-[11.5px]",
          value.length ? "border-primary bg-brand-soft text-primary" : "bg-surface-2 hover:bg-accent",
        )}
      >
        {statusSummary(value)}
        <ChevronDown className="h-3.5 w-3.5 opacity-60" aria-hidden />
      </button>
      {open && (
        <div role="group" aria-label="Run status" className="absolute left-0 top-full z-30 mt-1 w-[190px] rounded-md border bg-popover p-1 shadow-md">
          {STATUS_OPTIONS.map((o) => {
            const id = `history-status-${o.value}`;
            return (
              <label key={o.value} htmlFor={id} className="flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-xs hover:bg-accent">
                <Checkbox id={id} checked={value.includes(o.value)} onCheckedChange={() => onChange(toggleIn(value, o.value))} />
                {o.label}
              </label>
            );
          })}
          {value.length > 0 && (
            <button type="button" className="mt-1 w-full rounded-sm border-t px-2 py-1.5 text-left text-2xs text-primary hover:bg-accent" onClick={() => onChange([])}>
              Any status
            </button>
          )}
        </div>
      )}
    </div>
  );
}
