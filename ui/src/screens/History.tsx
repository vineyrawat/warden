import { useEffect, useMemo, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { EmptyState, ErrorBox, Loading, ScreenHeader } from "@/components/layout";
import { AuditDrawer, DRAWER_DEFAULT, DRAWER_MIN } from "@/components/history/audit-drawer";
import { FilterBar, type ProjectOption } from "@/components/history/filter-bar";
import { KpiStrip } from "@/components/history/kpi-strip";
import { RunsTable } from "@/components/history/runs-table";
import {
  buildRows,
  DEFAULT_FILTERS,
  filterRows,
  isDefaultFilters,
  moveCursor,
  type HistoryFilters,
} from "@/components/history/history-logic";
import { useNav } from "@/lib/nav";
import { useProjects, useRunRows } from "@/lib/queries";

export { summarize } from "@/components/history/summarize";

const PAGE = 50;
const WIDTH_KEY = "warden.history.drawerWidth";

function readWidth(): number {
  try {
    const n = Number(localStorage.getItem(WIDTH_KEY));
    return n >= DRAWER_MIN ? n : DRAWER_DEFAULT;
  } catch {
    return DRAWER_DEFAULT;
  }
}

function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}

const SKIP_KEYS_IN = "input, textarea, select, [contenteditable='true'], [role=dialog], [role=listbox], [role=menu], [role=radiogroup], [role=separator]";

