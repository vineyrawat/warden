import { useEffect, useMemo, useState } from "react";
import { ArrowLeft, ExternalLink, RotateCw, Square } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ErrorBox, Loading } from "@/components/layout";
import { ModePill, RunStatusPill } from "@/components/pills";
import { FindingsTab } from "@/components/review/findings-tab";
import { DiffTab } from "@/components/review/diff-tab";
import { CiTab } from "@/components/review/ci-tab";
import { SummaryTab } from "@/components/review/summary-tab";
import { PostControl, PostReportView } from "@/components/review/post-control";
import { useDecisions } from "@/components/review/use-decisions";
import {
  ciTabLabel,
  droppedFindings,
  initialFindingId,
  plural,
  postDisabledReason,
  reviewFiles,
  verdictInfo,
  walkthroughFindings,
  type ReviewTab,
} from "@/components/review/logic";
import type { Finding } from "@/bindings/Finding";
import type { PostReport } from "@/bindings/PostReport";
import type { RunDetail } from "@/bindings/RunDetail";
import { isRunActive } from "@/lib/format";
import { useNav } from "@/lib/nav";
import { useCancelRun, usePostFindings, useProjectConfig, useProjects, useRun, useRunProgress, useSettings, useStartReview, useUsage } from "@/lib/queries";
import { cn } from "@/lib/utils";

export type { ReviewTab } from "@/components/review/logic";

export function ReviewScreen({ runId, focusFindingId, tab }: { runId: number; focusFindingId?: number; tab?: ReviewTab }) {
  const { data, isLoading, error } = useRun(runId);
  if (isLoading) return <Loading label="Loading review…" />;
  if (error) return <ErrorBox error={error} />;
  if (!data) return null;
  return <ReviewBody detail={data} focusFindingId={focusFindingId} tab={tab} />;
}

