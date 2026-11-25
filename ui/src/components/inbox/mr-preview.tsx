// Right column of the Inbox: preview of the selected MR (UX 3C) with 3B's "Approve N & post".
import { useEffect, useState } from "react";
import { Check, CornerDownLeft, ExternalLink, Loader2, Play, RotateCw, Square, X } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ErrorBox, Loading, RichText } from "@/components/layout";
import { RunStatusPill } from "@/components/pills";
import type { Finding } from "@/bindings/Finding";
import type { MrRow } from "@/bindings/MrRow";
import type { RunDetail } from "@/bindings/RunDetail";
import { capitalize, ciJobPill, ciStatusLabel, formatCount, isRunActive, runStatusLabel } from "@/lib/format";
import { useNav } from "@/lib/nav";
import { useCancelRun, usePostFindings, useRun, useRunProgress, useSetFindingStatus } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { JobCell, SevChip } from "./bits";
import { approvePostPlan, basename, findingsBySeverity, reviewActionLabel, verdictMeta } from "./inbox-logic";

export function MrPreview({
  row,
  maxTurns,
  postingPaused,
  forgeName,
  onOpen,
  onReview,
  starting,
}: {
  row: MrRow;
  maxTurns: number;
  postingPaused: boolean;
  forgeName: string;
  onOpen: () => void;
  onReview: () => void;
  starting: boolean;
}) {
  const run = row.latest_run;
  const detail = useRun(run?.id);
  const active = !!run && isRunActive(run.status);
  const verdict = verdictMeta(run?.verdict);

  return (
    <div className="flex max-w-[900px] flex-col gap-3 px-5 py-4">
      <div className="flex items-start gap-2">
        <span className="pt-px font-mono text-[13px] text-muted-foreground">{row.iid ? `!${row.iid}` : "local"}</span>
        <h2 className="min-w-0 flex-1 text-[15px] font-semibold leading-snug">{row.title}</h2>
        {verdict && !active ? <Badge tone={verdict.tone}>{verdict.label}</Badge> : run && <RunStatusPill status={run.status} />}
      </div>
      <div className="-mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 font-mono text-2xs text-muted-foreground">
        <span>{row.project_name}</span>·<span>{row.author}</span>·
        <span>
          {row.source_branch} → {row.target_branch}
        </span>
        ·
        <span>
          <span className="text-green">+{formatCount(row.additions)}</span> <span className="text-red">−{formatCount(row.deletions)}</span>
        </span>
        {row.draft && <Badge tone="grey">draft</Badge>}
      </div>

      {!run ? (
        <NoRun row={row} forgeName={forgeName} onReview={onReview} starting={starting} />
      ) : (
        <>
          {active && <LiveProgress runId={run.id} status={run.status} turns={run.turns} maxTurns={maxTurns} />}
          {run.status === "failed" && <ErrorBox error={run.error ?? "The review failed."} />}
          {run.status === "skipped" && <p className="text-[12.5px] text-muted-foreground">Skipped{run.error ? `: ${run.error}` : ""}. Nothing was reviewed.</p>}
          {run.summary && <p className="text-[12.5px] leading-relaxed text-muted-foreground">{run.summary}</p>}
          {detail.isLoading ? (
            <Loading />
          ) : detail.error ? (
            <ErrorBox error={detail.error} />
          ) : detail.data ? (
            <RunBody
              row={row}
              detail={detail.data}
              active={active}
              postingPaused={postingPaused}
              forgeName={forgeName}
              onOpen={onOpen}
              onReview={onReview}
              starting={starting}
            />
          ) : null}
        </>
      )}
    </div>
  );
}

function NoRun({ row, forgeName, onReview, starting }: { row: MrRow; forgeName: string; onReview: () => void; starting: boolean }) {
  return (
    <>
      <p className="text-[12.5px] text-muted-foreground">
        {row.draft ? "Draft MRs are not reviewed automatically." : "Not reviewed yet. It is reviewed on the next poll, or start it now."}
      </p>
      <div className="flex items-center gap-2">
        <ForgeLink row={row} forgeName={forgeName} />
        <span className="flex-1" />
        <Button variant="default" onClick={onReview} disabled={starting}>
          <Play /> {reviewActionLabel(row)}
          <Kbd>R</Kbd>
        </Button>
      </div>
    </>
  );
}