export function HistoryScreen({ initialRunId }: { initialRunId?: number }) {
  const { navigate } = useNav();
  const now = useNow(60_000);
  const runsQ = useRunRows(500);
  const projectsQ = useProjects().data;

  const [filters, setFilters] = useState<HistoryFilters>(DEFAULT_FILTERS);
  const [cursorId, setCursorId] = useState<number | null>(initialRunId ?? null);
  const [openId, setOpenId] = useState<number | null>(initialRunId ?? null);
  const [shown, setShown] = useState(PAGE);
  const [drawerWidth, setDrawerWidth] = useState(readWidth);
  const [regionWidth, setRegionWidth] = useState(1200);
  const regionRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const widenedFor = useRef<number | null>(null);

  const rows = useMemo(() => buildRows(runsQ.data ?? []), [runsQ.data]);
  const runs = useMemo(() => rows.map((r) => r.run), [rows]);
  const projects = useMemo(() => projectsQ ?? [], [projectsQ]);
  const filtered = useMemo(() => filterRows(rows, filters, now), [rows, filters, now]);
  const visible = useMemo(() => filtered.slice(0, shown), [filtered, shown]);
  const ids = useMemo(() => visible.map((r) => r.run.id), [visible]);

  const projectOptions = useMemo<ProjectOption[]>(() => {
    const opts = projects.map((p) => ({ id: p.id, name: p.name }));
    if (rows.some((r) => r.mr?.project_id === 0)) opts.push({ id: 0, name: "local" });
    return opts;
  }, [projects, rows]);

  useEffect(() => setShown(PAGE), [filters]);

  // Deep link (Route { screen: "history", runId }): open it, widening the date range if needed.
  useEffect(() => {
    if (initialRunId == null) return;
    setOpenId(initialRunId);
    setCursorId(initialRunId);
    widenedFor.current = null;
  }, [initialRunId]);
  useEffect(() => {
    if (initialRunId == null || widenedFor.current === initialRunId || !runsQ.data) return;
    widenedFor.current = initialRunId;
    const r = rows.find((x) => x.run.id === initialRunId);
    if (r && !filtered.some((x) => x.run.id === initialRunId)) setFilters({ ...DEFAULT_FILTERS, range: "all" });
  }, [initialRunId, runsQ.data, rows, filtered]);

  useEffect(() => {
    const el = regionRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(([e]) => setRegionWidth(e.contentRect.width));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const setWidth = (w: number) => {
    setDrawerWidth(w);
    try {
      localStorage.setItem(WIDTH_KEY, String(w));
    } catch {
      /* per-viewer convenience only */
    }
  };

  // ↑↓ move, Enter opens, Esc closes, / focuses search.
  const keyState = useRef({ ids, cursorId, openId });
  keyState.current = { ids, cursorId, openId };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return;
      const t = e.target as HTMLElement | null;
      const s = keyState.current;
      if (e.key === "Escape") {
        if (s.openId != null && !t?.closest("[role=dialog], [role=listbox], [role=menu]")) {
          e.preventDefault();
          setOpenId(null);
        }
        return;
      }
      if (t?.closest?.(SKIP_KEYS_IN)) return;
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const next = moveCursor(s.ids, s.cursorId, e.key === "ArrowDown" ? 1 : -1);
        setCursorId(next);
        if (s.openId != null && next != null) setOpenId(next);
      } else if (e.key === "Enter") {
        if (s.cursorId == null || t?.closest("button, a")) return;
        e.preventDefault();
        setOpenId(s.cursorId);
      } else if (e.key === "/") {
        e.preventDefault();
        searchRef.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const open = (runId: number) => {
    setCursorId(runId);
    setOpenId(runId);
  };

  const total = runs.length;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <ScreenHeader
        title="History"
        meta={
          <span>
            Every run, tool call, guardrail decision and post is recorded · <kbd className="font-mono">↑↓</kbd> move · <kbd className="font-mono">Enter</kbd> open ·{" "}
            <kbd className="font-mono">Esc</kbd> close
          </span>
        }
      />
      <div className="shrink-0 space-y-2.5 border-b px-4 pb-2.5 pt-3">
        <KpiStrip runs={runs} />
        <FilterBar filters={filters} onChange={setFilters} projects={projectOptions} count={filtered.length} total={total} searchRef={searchRef} />
      </div>
      <div ref={regionRef} className="relative min-h-0 flex-1">
        <div className="h-full overflow-y-auto">
          {runsQ.isLoading ? (
            <Loading />
          ) : runsQ.error ? (
            <ErrorBox error={runsQ.error} />
          ) : total === 0 ? (
            <EmptyState title="No runs yet">
              Runs appear here once Warden reviews a merge request.{" "}
              <button type="button" className="text-primary hover:underline" onClick={() => navigate({ screen: "inbox" })}>
                Go to the Inbox
              </button>
            </EmptyState>
          ) : filtered.length === 0 ? (
            <EmptyState title="No runs match these filters">
              {!isDefaultFilters(filters) || filters.range !== "all" ? (
                <span className="inline-flex flex-wrap justify-center gap-2">
                  {filters.range !== "all" && (
                    <button type="button" className="text-primary hover:underline" onClick={() => setFilters({ ...filters, range: "all" })}>
                      Search all dates
                    </button>
                  )}
                  {!isDefaultFilters(filters) && (
                    <button type="button" className="text-primary hover:underline" onClick={() => setFilters(DEFAULT_FILTERS)}>
                      Clear filters
                    </button>
                  )}
                </span>
              ) : null}
            </EmptyState>
          ) : (
            <>
              <RunsTable rows={visible} cursorId={cursorId} openId={openId} onOpen={open} />
              {filtered.length > visible.length && (
                <div className="flex items-center justify-center gap-3 py-3 text-xs text-muted-foreground">
                  Showing {visible.length} of {filtered.length}
                  <Button size="sm" onClick={() => setShown((n) => n + PAGE)}>
                    Show {Math.min(PAGE, filtered.length - visible.length)} more
                  </Button>
                </div>
              )}
            </>
          )}
        </div>
        {openId != null && (
          <AuditDrawer
            runId={openId}
            row={rows.find((r) => r.run.id === openId)}
            width={drawerWidth}
            maxWidth={Math.max(DRAWER_MIN, regionWidth - 240)}
            onWidth={setWidth}
            onClose={() => setOpenId(null)}
          />
        )}
      </div>
    </div>
  );
}
