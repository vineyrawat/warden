// Approval queue (UX 5A + C): every pending finding across MRs in one dense, keyboard-driven list
// with bulk approve / dismiss / post, and an on-demand note preview pane.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Eye, Search } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { EmptyState, ErrorBox, Loading, ScreenHeader } from "@/components/layout";
import { Segmented } from "@/components/segmented";
import type { Severity } from "@/bindings/Severity";
import type { FindingStatus } from "@/bindings/FindingStatus";
import { Kbd } from "@/components/queue/bits";
import { PostConfirm, PostFooter, PostResults, SelectionBar } from "@/components/queue/bulk-bar";
import { PreviewPane } from "@/components/queue/preview-pane";
import { QueueEditor } from "@/components/queue/queue-editor";
import { QueueTable, rowDomId, type RowClick } from "@/components/queue/queue-list";
import {
  EMPTY_FILTER,
  actionTargets,
  approvedSet,
  cursorAfterRemoval,
  filterPending,
  forgeName,
  groupChecked,
  groupPending,
  isControlTarget,
  isEditableTarget,
  isFiltered,
  labelReports,
  moveCursor,
  nextUndecided,
  plural,
  postSet,
  projectNames,
  pruneSelection,
  queueKeyAction,
  queueMeta,
  selectRange,
  settleCursor,
  toggleAll,
  toggleGroup,
  toggleId,
  visibleIds,
  type LabeledReport,
  type PostSet,
  type QueueFilter,
  type QueueKey,
  type SortMode,
} from "@/components/queue/queue-logic";
import { SEVERITIES, capitalize } from "@/lib/format";
import { useNav } from "@/lib/nav";
import { usePendingFindings, usePostFindingsBulk, useSetFindingStatus, useSettings } from "@/lib/queries";

const PREVIEW_KEY = "warden.queue.preview";
const ALL = "__all";

function usePersistentFlag(key: string, initial: boolean): [boolean, (v: boolean) => void] {
  const [value, setValue] = useState<boolean>(() => {
    try {
      const v = localStorage.getItem(key);
      return v == null ? initial : v === "1";
    } catch {
      return initial;
    }
  });
  const set = useCallback(
    (v: boolean) => {
      setValue(v);
      try {
        localStorage.setItem(key, v ? "1" : "0");
      } catch {
        /* storage unavailable: keep the in-memory value */
      }
    },
    [key],
  );
  return [value, set];
}

type Confirm = { kind: "selected" | "approved"; set: PostSet; forge: string };