function ReviewBody({ detail, focusFindingId, tab }: { detail: RunDetail; focusFindingId?: number; tab?: ReviewTab }) {
  const { navigate } = useNav();
  const { run, mr, findings, files, ci_jobs } = detail;
  const isLocal = mr.project_id === 0;
  const project = useProjects().data?.find((p) => p.id === mr.project_id);
  const config = useProjectConfig(isLocal ? null : mr.project_id).data;
  const settings = useSettings().data;
  const usage = useUsage().data;
  const live = useRunProgress(run.id);
  const cancel = useCancelRun();
  const post = usePostFindings();
  const rerun = useStartReview((id) => navigate({ screen: "review", runId: id }));

  const rules = useMemo(() => {
    if (!config) return undefined;
    const off = new Set(config.disabled_rules ?? []);
    return config.rules.filter((r) => !off.has(r.id));
  }, [config]);

  // One state per finding: approved, dismissed or draft. Applied optimistically on top of the run.
  const decisions = useDecisions(findings);
  const eff = decisions.findings;
  const list = useMemo(() => walkthroughFindings(eff), [eff]);
  const dropped = useMemo(() => droppedFindings(eff), [eff]);
  const allFiles = useMemo(() => reviewFiles(files, findings), [files, findings]);
  const approved = useMemo(() => eff.filter((f) => f.status === "approved"), [eff]);

  const [activeTab, setActiveTab] = useState<ReviewTab>(tab ?? "findings");
  useEffect(() => {
    if (tab) setActiveTab(tab);
  }, [tab]);

  const [currentId, setCurrentId] = useState<number | null>(() => initialFindingId(list, focusFindingId));
  useEffect(() => {
    // Findings stream in while Claude reviews; open the first one once it arrives.
    if (currentId == null || !list.some((f) => f.id === currentId)) {
      const next = initialFindingId(list, null);
      if (next !== currentId) setCurrentId(next);
    }
  }, [list, currentId]);
  const [diffFocus, setDiffFocus] = useState<number | null>(focusFindingId ?? null);
  // A new link to a finding of this same run (palette, queue) moves both views to it.
  useEffect(() => {
    if (focusFindingId == null) return;
    setCurrentId(focusFindingId);
    setDiffFocus(focusFindingId);
  }, [focusFindingId]);
  const [report, setReport] = useState<PostReport | null>(null);

  const active = isRunActive(run.status);
  const forgeName = project?.forge === "github" ? "GitHub" : "GitLab";
  const mrLabel = mr.iid ? `!${mr.iid}` : "Local";
  const paused = usage?.paused ?? settings?.posting_paused ?? false;
  const turn = live?.turn ?? run.turns;
  const maxTurns = settings?.max_turns ?? 30;
  const verdict = verdictInfo(run.verdict);
  const ci = ciTabLabel(ci_jobs);

  const disabledReason = postDisabledReason({ isLocal, paused, approved: approved.length, active, forgeName });
  const doPost = () =>
    post.mutate(
      { runId: run.id, ids: approved.map((f) => f.id) },
      {
        onSuccess: (r) => setReport(r),
      },
    );
  const postProps = { count: approved.length, target: `${forgeName} ${mrLabel}`, disabledReason, busy: post.isPending, onPost: doPost };
  const reportView = report && (
    <PostReportView report={report} forgeName={forgeName} onClose={() => setReport(null)} onOpenAudit={() => navigate({ screen: "history", runId: run.id })} />
  );

  const openInDiff = (f: Finding) => {
    setDiffFocus(f.id);
    setActiveTab("diff");
  };

  const tabs: { id: ReviewTab; label: string; count?: string; alert?: boolean }[] = [
    { id: "findings", label: "Findings", count: String(list.length) },
    { id: "diff", label: "Diff", count: plural(files.length, "file") },
    { id: "ci", label: "CI", count: ci.label, alert: ci.tone === "red" },
    { id: "summary", label: "Summary" },
  ];

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* header */}
      <div className="flex min-h-[41px] shrink-0 items-center gap-2.5 border-b bg-surface px-3 py-1.5">
        <Button size="icon" variant="ghost" aria-label="Back to inbox" onClick={() => navigate({ screen: "inbox" })}>
          <ArrowLeft />
        </Button>
        <span className="shrink-0 font-mono text-xs text-muted-foreground">{mrLabel}</span>
        <h1 className="min-w-0 truncate text-[13px] font-semibold">{mr.title}</h1>
        {(mr.source_branch || mr.target_branch) && (
          <span className="hidden min-w-0 shrink truncate font-mono text-2xs text-muted-foreground lg:inline">
            {mr.source_branch} → {mr.target_branch}
          </span>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-2.5 text-xs">
          {verdict && !active ? <Badge tone={verdict.tone}>{verdict.label}</Badge> : <RunStatusPill status={run.status} />}
          {mr.web_url && (
            <a href={mr.web_url} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 rounded text-primary hover:underline">
              <ExternalLink className="h-3 w-3" /> Open in {forgeName}
            </a>
          )}
          {active ? (
            <Button size="sm" variant="outline-danger" onClick={() => cancel.mutate(run.id)} disabled={cancel.isPending}>
              <Square /> Cancel
            </Button>
          ) : (
            <Button size="sm" onClick={() => rerun.mutate(mr.id)} disabled={rerun.isPending || isLocal} title={isLocal ? "Local reviews are re-run from the inbox" : "Review the MR again"}>
              <RotateCw /> Re-run
            </Button>
          )}
        </div>
      </div>

      {/* tabs */}
      <div className="flex shrink-0 items-center gap-0.5 border-b bg-surface px-3" role="tablist" aria-label="Review views">
        {tabs.map((t) => {
          const on = t.id === activeTab;
          return (
            <button
              key={t.id}
              type="button"
              role="tab"
              aria-selected={on}
              onClick={() => setActiveTab(t.id)}
              className={cn(
                "-mb-px flex items-center gap-1.5 border-b-2 px-3 py-2 text-[12.5px]",
                on ? "border-primary font-semibold text-foreground" : "border-transparent text-muted-foreground hover:text-foreground",
              )}
            >
              {t.label}
              {t.count && (
                <small className={cn("rounded-full px-1.5 font-mono text-[10.5px] font-medium", t.alert ? "bg-red-soft text-red" : "bg-surface-2 text-muted-foreground")}>
                  {t.count}
                </small>
              )}
            </button>
          );
        })}
        <span className="flex-1" />
        <div className="flex items-center gap-2 text-2xs text-muted-foreground">
          {paused && !isLocal && <Badge tone="orange">Posting paused</Badge>}
          {project && (
            <span className="flex items-center gap-1.5" title="Posting mode for this project">
              Mode <ModePill mode={project.mode} />
            </span>
          )}
        </div>
      </div>

      {active && (
        <div className="flex shrink-0 items-center gap-3 border-b bg-brand-soft px-4 py-1.5 text-xs text-primary" role="status" aria-live="polite">
          <span className="font-semibold">Claude is reviewing</span>
          <span className="font-mono">
            turn {turn} / {maxTurns}
          </span>
          <div className="h-1 w-32 overflow-hidden rounded bg-surface" aria-hidden>
            <i className="block h-full bg-primary transition-[width]" style={{ width: `${Math.min(100, (turn / maxTurns) * 100)}%` }} />
          </div>
          <span className="min-w-0 truncate font-mono">{live?.message ?? "waiting for the next tool call…"}</span>
          <span className="ml-auto shrink-0">{plural(findings.length, "finding")} so far</span>
        </div>
      )}
      {run.error && run.status === "failed" && <ErrorBox error={run.error} />}

      {activeTab === "findings" && (
        <FindingsTab
          list={list}
          dropped={dropped}
          files={allFiles}
          currentId={currentId}
          setCurrentId={setCurrentId}
          decide={decisions.decide}
          history={decisions.history}
          rules={rules}
          active={active}
          onOpenInDiff={openInDiff}
          post={<PostControl {...postProps} label={`Post ${approved.length} approved`} showReason={approved.length > 0} />}
          report={reportView}
        />
      )}
      {activeTab === "diff" && (
        <DiffTab
          files={allFiles}
          findings={list}
          focusId={diffFocus}
          setFocusId={setDiffFocus}
          decide={decisions.decide}
          rules={rules}
          active={active}
          post={<PostControl {...postProps} label={`Post ${plural(approved.length, "note")}`} inverse />}
          report={reportView}
        />
      )}
      {activeTab === "ci" && <CiTab runId={run.id} jobs={ci_jobs} active={active} />}
      {activeTab === "summary" && <SummaryTab run={run} mr={mr} findings={findings} rules={rules} liveTurn={live?.turn} maxTurns={maxTurns} active={active} />}
    </div>
  );
}
