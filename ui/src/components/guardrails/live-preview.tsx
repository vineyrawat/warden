// Live preview (8C): findings from recent reviews re-judged under the draft settings, marked
// would post / would draft / would drop, with counts on top. Re-runs ~300ms after a control changes.
import { useMemo, useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Tip } from "@/components/ui/tooltip";
import { SeverityPill } from "@/components/pills";
import { Segmented } from "@/components/segmented";
import type { ProjectOverrides } from "@/lib/api";
import { errorMessage, useSimulateGuardrails } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { useDebounced } from "./bits";
import { asOutcome, mrRef, outcomeIndex, reasonTag, sortSimRows, summarizeSimulation, type Outcome } from "./logic";

const LIMIT_RUNS = 10;
const OUTCOME_TONE = { post: "green", draft: "blue", drop: "grey" } as const;
const OUTCOME_WORD: Record<Outcome, string> = { post: "would post", draft: "would draft", drop: "would drop" };

type Show = "all" | "changed" | Outcome;

export function LivePreview({ projectId, overrides, savedOverrides, dirty }: { projectId: number; overrides: ProjectOverrides; savedOverrides: ProjectOverrides; dirty: boolean }) {
  const debounced = useDebounced(overrides, 300);
  const sim = useSimulateGuardrails(projectId, debounced, LIMIT_RUNS);
  const saved = useSimulateGuardrails(projectId, savedOverrides, LIMIT_RUNS);
  const [show, setShow] = useState<Show>("all");

  const rows = useMemo(() => sortSimRows(sim.data ?? []), [sim.data]);
  const summary = useMemo(() => summarizeSimulation(rows), [rows]);
  const before = useMemo(() => (dirty ? outcomeIndex(saved.data) : new Map<number, Outcome>()), [dirty, saved.data]);
  const changed = rows.filter((r) => {
    const b = before.get(r.finding.id);
    return b != null && b !== asOutcome(r.outcome);
  }).length;
  const visible = rows.filter((r) => {
    if (show === "all") return true;
    if (show === "changed") {
      const b = before.get(r.finding.id);
      return b != null && b !== asOutcome(r.outcome);
    }
    return asOutcome(r.outcome) === show;
  });
  const stale = sim.isFetching || JSON.stringify(debounced) !== JSON.stringify(overrides);

  return (
    <section aria-labelledby="g-preview" className="flex min-w-0 flex-col gap-2 rounded-lg border bg-card px-3.5 py-3">
      <div className="flex flex-wrap items-center gap-1.5">
        <h3 id="g-preview" className="mr-auto text-[12.5px] font-semibold">
          {summary.runs ? `Last ${summary.runs} review${summary.runs === 1 ? "" : "s"} with these settings` : "Preview"}
        </h3>
        <span className={cn("text-2xs text-muted-foreground transition-opacity", stale ? "opacity-100" : "opacity-0")} aria-live="polite">
          {stale ? "Updating…" : ""}
        </span>
        <Badge tone="green">{summary.post} post</Badge>
        <Badge tone="blue">{summary.draft} draft</Badge>
        <Badge tone="grey">{summary.drop} drop</Badge>
      </div>

      {summary.dropReasons.length > 0 && (
        <div className="text-2xs text-muted-foreground">
          Dropped for {summary.dropReasons.map((d) => `${d.tag} (${d.count})`).join(", ")}
          {dirty && changed > 0 && (
            <span className="text-foreground">
              {" "}
              · {changed} finding{changed === 1 ? "" : "s"} change outcome vs saved
            </span>
          )}
        </div>
      )}

      {rows.length > 0 && (
        <Segmented<Show>
          label="Show findings"
          value={show}
          onChange={setShow}
          className="self-start"
          options={[
            { value: "all", label: `All ${rows.length}` },
            { value: "post", label: "Post" },
            { value: "draft", label: "Draft" },
            { value: "drop", label: "Drop" },
            ...(dirty ? [{ value: "changed" as const, label: `Changed ${changed}` }] : []),
          ]}
        />
      )}

      {sim.error ? (
        <div className="rounded-md bg-red-soft px-2.5 py-1.5 text-xs text-red">{errorMessage(sim.error)}</div>
      ) : sim.isLoading ? (
        <div className="py-8 text-center text-xs text-muted-foreground">Simulating recent reviews…</div>
      ) : rows.length === 0 ? (
        <div className="py-8 text-center text-xs text-muted-foreground">No reviews with findings yet. The preview fills in after this project's first review.</div>
      ) : (
        <ul className={cn("max-h-[460px] divide-y overflow-y-auto rounded-md border transition-opacity", stale && "opacity-70")}>
          {visible.length === 0 && <li className="px-2.5 py-4 text-center text-xs text-muted-foreground">Nothing here.</li>}
          {visible.map((r) => {
            const o = asOutcome(r.outcome);
            const was = before.get(r.finding.id);
            const moved = was != null && was !== o;
            return (
              <li key={r.finding.id} className={cn("flex items-center gap-2 px-2.5 py-1.5 text-xs", o === "drop" && "opacity-60", moved && "bg-brand-soft/40")}>
                <SeverityPill severity={r.finding.severity} />
                <span className="min-w-0 flex-1 truncate" title={`${r.finding.title} · ${r.mr_label}`}>
                  {r.finding.title} <span className="text-muted-foreground">· {mrRef(r.mr_label)}</span>
                </span>
                {moved && <span className="shrink-0 text-2xs text-muted-foreground">was {was}</span>}
                <span className="w-8 shrink-0 text-right font-mono text-2xs text-muted-foreground">{r.finding.confidence.toFixed(2)}</span>
                <Tip content={r.reason || OUTCOME_WORD[o]}>
                  <span tabIndex={0} className="shrink-0 rounded-full">
                    <Badge tone={OUTCOME_TONE[o]} aria-label={`${OUTCOME_WORD[o]}${r.reason ? `: ${r.reason}` : ""}`}>
                      {o === "drop" ? `drop: ${reasonTag(r.reason)}` : o}
                    </Badge>
                  </span>
                </Tip>
              </li>
            );
          })}
        </ul>
      )}
      <p className="text-2xs text-muted-foreground">Recent findings re-checked with the settings on the left. Nothing is posted from here.</p>
    </section>
  );
}
