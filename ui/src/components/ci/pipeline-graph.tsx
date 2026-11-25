import { Fragment } from "react";
import { CircleCheck, CircleDashed, CircleMinus, CircleX, Clock, Copy, LoaderCircle, TimerOff } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import type { CiJob } from "@/bindings/CiJob";
import type { CiStatus } from "@/bindings/CiStatus";
import { groupStages, missingTool } from "@/lib/ci-parse";
import { ciJobPill, ciStatusLabel, installHint } from "@/lib/format";
import { cn } from "@/lib/utils";

export function JobStatusIcon({ status, className }: { status: CiStatus; className?: string }) {
  const c = cn("h-3.5 w-3.5 shrink-0", className);
  switch (status) {
    case "passed":
      return <CircleCheck className={cn(c, "text-green")} aria-label="passed" />;
    case "failed":
      return <CircleX className={cn(c, "text-red")} aria-label="failed" />;
    case "timed_out":
      return <TimerOff className={cn(c, "text-red")} aria-label="timed out" />;
    case "running":
      return <LoaderCircle className={cn(c, "animate-spin text-brand")} aria-label="running" />;
    case "pending":
      return <Clock className={cn(c, "text-muted-foreground")} aria-label="pending" />;
    case "not_installed":
      return <CircleDashed className={cn(c, "text-muted-foreground")} aria-label="not installed" />;
    default:
      return <CircleMinus className={cn(c, "text-muted-foreground")} aria-label={ciStatusLabel(status)} />;
  }
}

export async function copyText(text: string, what = "Copied") {
  try {
    await navigator.clipboard.writeText(text);
    toast.success(what);
  } catch {
    toast.error("Could not copy to the clipboard");
  }
}

/** Jobs drawn as stages left to right (prepare → check → test → scan), like a GitLab pipeline. */
export function PipelineGraph({ jobs, selectedId, onSelect }: { jobs: CiJob[]; selectedId: number | null; onSelect: (id: number) => void }) {
  const groups = groupStages(jobs);
  const missing = jobs.filter((j) => j.status === "not_installed");
  const missingTools = [...new Set(missing.map(missingTool))];

  return (
    <div className="shrink-0 border-b">
      <div className="flex items-start gap-2.5 overflow-x-auto px-4 py-3" role="list" aria-label="Pipeline stages">
        {groups.map((g, i) => (
          <Fragment key={g.stage}>
            {i > 0 && (
              <span aria-hidden className="self-center pt-4 font-mono text-line">
                →
              </span>
            )}
            <div role="listitem" aria-label={`${g.label} stage`} className="grid min-w-[150px] flex-1 content-start gap-1.5">
              <span className="font-mono text-[10.5px] font-medium uppercase tracking-[0.07em] text-muted-foreground">{g.label}</span>
              {g.jobs.map((j) => (
                <JobPill key={j.id} job={j} on={j.id === selectedId} onClick={() => onSelect(j.id)} />
              ))}
            </div>
          </Fragment>
        ))}
      </div>
      {missingTools.map((tool) => (
        <div key={tool} className="flex items-center gap-2 border-t border-dashed px-4 py-1.5 text-xs">
          <span aria-hidden className="inline-block h-2 w-2 shrink-0 rounded-full bg-orange" />
          <span className="min-w-0 flex-1 truncate">
            <b className="font-mono font-medium">{tool}</b> is not installed, so it is skipped on every run.{" "}
            <span className="font-mono text-muted-foreground">{installHint(tool)}</span>
          </span>
          <Button size="sm" onClick={() => copyText(installHint(tool), "Install command copied")}>
            <Copy /> Copy install command
          </Button>
        </div>
      ))}
    </div>
  );
}

function JobPill({ job, on, onClick }: { job: CiJob; on: boolean; onClick: () => void }) {
  const missing = job.status === "not_installed";
  const muted = missing || job.status === "skipped" || job.status === "cancelled";
  return (
    <button
      type="button"
      aria-pressed={on}
      onClick={onClick}
      title={missing ? installHint(missingTool(job)) : job.command}
      className={cn(
        "flex min-w-0 items-center gap-2 rounded-lg border bg-card px-2.5 py-[7px] text-left text-xs transition-shadow hover:border-brand/60",
        muted && "border-dashed bg-transparent text-muted-foreground",
        on && "border-brand shadow-[0_0_0_3px_hsl(var(--brand-soft))]",
      )}
    >
      <JobStatusIcon status={job.status} />
      <span className="min-w-0 truncate font-medium">{job.name}</span>
      <small className="ml-auto shrink-0 font-mono text-2xs text-muted-foreground">{ciJobPill(job)}</small>
    </button>
  );
}