export function ApprovalQueueScreen() {
  const { navigate } = useNav();
  const pending = usePendingFindings();
  const settings = useSettings();
  const setStatus = useSetFindingStatus();
  const bulk = usePostFindingsBulk();

  const [filter, setFilter] = useState<QueueFilter>(EMPTY_FILTER);
  const [sort, setSort] = useState<SortMode>("mr");
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());
  const [selected, setSelected] = useState<Set<number>>(() => new Set());
  const [anchor, setAnchor] = useState<number | null>(null);
  const [cursor, setCursor] = useState<number | null>(null);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [previewOpen, setPreviewOpen] = usePersistentFlag(PREVIEW_KEY, false);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const [results, setResults] = useState<LabeledReport[] | null>(null);
  const [deciding, setDeciding] = useState(0);
  const lastIndex = useRef(0);
  const searchRef = useRef<HTMLInputElement>(null);

  const all = useMemo(() => pending.data ?? [], [pending.data]);
  const byId = useMemo(() => new Map(all.map((r) => [r.finding.id, r])), [all]);
  const rows = useMemo(() => filterPending(all, filter), [all, filter]);
  const rowIds = useMemo(() => rows.map((r) => r.finding.id), [rows]);
  const groups = useMemo(() => groupPending(rows, sort), [rows, sort]);
  const order = useMemo(() => visibleIds(groups, collapsed), [groups, collapsed]);
  const projects = useMemo(() => projectNames(all), [all]);
  // Selection limited to rows the filters show, so bulk actions never touch hidden findings.
  const sel = useMemo(() => pruneSelection(selected, rowIds), [selected, rowIds]);
  const approved = useMemo(() => approvedSet(all), [all]);
  const paused = settings.data?.posting_paused ?? false;
  const postBlocked = paused ? "Posting is paused." : null;
  const busy = deciding > 0 || bulk.isPending;
  const cursorRow = cursor != null ? (byId.get(cursor) ?? null) : null;

  // Drop selections and edits for findings that left the queue (posted, dismissed elsewhere).
  useEffect(() => {
    setSelected((s) => {
      const next = pruneSelection(s, byId.keys());
      return next.size === s.size ? s : next;
    });
    setEditingId((id) => (id != null && !byId.has(id) ? null : id));
  }, [byId]);

  // Keep the cursor on a visible row.
  useEffect(() => {
    const next = settleCursor(order, cursor, lastIndex.current);
    if (next !== cursor) setCursor(next);
  }, [order, cursor]);

  useEffect(() => {
    if (cursor == null) return;
    const i = order.indexOf(cursor);
    if (i >= 0) lastIndex.current = i;
    document.getElementById(rowDomId(cursor))?.scrollIntoView({ block: "nearest" });
  }, [cursor, order]);

  const moveTo = (id: number | null) => {
    setCursor(id);
    if (editingId != null && editingId !== id) setEditingId(null);
  };

  /** Sets every id's status; resolves false when any write failed (the hook shows the error). */
  const decide = async (ids: number[], to: FindingStatus): Promise<boolean> => {
    if (ids.length === 0) return true;
    setDeciding((n) => n + 1);
    try {
      await Promise.all(
        ids.map((id) => {
          const r = byId.get(id);
          return r ? setStatus.mutateAsync({ id, status: to, runId: r.run_id }) : undefined;
        }),
      );
      return true;
    } catch {
      return false;
    } finally {
      setDeciding((n) => n - 1);
    }
  };

  const approve = (ids = actionTargets(sel, cursor, order)) => {
    if (ids.length === 0) return;
    const todo = ids.filter((id) => byId.get(id)?.finding.status !== "approved");
    setConfirm(null);
    if (sel.size > 0) setSelected(new Set());
    else if (cursor != null) moveTo(nextUndecided(order, byId, cursor, new Set(ids)) ?? cursor);
    void decide(todo, "approved");
  };

  const dismiss = (ids = actionTargets(sel, cursor, order)) => {
    if (ids.length === 0) return;
    setConfirm(null);
    moveTo(cursorAfterRemoval(order, cursor, ids));
    setSelected((s) => new Set([...s].filter((id) => !ids.includes(id))));
    void decide(ids, "dismissed").then((ok) => {
      if (ok) toast(`Dismissed ${plural(ids.length, "finding")}`, { action: { label: "Undo", onClick: () => void decide(ids, "draft") } });
    });
  };

  const askPost = (kind: Confirm["kind"]) => {
    if (postBlocked) return;
    const set = kind === "approved" ? approved : postSet(all, sel);
    if (set.ids.length === 0) return;
    setResults(null);
    setConfirm({ kind, set, forge: forgeName(all.filter((r) => set.ids.includes(r.finding.id))) });
  };

  const doPost = () => {
    if (!confirm) return;
    const { set } = confirm;
    const snapshot = all;
    bulk.mutate(set.ids, {
      onSuccess: (reports) => {
        setResults(labelReports(set.ids, snapshot, reports));
        setSelected(new Set());
        setConfirm(null);
      },
      onError: () => setConfirm(null),
    });
  };

  const onRowClick = (id: number, how: RowClick) => {
    if (how.shift) {
      setSelected((s) => selectRange(s, order, anchor ?? cursor, id));
    } else if (how.toggle) {
      setSelected((s) => toggleId(s, id));
      setAnchor(id);
    } else {
      setAnchor(id);
    }
    moveTo(id);
  };

  const onCheck = (id: number, shift: boolean) => {
    if (shift) setSelected((s) => selectRange(s, order, anchor ?? cursor, id));
    else {
      setSelected((s) => toggleId(s, id));
      setAnchor(id);
    }
    moveTo(id);
  };

  const run = (action: QueueKey): boolean => {
    switch (action) {
      case "up":
      case "down":
        moveTo(moveCursor(order, cursor, action === "up" ? -1 : 1));
        return true;
      case "toggle":
        if (cursor == null) return false;
        setSelected((s) => toggleId(s, cursor));
        setAnchor(cursor);
        return true;
      case "selectAll":
        setSelected((s) => toggleAll(s, order));
        return true;
      case "approve":
        approve();
        return true;
      case "dismiss":
        dismiss();
        return true;
      case "edit":
        if (cursor == null) return false;
        setEditingId(cursor);
        return true;
      case "preview":
        setPreviewOpen(!previewOpen);
        return true;
      case "escape":
        if (confirm) setConfirm(null);
        else if (editingId != null) setEditingId(null);
        else if (sel.size > 0) setSelected(new Set());
        else return false;
        return true;
    }
  };

  const keyHandler = useRef<(e: KeyboardEvent) => void>(() => {});
  keyHandler.current = (e: KeyboardEvent) => {
    if (e.defaultPrevented || all.length === 0) return;
    // Dialogs (command palette, settings) own the keyboard while open.
    if (document.querySelector('[role="dialog"][data-state="open"]')) return;
    const editable = isEditableTarget(e.target);
    if (e.key === "/" && !editable && !e.metaKey && !e.ctrlKey) {
      e.preventDefault();
      searchRef.current?.focus();
      return;
    }
    const action = queueKeyAction({
      key: e.key,
      metaKey: e.metaKey,
      ctrlKey: e.ctrlKey,
      altKey: e.altKey,
      shiftKey: e.shiftKey,
      editable,
      onControl: isControlTarget(e.target),
    });
    if (action && run(action)) e.preventDefault();
  };
  useEffect(() => {
    const on = (e: KeyboardEvent) => keyHandler.current(e);
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, []);

  const toggleCollapse = (key: string) =>
    setCollapsed((c) => {
      const next = new Set(c);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const openReview = (runId: number, findingId?: number) => navigate({ screen: "review", runId, findingId });
  const openSettings = () => navigate({ screen: "settings", pane: "advanced" });

  const confirmEl = (kind: Confirm["kind"]) =>
    confirm?.kind === kind ? <PostConfirm set={confirm.set} forge={confirm.forge} onConfirm={doPost} onCancel={() => setConfirm(null)} pending={bulk.isPending} /> : undefined;

  const filtered = isFiltered(filter);
  const meta = filtered && all.length > 0 ? `${rows.length} shown · ${queueMeta(all)}` : queueMeta(all);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <ScreenHeader
        title="Approval queue"
        meta={all.length > 0 ? <span>{meta}</span> : undefined}
        actions={
          all.length > 0 ? (
            <Button size="sm" variant={previewOpen ? "secondary" : "outline"} aria-pressed={previewOpen} onClick={() => setPreviewOpen(!previewOpen)}>
              <Eye /> Preview <Kbd>P</Kbd>
            </Button>
          ) : undefined
        }
      />
      {pending.isLoading ? (
        <Loading />
      ) : pending.error ? (
        <ErrorBox error={pending.error} />
      ) : all.length === 0 ? (
        <div className="min-h-0 flex-1 overflow-y-auto">
          {results && <PostResults reports={results} onClose={() => setResults(null)} />}
          <EmptyState title="Nothing waiting.">
            New findings land here after each review.{" "}
            <button type="button" onClick={() => navigate({ screen: "inbox" })} className="text-primary hover:underline">
              Go to Inbox
            </button>
          </EmptyState>
        </div>
      ) : (
        <div className="flex min-h-0 flex-1">
          <div className="flex min-w-0 flex-1 flex-col">
            <Toolbar
              filter={filter}
              onFilter={setFilter}
              projects={projects}
              sort={sort}
              onSort={setSort}
              searchRef={searchRef}
            />
            <SelectionBar
              checked={groupChecked(sel, order)}
              selectedCount={sel.size}
              onToggleAll={() => setSelected((s) => toggleAll(s, order))}
              onApprove={() => approve()}
              onDismiss={() => dismiss()}
              onPost={() => askPost("selected")}
              busy={busy}
              postBlocked={postBlocked}
            >
              {confirmEl("selected")}
            </SelectionBar>
            {results && <PostResults reports={results} onClose={() => setResults(null)} />}
            <div className="min-h-0 flex-1 overflow-y-auto">
              {rows.length === 0 ? (
                <EmptyState title="No findings match these filters">
                  <button type="button" onClick={() => setFilter(EMPTY_FILTER)} className="text-primary hover:underline">
                    Clear filters
                  </button>
                </EmptyState>
              ) : (
                <QueueTable
                  groups={groups}
                  collapsed={collapsed}
                  onToggleCollapse={toggleCollapse}
                  cursorId={cursor}
                  selected={sel}
                  onRowClick={onRowClick}
                  onCheck={onCheck}
                  onGroupCheck={(ids) => setSelected((s) => toggleGroup(s, ids))}
                  onOpenReview={openReview}
                  onUndoApprove={(r) => void decide([r.finding.id], "draft")}
                  editingId={previewOpen ? null : editingId}
                  renderEditor={(r) => <QueueEditor key={r.finding.id} finding={r.finding} onDone={() => setEditingId(null)} />}
                  compact={previewOpen}
                />
              )}
            </div>
            <PostFooter
              approved={approved}
              onPostAll={() => askPost("approved")}
              postBlocked={postBlocked}
              onOpenSettings={openSettings}
              confirm={confirmEl("approved")}
              busy={busy}
            />
          </div>
          {previewOpen && (
            <PreviewPane
              key={cursorRow?.finding.id ?? "none"}
              row={cursorRow}
              editing={editingId != null && editingId === cursor}
              onEdit={() => cursor != null && setEditingId(cursor)}
              onEditDone={() => setEditingId(null)}
              onApprove={() => cursor != null && approve([cursor])}
              onDismiss={() => cursor != null && dismiss([cursor])}
              onClose={() => setPreviewOpen(false)}
              busy={busy}
            />
          )}
        </div>
      )}
    </div>
  );
}

function Toolbar({
  filter,
  onFilter,
  projects,
  sort,
  onSort,
  searchRef,
}: {
  filter: QueueFilter;
  onFilter: (f: QueueFilter) => void;
  projects: string[];
  sort: SortMode;
  onSort: (s: SortMode) => void;
  searchRef: React.RefObject<HTMLInputElement>;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2 border-b bg-surface px-4 py-2">
      <label className="flex h-7 min-w-[180px] flex-1 items-center gap-2 rounded-[5px] border bg-surface-2 px-2 text-xs focus-within:ring-2 focus-within:ring-ring">
        <Search className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
        <input
          ref={searchRef}
          value={filter.search}
          onChange={(e) => onFilter({ ...filter, search: e.target.value })}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              if (filter.search) onFilter({ ...filter, search: "" });
              else e.currentTarget.blur();
            }
          }}
          placeholder="Search findings, files, rules, MRs"
          aria-label="Search findings"
          className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground"
        />
        <Kbd>/</Kbd>
      </label>
      {projects.length > 1 && (
        <Select value={filter.project ?? ALL} onValueChange={(v) => onFilter({ ...filter, project: v === ALL ? null : v })}>
          <SelectTrigger className="h-7 w-40 bg-card font-sans" aria-label="Project">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL}>All projects</SelectItem>
            {projects.map((p) => (
              <SelectItem key={p} value={p}>
                {p}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      <Select value={filter.severity ?? ALL} onValueChange={(v) => onFilter({ ...filter, severity: v === ALL ? null : (v as Severity) })}>
        <SelectTrigger className="h-7 w-32 bg-card font-sans" aria-label="Severity">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={ALL}>All severities</SelectItem>
          {SEVERITIES.map((s) => (
            <SelectItem key={s} value={s}>
              {capitalize(s)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Segmented<SortMode>
        label="Group findings"
        value={sort}
        onChange={onSort}
        options={[
          { value: "mr", label: "By MR" },
          { value: "severity", label: "By severity" },
        ]}
      />
    </div>
  );
}
