import { useMemo, useRef, useState } from "react";
import { toast } from "sonner";
import {
  Check,
  ChevronRight,
  CircleDot,
  Copy,
  Cpu,
  ExternalLink,
  ListChecks,
  Send,
  ShieldCheck,
  TriangleAlert,
  Wrench,
  X,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { EmptyState, ErrorBox, Loading } from "@/components/layout";
import { RunStatusPill } from "@/components/pills";
import type { AuditEvent } from "@/bindings/AuditEvent";
import { formatDuration, formatRelative, runStatusLabel, shortSha } from "@/lib/format";
import { useNav } from "@/lib/nav";
import { useAudit, useRun } from "@/lib/queries";
import { cn } from "@/lib/utils";
import {
  ciLabel,
  groupEvents,
  isErrorEvent,
  phaseCounts,
  phaseLabel,
  PHASES,
  statusPillText,
  summarizeCi,
  tallyFindings,
  toggleIn,
  verdictLabel,
  verdictTone,
  type EventGroup,
  type Phase,
  type RunRow,
} from "./history-logic";
import { formatAbsolute } from "./runs-table";
import { summarize } from "./summarize";

const PHASE_ICON: Record<Phase, typeof Cpu> = {
  engine: Cpu,
  tool_call: Wrench,
  guardrail: ShieldCheck,
  post: Send,
  error: TriangleAlert,
  other: CircleDot,
};

const PHASE_TONE: Record<Phase, string> = {
  engine: "text-muted-foreground",
  tool_call: "text-brand",
  guardrail: "text-orange",
  post: "text-green",
  error: "text-red",
  other: "text-muted-foreground",
};

const PHASE_DOT: Record<Phase, string> = {
  engine: "bg-muted-foreground",
  tool_call: "bg-brand",
  guardrail: "bg-orange",
  post: "bg-green",
  error: "bg-red",
  other: "bg-border",
};

const TONE_TEXT = { good: "text-green", bad: "text-red", neutral: "text-foreground", active: "text-brand" } as const;

export const DRAWER_MIN = 360;
export const DRAWER_DEFAULT = 480;

function formatTime(iso: string): string {
  const t = Date.parse(iso);
  return Number.isNaN(t) ? iso : new Date(t).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
}

/** Right-hand audit drawer for one run: header, links, then the phase-grouped event timeline. */
export function AuditDrawer({
  runId,
  row,
  width,
  maxWidth,
  onWidth,
  onClose,
}: {
  runId: number;
  row: RunRow | undefined;
  width: number;
  maxWidth: number;
  onWidth: (w: number) => void;
  onClose: () => void;
}) {
  const { navigate } = useNav();
  // The row (list_run_rows) has everything the header needs; get_run (builds a diff) is only a
  // fallback for a deep-linked run outside the loaded rows.
  const detail = useRun(row ? null : runId);
  const audit = useAudit(runId, 500);
  const [kinds, setKinds] = useState<Phase[]>([]);

  const run = detail.data?.run ?? row?.run;
  const mr = detail.data?.mr ?? row?.mr ?? null;
  const findings = detail.data ? tallyFindings(detail.data.findings) : (row?.findings ?? null);
  const ci = detail.data ? summarizeCi(detail.data.ci_jobs) : (row?.ci ?? null);
  const events = useMemo(() => audit.data ?? [], [audit.data]);
  const counts = useMemo(() => phaseCounts(events), [events]);
  const groups = useMemo(() => groupEvents(events, kinds), [events, kinds]);
  const ciInfo = ciLabel(ci);

  return (
    <aside
      className="absolute inset-y-0 right-0 z-20 flex min-h-0 flex-col border-l bg-surface shadow-[-8px_0_24px_-12px_hsl(var(--fg)/0.25)]"
      style={{ width: Math.min(width, maxWidth) }}
      aria-label={`Audit for run ${runId}`}
    >
      <ResizeHandle width={width} maxWidth={maxWidth} onWidth={onWidth} />

      {/* header */}
      <div className="shrink-0 border-b px-4 pb-3 pt-3">
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <div className="truncate text-[13px] font-semibold">
              {mr ? (
                <>
                  <span className="font-mono">{mr.iid ? `!${mr.iid}` : "local"}</span> {mr.title}
                </>
              ) : (
                `Run #${runId}`
              )}
            </div>
            <div className="truncate font-mono text-2xs text-muted-foreground">
              Run #{runId}
              {row?.projectName ? ` · ${row.projectName}` : ""}
              {mr?.author ? ` · ${mr.author}` : ""}
            </div>
          </div>
          <Button variant="ghost" size="icon" className="-mr-2 -mt-1 h-7 w-7" onClick={onClose} aria-label="Close audit (Esc)" title="Close (Esc)">
            <X />
          </Button>
        </div>

        {detail.error && !run ? (
          <ErrorBox error={detail.error} />
        ) : !run ? (
          <Loading />
        ) : (
          <>
            <div className="mt-2 flex flex-wrap items-center gap-2">
              <RunStatusPill status={run.status}>{statusPillText({ run, findings }, runStatusLabel(run.status))}</RunStatusPill>
              {run.verdict && <span className={cn("text-xs font-medium", TONE_TEXT[verdictTone(run.verdict)])}>{verdictLabel(run.verdict)}</span>}
              {findings && (
                <span className="text-2xs text-muted-foreground">
                  {findings.posted} posted · {findings.drafted} drafted · {findings.dropped} dropped
                </span>
              )}
            </div>
            <dl className="mt-2 grid grid-cols-[auto_minmax(0,1fr)_auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
              <Meta label="Range">
                <span className="font-mono">
                  {shortSha(run.since_sha ?? run.base_sha)}…{shortSha(run.head_sha)}
                </span>
                {run.since_sha && <span className="text-muted-foreground"> · incremental</span>}
              </Meta>
              <Meta label="Model">
                <span className="font-mono">{run.model || "—"}</span>
              </Meta>
              <Meta label="Turns">
                <span className="font-mono">{run.turns || "—"}</span>
              </Meta>
              <Meta label="Duration">
                <span className="font-mono">{formatDuration(run.duration_ms)}</span>
              </Meta>
              <Meta label="Started">
                <span title={formatAbsolute(run.started_at)}>{formatRelative(run.started_at)}</span>
              </Meta>
              <Meta label="CI">
                <span className={cn("font-mono", ciInfo.tone === "bad" ? "text-red" : ciInfo.tone === "good" ? "text-green" : undefined)}>{ciInfo.label}</span>
              </Meta>
            </dl>
            {run.error && run.status === "skipped" ? (
              <div className="mt-2 rounded-md border bg-surface-2 px-2.5 py-1.5 text-xs text-muted-foreground">Skipped: {run.error}</div>
            ) : run.error ? (
              <div className="mt-2 rounded-md border border-red/40 bg-red-soft px-2.5 py-1.5 text-xs text-red">
                <TriangleAlert className="mr-1 inline h-3.5 w-3.5 align-[-2px]" aria-hidden />
                {run.error}
              </div>
            ) : null}
            {run.summary && <p className="mt-2 line-clamp-3 text-xs text-muted-foreground">{run.summary}</p>}
            <div className="mt-2.5 flex flex-wrap items-center gap-2">
              <Button size="sm" variant="default" onClick={() => navigate({ screen: "review", runId })}>
                <ListChecks />
                Open review
              </Button>
              <Button size="sm" disabled={ci != null && ci.total === 0} onClick={() => navigate({ screen: "ci", runId })}>
                Open CI
              </Button>
              {mr?.web_url && (
                <a href={mr.web_url} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 rounded text-xs text-primary hover:underline">
                  <ExternalLink className="h-3.5 w-3.5" aria-hidden />
                  Open MR
                </a>
              )}
            </div>
          </>
        )}
      </div>

      {/* kind chips */}
      <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b px-4 py-2" role="group" aria-label="Filter events by kind">
        <Chip on={kinds.length === 0} onClick={() => setKinds([])}>
          All <span className="font-mono">{events.length}</span>
        </Chip>
        {PHASES.filter((p) => counts[p.value] > 0).map((p) => (
          <Chip key={p.value} on={kinds.includes(p.value)} danger={p.value === "error"} onClick={() => setKinds((k) => toggleIn(k, p.value))}>
            {p.label} <span className="font-mono">{counts[p.value]}</span>
          </Chip>
        ))}
      </div>

      {/* timeline */}
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-2">
        {audit.isLoading ? (
          <Loading />
        ) : audit.error ? (
          <ErrorBox error={audit.error} />
        ) : events.length === 0 ? (
          <EmptyState title="No audit events for this run">Skipped runs and runs queued before auditing started have no trail.</EmptyState>
        ) : groups.length === 0 ? (
          <EmptyState title="No events of this kind">
            <button type="button" className="text-primary hover:underline" onClick={() => setKinds([])}>
              Show all events
            </button>
          </EmptyState>
        ) : (
          <ol className="space-y-2">
            {groups.map((g) => (
              <Group key={g.key} group={g} />
            ))}
          </ol>
        )}
      </div>
    </aside>
  );
}

function Meta({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 truncate">{children}</dd>
    </>
  );
}

function Chip({ on, danger, onClick, children }: { on: boolean; danger?: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-pressed={on}
      onClick={onClick}
      className={cn(
        "inline-flex items-center gap-1 rounded-full border px-2 py-px text-2xs",
        on ? (danger ? "border-red bg-red-soft font-semibold text-red" : "border-primary bg-brand-soft font-semibold text-primary") : danger ? "bg-card text-red hover:bg-red-soft" : "bg-card hover:bg-accent",
      )}
    >
      {children}
    </button>
  );
}

function Group({ group }: { group: EventGroup }) {
  const Icon = PHASE_ICON[group.phase];
  const first = group.events[0];
  return (
    <li className={cn("rounded-md border", group.phase === "error" ? "border-red/40 bg-red-soft/50" : "bg-card")}>
      <div className="flex items-center gap-2 border-b px-2.5 py-1.5">
        <Icon className={cn("h-3.5 w-3.5 shrink-0", PHASE_TONE[group.phase])} aria-hidden />
        <span className={cn("text-xs font-semibold", group.phase === "error" && "text-red")}>{phaseLabel(group.phase)}</span>
        {group.events.length > 1 && <Badge tone="grey">{group.events.length}</Badge>}
        <span className="ml-auto font-mono text-2xs text-muted-foreground" title={formatAbsolute(first.created_at)}>
          {formatTime(first.created_at)}
        </span>
      </div>
      <ol className="py-0.5">
        {group.events.map((ev) => (
          <EventItem key={ev.id} ev={ev} phase={group.phase} />
        ))}
      </ol>
    </li>
  );
}

function EventItem({ ev, phase }: { ev: AuditEvent; phase: Phase }) {
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const err = isErrorEvent(ev);
  const json = useMemo(() => JSON.stringify(ev.payload, null, 2), [ev.payload]);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(json);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      toast.error("Could not copy to the clipboard");
    }
  };
  return (
    <li className={cn("px-2.5", err && "text-red")}>
      <button
        type="button"
        className="grid w-full grid-cols-[62px_8px_minmax(0,1fr)_12px] items-start gap-2 rounded py-1 text-left hover:bg-surface-2/60"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
      >
        <span className="pt-px font-mono text-2xs text-muted-foreground" title={formatAbsolute(ev.created_at)}>
          {formatTime(ev.created_at)}
        </span>
        <span className={cn("mt-[5px] h-2 w-2 rounded-full", PHASE_DOT[phase])} aria-hidden />
        <span className={cn("min-w-0 break-words font-mono text-xs", !open && "truncate")}>{summarize(ev.payload) || ev.kind}</span>
        <ChevronRight className={cn("mt-0.5 h-3 w-3 text-muted-foreground transition-transform", open && "rotate-90")} aria-hidden />
      </button>
      {open && (
        <div className="relative mb-1.5 ml-[70px]">
          <pre className="code-block max-h-72 overflow-auto whitespace-pre-wrap break-all pr-9 text-foreground">{json}</pre>
          <Button size="icon" variant="ghost" className="absolute right-1 top-1 h-6 w-6" onClick={copy} aria-label="Copy JSON" title="Copy JSON">
            {copied ? <Check /> : <Copy />}
          </Button>
        </div>
      )}
    </li>
  );
}