function LiveProgress({ runId, status, turns, maxTurns }: { runId: number; status: RunDetail["run"]["status"]; turns: number; maxTurns: number }) {
  const live = useRunProgress(runId);
  const cancel = useCancelRun();
  const turn = live?.turn ?? turns;
  const reviewing = status === "reviewing";
  return (
    <div className="flex flex-col gap-1.5 rounded-lg border bg-brand-soft px-3 py-2 text-xs text-primary" role="status" aria-live="polite">
      <div className="flex items-center gap-2">
        <Loader2 className="size-3.5 animate-spin" aria-hidden />
        <span className="font-semibold">{reviewing ? "Claude is reviewing" : runStatusLabel(status)}</span>
        {reviewing && (
          <span className="font-mono">
            turn {turn} / {maxTurns}
          </span>
        )}
        <span className="flex-1" />
        <Button size="sm" variant="outline-danger" onClick={() => cancel.mutate(runId)} disabled={cancel.isPending}>
          <Square /> Cancel
        </Button>
      </div>
      {reviewing && (
        <div className="h-1 overflow-hidden rounded bg-surface" aria-hidden>
          <i className="block h-full bg-primary transition-[width]" style={{ width: `${Math.min(100, (turn / Math.max(1, maxTurns)) * 100)}%` }} />
        </div>
      )}
      <span className="truncate font-mono text-2xs">{live?.message ?? "waiting for the next step…"}</span>
    </div>
  );
}

function RunBody({
  row,
  detail,
  active,
  postingPaused,
  forgeName,
  onOpen,
  onReview,
  starting,
}: {
  row: MrRow;
  detail: RunDetail;
  active: boolean;
  postingPaused: boolean;
  forgeName: string;
  onOpen: () => void;
  onReview: () => void;
  starting: boolean;
}) {
  const { navigate } = useNav();
  const { run, findings, ci_jobs } = detail;
  const groups = findingsBySeverity(findings);
  const isLocal = !row.web_url || row.project_id === 0;

  return (
    <>
      {groups.length > 0 ? (
        <div className="flex flex-col gap-2">
          {groups.map((g) => (
            <section key={g.severity} aria-label={`${capitalize(g.severity)} findings`}>
              <h3 className="mb-1 text-2xs font-semibold uppercase tracking-[.04em] text-muted-foreground">
                {capitalize(g.severity)} · {g.findings.length}
              </h3>
              <div className="overflow-hidden rounded-lg border bg-card">
                {g.findings.map((f) => (
                  <FindingRow key={f.id} f={f} runId={run.id} onOpen={() => navigate({ screen: "review", runId: run.id, findingId: f.id })} locked={active} />
                ))}
              </div>
            </section>
          ))}
        </div>
      ) : (
        !active && run.status !== "failed" && run.status !== "skipped" && <p className="text-[12.5px] text-muted-foreground">No findings.</p>
      )}

      {ci_jobs.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5" aria-label="CI jobs">
          {ci_jobs.map((j) => (
            <JobCell
              key={j.id}
              status={j.status}
              title={`${j.name}: ${ciStatusLabel(j.status)} · open in CI runs`}
              onClick={() => navigate({ screen: "ci", runId: run.id, jobId: j.id })}
            >
              {j.name} {ciJobPill(j)}
            </JobCell>
          ))}
        </div>
      )}

      <Actions
        row={row}
        detail={detail}
        active={active}
        isLocal={isLocal}
        postingPaused={postingPaused}
        forgeName={forgeName}
        onOpen={onOpen}
        onReview={onReview}
        starting={starting}
      />
    </>
  );
}

function FindingRow({ f, runId, onOpen, locked }: { f: Finding; runId: number; onOpen: () => void; locked: boolean }) {
  const set = useSetFindingStatus();
  const decided = f.status === "posted" || f.status === "resolved";
  const toggle = (status: "approved" | "dismissed") => set.mutate({ id: f.id, runId, status: f.status === status ? "draft" : status });
  return (
    <div className={cn("flex items-start gap-2 border-t px-3 py-2 first:border-t-0", f.status === "dismissed" && "opacity-55")}>
      <SevChip severity={f.severity} className="mt-[3px]" />
      <div className="min-w-0 flex-1">
        <button type="button" onClick={onOpen} className="block max-w-full text-left text-[12.5px] font-semibold hover:underline" title="Open in the full review">
          <RichText text={f.title} />
        </button>
        <div className="truncate font-mono text-2xs text-muted-foreground" title={`${f.file}:${f.line}`}>
          {basename(f.file)}:{f.line}
          {f.rule_id && ` · ${f.rule_id}`}
          {f.status === "rejected" && f.guardrail_reason && ` · held back: ${f.guardrail_reason}`}
        </div>
      </div>
      {decided ? (
        <Badge tone="green">{f.status}</Badge>
      ) : (
        <div className="flex shrink-0 items-center gap-1">
          <Button
            size="sm"
            variant="outline"
            aria-pressed={f.status === "approved"}
            disabled={set.isPending || locked}
            onClick={() => toggle("approved")}
            className={cn(f.status === "approved" && "border-green bg-green-soft text-green hover:bg-green-soft")}
          >
            <Check /> {f.status === "approved" ? "Approved" : "Approve"}
          </Button>
          <Button
            size="sm"
            variant="outline"
            aria-pressed={f.status === "dismissed"}
            disabled={set.isPending || locked}
            onClick={() => toggle("dismissed")}
            className={cn(f.status === "dismissed" && "bg-surface-2")}
          >
            <X /> {f.status === "dismissed" ? "Dismissed" : "Dismiss"}
          </Button>
        </div>
      )}
    </div>
  );
}

