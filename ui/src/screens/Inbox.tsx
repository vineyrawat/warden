// Inbox (UX section 3): variant C "List + preview" with A's groups in the left list and B's
// one-click "Approve N & post" in the preview. ↑↓ moves the selection, Enter opens the full
// review, R starts or re-runs the review of the selected MR.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { RefreshCw } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { EmptyState, ErrorBox, Loading, ScreenHeader } from "@/components/layout";
import { markPolled } from "@/components/shell";
import { buildGroups, groupOf, matchesFilter, moveSelection, resolveSelection, visibleIds, type GroupKey } from "@/components/inbox/inbox-logic";
import { MrFilterInput, MrList } from "@/components/inbox/mr-list";
import { MrPreview } from "@/components/inbox/mr-preview";
import { ReviewLocalDialog } from "@/components/inbox/review-local-dialog";
import type { MrRow } from "@/bindings/MrRow";
import { isRunActive } from "@/lib/format";
import { useNav } from "@/lib/nav";
import { useMrs, usePollNow, useProjects, useSettings, useStartReview } from "@/lib/queries";
import { cn } from "@/lib/utils";

const COLLAPSED_KEY = "warden.inbox.collapsed";
const ALL = "all";

function loadCollapsed(): Set<GroupKey> {
  try {
    const v = JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? "[]");
    return new Set(Array.isArray(v) ? v : []);
  } catch {
    return new Set();
  }
}

function saveCollapsed(s: Set<GroupKey>) {
  try {
    localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...s]));
  } catch {
    /* storage unavailable: groups just start expanded next time */
  }
}

function isTextField(el: HTMLElement): boolean {
  return el.isContentEditable || el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.getAttribute("role") === "combobox";
}

