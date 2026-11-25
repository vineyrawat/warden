import { ArrowRight, Copy } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { RunStatusPill } from "@/components/pills";
import type { Finding } from "@/bindings/Finding";
import type { MergeRequest } from "@/bindings/MergeRequest";
import type { ReviewRun } from "@/bindings/ReviewRun";
import type { RuleConfig } from "@/bindings/RuleConfig";
import { findingsToMarkdown, formatDuration, formatRelative, shortSha } from "@/lib/format";
import { useNav } from "@/lib/nav";
import { cn } from "@/lib/utils";
import { plural, verdictInfo } from "./logic";

function Stat({ label, children, mono }: { label: string; children: React.ReactNode; mono?: boolean }) {
  return (
    <div className="grid gap-0.5">
      <dt className="text-2xs text-muted-foreground">{label}</dt>
      <dd className={cn("truncate text-[12.5px]", mono && "font-mono")}>{children}</dd>
    </div>
  );
}

/** Verdict, Claude's summary note, run stats, rules checked and a link to the audit trail. */
export function SummaryTab({
  run,
  mr,
  findings,
  rules,
  liveTurn,
  maxTurns,
  active,
}: {
  run: ReviewRun;
  mr: MergeRequest;
  findings: Finding[];
  /** Enabled rules from the project config; undefined when unknown (local review, still loading). */
  rules?: RuleConfig[];
  liveTurn?: number;
  maxTurns: number;
  active: boolean;
}) {
  const { navigate } = useNav();
  const verdict = verdictInfo(run.verdict);
  const fired = new Set(findings.map((f) => f.rule_id).filter((r): r is string => !!r));

  const copy = async () => {
    const md = findingsToMarkdown(mr, run, findings.filter((f) => f.status !== "dismissed" && f.status !== "rejected"));
    try {
      await navigator.clipboard.writeText(md);
      toast.success("Copied review as markdown");
    } catch {
      toast.error("Clipboard is not available");
    }
  };

  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto grid w-full max-w-[760px] gap-3 px-4 py-4">
        <section className="grid gap-2 rounded-xl border bg-card px-[18px] py-4">
          <div className="flex items-center gap-2">
            {verdict ? <Badge tone={verdict.tone}>{verdict.label}</Badge> : <RunStatusPill status={run.status} />}
            <b className="text-sm">Summary note</b>
          </div>
          <p className="whitespace-pre-wrap text-[13px] leading-[1.55]">
            {run.summary ?? (active ? "Claude writes the summary at the end of the review." : "No summary for this run.")}
          </p>
          {run.error && <p className="text-xs text-red">{run.error}</p>}
        </section>

        <dl className="grid grid-cols-3 gap-x-4 gap-y-3 rounded-xl border bg-card px-[18px] py-4">
          <Stat label="Turns" mono>
            {active ? `${liveTurn ?? run.turns} / ${maxTurns}` : run.turns}
          </Stat>
          <Stat label="Duration" mono>
            {active ? "running" : formatDuration(run.duration_ms)}
          </Stat>
          <Stat label="Model" mono>
            {run.model}
          </Stat>
          <Stat label="Reviewed" mono>
            {shortSha(run.since_sha ?? run.base_sha)} → {shortSha(run.head_sha)}
            {run.since_sha ? " (incremental)" : ""}
          </Stat>
          <Stat label="Started">{formatRelative(run.started_at)}</Stat>
          <Stat label="Findings">{plural(findings.length, "finding")}</Stat>
        </dl>

        <section className="grid gap-2 rounded-xl border bg-card px-[18px] py-4">
          <div className="flex items-center gap-2">
            <b className="text-[12.5px]">Rules ran</b>
            <span className="text-2xs text-muted-foreground">
              {rules ? `${plural(rules.length, "rule")} in the project config · ${fired.size} raised findings` : mr.project_id === 0 ? "Local review: repo rules only" : "Loading…"}
            </span>
          </div>
          {rules && rules.length > 0 && (
            <ul className="flex flex-wrap gap-1.5">
              {rules.map((r) => (
                <li key={r.id} title={r.title}>
                  <Badge tone={fired.has(r.id) ? "orange" : "outline"} className="font-mono font-medium">
                    {r.id}
                  </Badge>
                </li>
              ))}
            </ul>
          )}
        </section>

        <div className="flex items-center gap-2">
          <Button onClick={copy} disabled={findings.length === 0 && !run.summary}>
            <Copy /> Copy as markdown
          </Button>
          <span className="flex-1" />
          <Button variant="ghost" onClick={() => navigate({ screen: "history", runId: run.id })}>
            Open History audit <ArrowRight />
          </Button>
        </div>
      </div>
    </div>
  );
}