function Actions({
  row,
  detail,
  active,
  isLocal,
  postingPaused,
  forgeName,
  onOpen,
  onReview,
  starting,
}: {
  row: MrRow;
  detail: RunDetail;
  active: boolean;
  isLocal: boolean;
  postingPaused: boolean;
  forgeName: string;
  onOpen: () => void;
  onReview: () => void;
  starting: boolean;
}) {
  const runId = detail.run.id;
  const plan = approvePostPlan(detail.findings);
  const n = plan.post.length;
  const setStatus = useSetFindingStatus();
  const post = usePostFindings();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  // A different MR/run in the preview cancels a pending confirmation.
  useEffect(() => setConfirming(false), [runId]);

  const approveAndPost = async () => {
    setBusy(true);
    try {
      for (const id of plan.approve) await setStatus.mutateAsync({ id, runId, status: "approved" });
      await post.mutateAsync({ runId, ids: plan.post });
      setConfirming(false);
    } catch {
      // the mutation hooks already show the error toast
    } finally {
      setBusy(false);
    }
  };

  const postLabel = plan.approve.length > 0 ? `Approve ${n} & post` : `Post ${n} approved`;
  const disabledReason = isLocal
    ? "Local review: nothing is posted"
    : postingPaused
      ? "Posting is paused"
      : active
        ? "Wait for the review to finish"
        : null;
  const target = row.iid ? `!${row.iid}` : "this MR";

  if (confirming)
    return (
      <div className="flex flex-wrap items-center gap-2 rounded-lg border border-orange/40 bg-orange-soft px-3 py-2 text-xs" role="alertdialog" aria-label="Confirm posting">
        <span className="min-w-0 flex-1">
          Post {n} note{n === 1 ? "" : "s"} to {target} on {forgeName}?
          {plan.approve.length > 0 && ` ${plan.approve.length} undecided finding${plan.approve.length === 1 ? " is" : "s are"} approved first.`} They appear on the MR right away.
        </span>
        <Button size="sm" onClick={() => setConfirming(false)} disabled={busy}>
          Cancel
        </Button>
        <Button size="sm" variant="default" onClick={approveAndPost} disabled={busy} autoFocus>
          {busy && <Loader2 className="animate-spin" />} Post {n} note{n === 1 ? "" : "s"}
        </Button>
      </div>
    );

  return (
    <div className="flex flex-wrap items-center gap-2 pt-1">
      <ForgeLink row={row} forgeName={forgeName} />
      <span className="flex-1" />
      {!active && (
        <Button onClick={onReview} disabled={starting} title="R">
          <RotateCw /> {reviewActionLabel(row)}
        </Button>
      )}
      <Button onClick={onOpen}>
        Open review <CornerDownLeft />
      </Button>
      {n > 0 && (
        <Button variant="default" onClick={() => setConfirming(true)} disabled={!!disabledReason} title={disabledReason ?? undefined}>
          {postLabel}
        </Button>
      )}
      {n > 0 && disabledReason && <span className="w-full text-right text-2xs text-muted-foreground">{disabledReason}</span>}
    </div>
  );
}

function ForgeLink({ row, forgeName = "forge" }: { row: MrRow; forgeName?: string }) {
  if (!row.web_url) return null;
  return (
    <a href={row.web_url} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 rounded text-xs text-primary hover:underline">
      <ExternalLink className="size-3" /> Open on {forgeName}
    </a>
  );
}

function Kbd({ children }: { children: React.ReactNode }) {
  return <kbd className="ml-1 rounded border border-current px-1 font-mono text-[10px] leading-[1.4] opacity-80">{children}</kbd>;
}