export function InboxScreen({ initialMrId }: { initialMrId?: number }) {
  const { navigate } = useNav();
  const [projectId, setProjectId] = useState<number | null>(null);
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<Set<GroupKey>>(loadCollapsed);
  const [selectedId, setSelectedId] = useState<number | null>(initialMrId ?? null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const filterRef = useRef<HTMLInputElement>(null);
  const reveal = useRef<number | null>(initialMrId ?? null);

  const projectsQ = useProjects();
  const projects = projectsQ.data ?? [];
  const mrs = useMrs(projectId);
  const settings = useSettings().data;
  const maxTurns = settings?.max_turns ?? 30;
  const poll = usePollNow();
  const start = useStartReview();

  // A route with an mrId (palette, sidebar card, other screens) preselects that MR.
  useEffect(() => {
    if (initialMrId == null) return;
    setSelectedId(initialMrId);
    setQuery("");
    reveal.current = initialMrId;
  }, [initialMrId]);

  const rows = mrs.data ?? [];
  const filtered = useMemo(() => rows.filter((r) => matchesFilter(r, query)), [rows, query]);
  const groups = useMemo(() => buildGroups(filtered), [filtered]);

  // Expand the group holding a preselected MR once it is loaded.
  useEffect(() => {
    const id = reveal.current;
    if (id == null) return;
    const row = rows.find((r) => r.id === id);
    if (!row) return;
    reveal.current = null;
    const g = groupOf(row);
    setCollapsed((c) => {
      if (!c.has(g)) return c;
      const n = new Set(c);
      n.delete(g);
      return n;
    });
  }, [rows]);

  const ids = useMemo(() => visibleIds(groups, collapsed), [groups, collapsed]);
  const currentId = resolveSelection(ids, selectedId);
  const current = filtered.find((r) => r.id === currentId) ?? null;
  const needsCount = groups.find((g) => g.key === "needs")?.rows.length ?? 0;

  const toggleGroup = (k: GroupKey) =>
    setCollapsed((c) => {
      const n = new Set(c);
      if (n.has(k)) n.delete(k);
      else n.add(k);
      saveCollapsed(n);
      return n;
    });

  const openReview = useCallback(
    (row: MrRow | null) => {
      if (!row) return;
      if (row.latest_run) navigate({ screen: "review", runId: row.latest_run.id });
      else toast.info(`!${row.iid} has not been reviewed yet. Press R to start.`);
    },
    [navigate],
  );

  const startReview = useCallback(
    (row: MrRow | null) => {
      if (!row || start.isPending) return;
      if (row.latest_run && isRunActive(row.latest_run.status)) return;
      if (row.project_id === 0) {
        toast.info("Local reviews are started from “Review MR…”.");
        return;
      }
      start.mutate(row.id, { onSuccess: () => toast.success(`Review started for ${row.iid ? `!${row.iid}` : row.title}`) });
    },
    [start],
  );

  // Keyboard: ↑↓ select, Enter opens, R reviews. Typing in fields and dialogs is left alone, except
  // that ↑↓ and Enter also work from the filter box.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey) return;
      const t = e.target instanceof HTMLElement ? e.target : null;
      if (t?.closest('[role="dialog"]')) return;
      const inFilter = !!t?.hasAttribute("data-inbox-filter");
      if (t && isTextField(t) && !inFilter) return;
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const next = moveSelection(ids, currentId, e.key === "ArrowDown" ? 1 : -1);
        if (next != null) setSelectedId(next);
      } else if (e.key === "Enter") {
        if (t && !inFilter && (t.tagName === "BUTTON" || t.tagName === "A")) return;
        e.preventDefault();
        openReview(current);
      } else if ((e.key === "r" || e.key === "R") && !inFilter && !e.shiftKey) {
        e.preventDefault();
        startReview(current);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [ids, currentId, current, openReview, startReview]);

  const forgeName = (row: MrRow) => (projects.find((p) => p.id === row.project_id)?.forge === "github" ? "GitHub" : "GitLab");
  const noProjects = projectsQ.isSuccess && projects.length === 0;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <ScreenHeader
        title="Inbox"
        meta={
          mrs.data ? (
            <span>
              {rows.length} open{needsCount > 0 && ` · ${needsCount} need${needsCount === 1 ? "s" : ""} you`}
            </span>
          ) : undefined
        }
        actions={
          <>
            <Select value={projectId == null ? ALL : String(projectId)} onValueChange={(v) => setProjectId(v === ALL ? null : Number(v))}>
              <SelectTrigger className="h-7 w-[170px] font-sans text-xs" aria-label="Project">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={ALL}>All projects</SelectItem>
                {projects.map((p) => (
                  <SelectItem key={p.id} value={String(p.id)}>
                    {p.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button onClick={() => poll.mutate(undefined, { onSuccess: () => markPolled() })} disabled={poll.isPending}>
              <RefreshCw className={cn(poll.isPending && "animate-spin")} /> Poll now
            </Button>
            <Button variant="default" onClick={() => setDialogOpen(true)}>
              Review MR…
            </Button>
          </>
        }
      />

      {mrs.isLoading ? (
        <Loading />
      ) : mrs.error ? (
        <ErrorBox error={mrs.error} />
      ) : rows.length === 0 ? (
        noProjects && projectId == null ? (
          <EmptyState title="No projects yet" className="flex-1">
            <p>Add the GitLab projects Warden should watch. New merge requests are reviewed on the next poll.</p>
            <Button variant="default" className="mt-3" onClick={() => navigate({ screen: "settings", pane: "projects" })}>
              Add projects
            </Button>
          </EmptyState>
        ) : (
          <EmptyState title="No open merge requests" className="flex-1">
            {projectId == null ? "Watched projects have no open MRs targeting their branches." : "This project has no open MRs targeting its branches."} Press Poll now to check again, or use “Review MR…” for a local branch.
          </EmptyState>
        )
      ) : (
        <div className="grid min-h-0 flex-1 grid-cols-[300px_minmax(0,1fr)] xl:grid-cols-[340px_minmax(0,1fr)]">
          <div className="flex min-h-0 flex-col border-r bg-surface">
            <div className="shrink-0 border-b px-3 py-2.5">
              <MrFilterInput ref={filterRef} value={query} onChange={setQuery} />
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto">
              {filtered.length === 0 ? (
                <EmptyState title="No matches">No open MR matches “{query.trim()}”.</EmptyState>
              ) : (
                <MrList
                  groups={groups}
                  collapsed={collapsed}
                  onToggleGroup={toggleGroup}
                  selectedId={currentId}
                  onSelect={setSelectedId}
                  onOpen={openReview}
                  maxTurns={maxTurns}
                />
              )}
            </div>
            <div className="shrink-0 border-t px-3 py-1.5 font-mono text-[10.5px] text-muted-foreground">↑↓ move · ↵ open review · R review</div>
          </div>
          <div className="min-h-0 overflow-y-auto bg-background">
            {current ? (
              <MrPreview
                key={current.id}
                row={current}
                maxTurns={maxTurns}
                postingPaused={!!settings?.posting_paused}
                forgeName={forgeName(current)}
                onOpen={() => openReview(current)}
                onReview={() => startReview(current)}
                starting={start.isPending}
              />
            ) : (
              <EmptyState title="Select a merge request">Pick an MR on the left to see Claude's verdict, findings and CI.</EmptyState>
            )}
          </div>
        </div>
      )}

      <ReviewLocalDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        projects={projects}
        mrs={rows}
        preferProjectId={current?.project_id ?? projectId}
        onStarted={(runId) => navigate({ screen: "review", runId })}
      />
    </div>
  );
}
