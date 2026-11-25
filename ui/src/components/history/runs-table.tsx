import { useEffect, useRef } from "react";

import { RunStatusPill } from "@/components/pills";
import { formatDuration, formatRelative, runStatusLabel, shortSha } from "@/lib/format";
import { cn } from "@/lib/utils";
import { ciLabel, statusPillText, verdictLabel, verdictTone, type RunRow } from "./history-logic";

const TONE_TEXT = { good: "text-green", bad: "text-red", neutral: "text-muted-foreground", active: "text-brand" } as const;

export function formatAbsolute(iso: string | null | undefined): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  return Number.isNaN(t) ? iso : new Date(t).toLocaleString(undefined, { weekday: "short", year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

/** Runs table: click (or ↑↓ + Enter from the screen) opens the audit drawer. */
export function RunsTable({
  rows,
  cursorId,
  openId,
  onOpen,
}: {
  rows: RunRow[];
  cursorId: number | null;
  openId: number | null;
  onOpen: (runId: number) => void;
}) {
  const bodyRef = useRef<HTMLTableSectionElement>(null);

  useEffect(() => {
    if (cursorId == null) return;
    const el = bodyRef.current?.querySelector<HTMLElement>(`[data-run-id="${cursorId}"]`);
    el?.scrollIntoView?.({ block: "nearest" });
  }, [cursorId]);

  return (
    <table className="w-full border-collapse text-[13px]" aria-label="Review runs">
      <thead>
        <tr className="text-left text-2xs font-semibold uppercase tracking-wide text-muted-foreground">
          <Th className="pl-4">Merge request</Th>
          <Th className="w-[118px]">Project</Th>
          <Th className="w-[132px]">Status</Th>
          <Th className="w-[112px]">Verdict</Th>
          <Th className="w-[86px]" title="Posted / drafted / dropped by guardrails">
            Findings
          </Th>
          <Th className="w-[92px]">CI</Th>
          <Th className="w-[64px] text-right">Time</Th>
          <Th className="w-[48px] text-right">Turns</Th>
          <Th className="w-[84px] pr-4">Started</Th>
        </tr>
      </thead>
      <tbody ref={bodyRef}>
        {rows.map((r) => {
          const { run, mr } = r;
          const on = cursorId === run.id;
          const open = openId === run.id;
          const ci = ciLabel(r.ci);
          return (
            <tr
              key={run.id}
              data-run-id={run.id}
              data-state={on ? "selected" : undefined}
              onClick={() => onOpen(run.id)}
              className={cn(
                "cursor-pointer border-b hover:bg-surface-2/60",
                on && "bg-surface-2",
                open && "bg-brand-soft/60 shadow-[inset_2px_0_0_hsl(var(--brand))] hover:bg-brand-soft/60",
              )}
            >
              <td className="max-w-0 py-2 pl-4 pr-2">
                <button
                  type="button"
                  className="block w-full min-w-0 rounded text-left"
                  aria-label={`Open audit for run ${run.id}`}
                  tabIndex={on ? 0 : -1}
                  onClick={(e) => {
                    e.stopPropagation();
                    onOpen(run.id);
                  }}
                >
                  <div className="truncate font-semibold">
                    {mr ? (
                      <>
                        <span className="font-mono">{mr.iid ? `!${mr.iid}` : "local"}</span> {mr.title}
                      </>
                    ) : (
                      <span className="text-muted-foreground">{`MR ${run.mr_id}`}</span>
                    )}
                  </div>
                  <div className="truncate font-mono text-2xs text-muted-foreground">
                    #{run.id} · {shortSha(run.since_sha ?? run.base_sha)}…{shortSha(run.head_sha)}
                    {run.since_sha ? " · incremental" : ""}
                    {mr?.author ? ` · ${mr.author}` : ""}
                  </div>
                </button>
              </td>
              <td className="max-w-0 truncate px-2 py-2 font-mono text-xs text-muted-foreground">{r.projectName ?? "—"}</td>
              <td className="px-2 py-2" title={run.error ?? undefined}>
                <RunStatusPill status={run.status}>{statusPillText(r, runStatusLabel(run.status))}</RunStatusPill>
              </td>
              <td className={cn("whitespace-nowrap px-2 py-2 text-xs", TONE_TEXT[verdictTone(run.verdict)])}>{verdictLabel(run.verdict)}</td>
              <td className="whitespace-nowrap px-2 py-2 font-mono text-xs">
                {r.findings ? (
                  <span title={`${r.findings.posted} posted · ${r.findings.drafted} drafted · ${r.findings.dropped} dropped`}>
                    <span className={r.findings.posted ? "text-green" : "text-muted-foreground"}>{r.findings.posted}</span>
                    <span className="text-muted-foreground"> / </span>
                    <span className={r.findings.drafted ? "text-orange" : "text-muted-foreground"}>{r.findings.drafted}</span>
                    <span className="text-muted-foreground"> / </span>
                    <span className="text-muted-foreground">{r.findings.dropped}</span>
                  </span>
                ) : (
                  <span className="text-muted-foreground">—</span>
                )}
              </td>
              <td className={cn("whitespace-nowrap px-2 py-2 font-mono text-xs", TONE_TEXT[ci.tone])}>{r.ci ? ci.label : "—"}</td>
              <td className="whitespace-nowrap px-2 py-2 text-right font-mono text-xs">{formatDuration(run.duration_ms)}</td>
              <td className="px-2 py-2 text-right font-mono text-xs">{run.turns || "—"}</td>
              <td className="whitespace-nowrap py-2 pl-2 pr-4 text-xs text-muted-foreground" title={formatAbsolute(run.started_at)}>
                {formatRelative(run.started_at)}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

function Th({ className, children, title }: { className?: string; children: React.ReactNode; title?: string }) {
  return (
    <th scope="col" title={title} className={cn("sticky top-0 z-10 border-b bg-surface px-2 py-1.5 font-semibold", className)}>
      {children}
    </th>
  );
}
