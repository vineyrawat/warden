import { ArrowRight, RotateCw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { CiStatusPill } from "@/components/pills";
import type { CiJob } from "@/bindings/CiJob";
import { ciJobPill, formatDuration } from "@/lib/format";
import { useNav } from "@/lib/nav";
import { useRerunCiJob } from "@/lib/queries";
import { plural } from "./logic";

const KIND_LABEL: Record<string, string> = { recipe: "recipe", gitlab_ci_local: "gitlab-ci-local", check: "check" };

/** Compact CI job list for this run; rows open the CI screen on that job. */
export function CiTab({ runId, jobs, active }: { runId: number; jobs: CiJob[]; active: boolean }) {
  const { navigate } = useNav();
  const rerun = useRerunCiJob();
  const failed = jobs.filter((j) => j.status === "failed" || j.status === "timed_out").length;
  const total = jobs.reduce((n, j) => n + j.duration_ms, 0);

  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto grid w-full max-w-[760px] gap-3 px-4 py-4">
        <div className="flex items-center gap-3">
          <span className="min-w-0 flex-1 font-mono text-2xs text-muted-foreground">
            {jobs.length ? `${plural(jobs.length, "job")} · ${failed} failed · ${formatDuration(total)} total` : active ? "Checks have not started yet" : "No CI jobs ran for this review"}
          </span>
          <Button variant="ghost" onClick={() => navigate({ screen: "ci", runId })}>
            Open CI runs <ArrowRight />
          </Button>
        </div>
        {jobs.length > 0 && (
          <ul className="overflow-hidden rounded-xl border bg-card" aria-label="CI jobs">
            {jobs.map((j) => {
              const bad = j.status === "failed" || j.status === "timed_out";
              return (
                <li key={j.id} className="border-b last:border-b-0">
                  <div className="flex items-center gap-3 px-3 py-2 hover:bg-surface-2/60">
                    <button
                      type="button"
                      className="flex min-w-0 flex-1 items-center gap-3 rounded text-left"
                      onClick={() => navigate({ screen: "ci", runId, jobId: j.id })}
                      aria-label={`Open ${j.name} in CI runs`}
                    >
                      <CiStatusPill status={j.status} />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-[12.5px] font-medium">{j.name}</span>
                        <span className="block truncate font-mono text-2xs text-muted-foreground">
                          {KIND_LABEL[j.kind] ?? j.kind} · {j.command}
                        </span>
                        {bad && j.failure_excerpt && <span className="mt-0.5 block truncate font-mono text-2xs text-red">{j.failure_excerpt.split("\n").find((l) => l.trim()) ?? ""}</span>}
                      </span>
                      <span className="shrink-0 font-mono text-2xs text-muted-foreground">{ciJobPill(j)}</span>
                    </button>
                    {bad && (
                      <Button size="sm" onClick={() => rerun.mutate({ runId, name: j.name })} disabled={rerun.isPending}>
                        <RotateCw /> Re-run
                      </Button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}