function ResizeHandle({ width, maxWidth, onWidth }: { width: number; maxWidth: number; onWidth: (w: number) => void }) {
  const start = useRef<{ x: number; w: number } | null>(null);
  const clamp = (w: number) => Math.round(Math.max(DRAWER_MIN, Math.min(maxWidth, w)));
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize audit drawer"
      aria-valuemin={DRAWER_MIN}
      aria-valuemax={maxWidth}
      aria-valuenow={Math.min(width, maxWidth)}
      tabIndex={0}
      className="absolute inset-y-0 -left-1 z-10 w-2 cursor-col-resize hover:bg-brand/20 focus-visible:bg-brand/30"
      onPointerDown={(e) => {
        e.preventDefault();
        (e.target as HTMLElement).setPointerCapture(e.pointerId);
        start.current = { x: e.clientX, w: Math.min(width, maxWidth) };
      }}
      onPointerMove={(e) => {
        if (start.current) onWidth(clamp(start.current.w + (start.current.x - e.clientX)));
      }}
      onPointerUp={() => {
        start.current = null;
      }}
      onDoubleClick={() => onWidth(DRAWER_DEFAULT)}
      onKeyDown={(e) => {
        if (e.key === "ArrowLeft") {
          e.preventDefault();
          onWidth(clamp(width + 24));
        } else if (e.key === "ArrowRight") {
          e.preventDefault();
          onWidth(clamp(width - 24));
        }
      }}
    />
  );
}
