import { useEffect, useMemo, useState } from "react";
import { ArrowDownToLine, Copy, RotateCw, Sparkles, Square } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { EmptyState, ErrorBox, Loading } from "@/components/layout";
import { JobStatusIcon, PipelineGraph, copyText } from "@/components/ci/pipeline-graph";
import { ParsedFailures } from "@/components/ci/parsed-failures";
import { LogViewer, type LogFocus } from "@/components/ci/log-viewer";
import type { CiJob } from "@/bindings/CiJob";
import type { MrRow } from "@/bindings/MrRow";
import type { ReviewRun } from "@/bindings/ReviewRun";
import type { RunDetail } from "@/bindings/RunDetail";
import { missingTool, parseFailures, summarizeLog } from "@/lib/ci-parse";
import {
  ciStatusLabel,
  classifyLogLine,
  filePath,
  formatDuration,
  formatRelative,
  installHint,
  isRunActive,
  runStatusLabel,
  shortSha,
} from "@/lib/format";
import { useNav } from "@/lib/nav";
import { useCancelRun, useCiLog, useMrs, useRerunCiJob, useRun, useRuns } from "@/lib/queries";

export function CiScreen({ initialRunId, initialJobId }: { initialRunId?: number; initialJobId?: number }) {
  const runs = useRuns(null, 50);
  const mrs = useMrs(null);
  const mrById = useMemo(() => new Map((mrs.data ?? []).map((m) => [m.id, m])), [mrs.data]);

  // Default: the most recent run that has CI jobs (known from the MR list's latest-run summary).
  const fallbackRunId = useMemo(() => {
    const withCi = (mrs.data ?? [])
      .filter((m) => m.latest_run && m.ci_summary.total > 0)
      .sort((a, b) => Date.parse(b.latest_run!.started_at) - Date.parse(a.latest_run!.started_at));
    return withCi[0]?.latest_run?.id ?? runs.data?.[0]?.id ?? null;
  }, [mrs.data, runs.data]);

  const [runId, setRunId] = useState<number | null>(initialRunId ?? null);
  useEffect(() => {
    if (initialRunId != null) setRunId(initialRunId);
  }, [initialRunId]);
  const effectiveRunId = runId ?? fallbackRunId;

  if (runs.isLoading || mrs.isLoading) return <Loading />;
  if (effectiveRunId == null)
    return (
      <div className="flex h-full flex-col">
        <CiHeader />
        <EmptyState title="No CI runs yet">CI recipes from .warden.yml run in the MR worktree when a review starts. Start a review from the Inbox to see jobs here.</EmptyState>
      </div>
    );

  return (
    <CiRunView
      key={effectiveRunId}
      runId={effectiveRunId}
      initialJobId={runId === initialRunId ? initialJobId : undefined}
      picker={<RunPicker runs={runs.data ?? []} mrById={mrById} value={effectiveRunId} onChange={setRunId} />}
    />
  );
}

function CiHeader({ picker, meta, actions }: { picker?: React.ReactNode; meta?: React.ReactNode; actions?: React.ReactNode }) {
  return (
    <div className="flex min-h-[41px] shrink-0 items-center gap-3 border-b bg-surface px-4 py-2">
      <h1 className="shrink-0 text-[13px] font-semibold">Local CI</h1>
      {picker}
      {meta && <div className="ml-auto flex min-w-0 items-center gap-3.5 text-xs text-muted-foreground">{meta}</div>}
      {actions && <div className={meta ? "flex shrink-0 items-center gap-2" : "ml-auto flex shrink-0 items-center gap-2"}>{actions}</div>}
    </div>
  );
}

function runLabel(run: ReviewRun, mr: MrRow | undefined): string {
  if (!mr) return `Run #${run.id}`;
  return `${mr.iid ? `!${mr.iid}` : "local"} ${mr.title}`;
}

function RunPicker({ runs, mrById, value, onChange }: { runs: ReviewRun[]; mrById: Map<number, MrRow>; value: number; onChange: (id: number) => void }) {
  return (
    <Select value={String(value)} onValueChange={(v) => onChange(Number(v))}>
      <SelectTrigger className="h-7 w-[380px] min-w-0 max-w-[45%] bg-card font-sans" aria-label="CI run">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {runs.map((r) => {
          const mr = mrById.get(r.mr_id);
          const ci = mr?.latest_run?.id === r.id ? mr.ci_summary : null;
          const ciText = ci ? (ci.total === 0 ? "no CI" : ci.failed ? `${ci.failed} failed` : ci.running ? "CI running" : `${ci.passed}/${ci.total} passed`) : null;
          return (
            <SelectItem key={r.id} value={String(r.id)}>
              <span className="truncate">{runLabel(r, mr)}</span>
              <span className="text-muted-foreground">
                {" "}
                · {ciText ?? runStatusLabel(r.status)} · {formatRelative(r.started_at)}
              </span>
            </SelectItem>
          );
        })}
        {!runs.some((r) => r.id === value) && <SelectItem value={String(value)}>Run #{value}</SelectItem>}
      </SelectContent>
    </Select>
  );
}

function CiRunView({ runId, initialJobId, picker }: { runId: number; initialJobId?: number; picker: React.ReactNode }) {
  const { data, isLoading, error } = useRun(runId);
  const rerun = useRerunCiJob();
  const cancel = useCancelRun();
  const jobs = data?.ci_jobs ?? [];
  // Selected by name so the selection survives a re-run (which can replace the job row).
  const [selName, setSelName] = useState<string | null>(null);
  const defaultJob =
    jobs.find((j) => j.id === initialJobId) ??
    jobs.find((j) => j.status === "failed" || j.status === "timed_out") ??
    jobs.find((j) => j.status === "running") ??
    jobs[0];
  const job = jobs.find((j) => j.name === selName) ?? defaultJob;

  if (error) return <ErrorBox error={error} />;
  if (isLoading || !data) return <Loading />;
  const { mr, run } = data;
  const failed = jobs.filter((j) => j.status === "failed" || j.status === "timed_out");
  const anyRunning = jobs.some((j) => j.status === "running" || j.status === "pending");
  const total = jobs.reduce((s, j) => s + j.duration_ms, 0);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <CiHeader
        picker={picker}
        meta={
          <span className="truncate font-mono text-[11.5px]" title={mr.source_branch}>
            {shortSha(run.head_sha)}
            {total > 0 && ` · ${formatDuration(total)}`}
          </span>
        }
        actions={
          <>
            <Button disabled={failed.length === 0 || rerun.isPending} onClick={() => failed.forEach((j) => rerun.mutate({ runId, name: j.name }))}>
              <RotateCw /> Re-run failed
            </Button>
            {(anyRunning || isRunActive(run.status)) && (
              <Button variant="outline-danger" disabled={cancel.isPending} onClick={() => cancel.mutate(runId)}>
                <Square /> Stop
              </Button>
            )}
          </>
        }
      />
      {jobs.length === 0 ? (
        <NoCi data={data} />
      ) : (
        <>
          <PipelineGraph jobs={jobs} selectedId={job?.id ?? null} onSelect={(id) => setSelName(jobs.find((j) => j.id === id)?.name ?? null)} />
          {job ? (
            <JobPanel
              key={job.id}
              job={job}
              data={data}
              rerunning={rerun.isPending}
              onRerun={() => rerun.mutate({ runId, name: job.name })}
            />
          ) : (
            <EmptyState title="Select a job" />
          )}
        </>
      )}
    </div>
  );
}

function NoCi({ data }: { data: RunDetail }) {
  const { run } = data;
  if (isRunActive(run.status) && (run.status === "queued" || run.status === "preparing"))
    return <EmptyState title="Waiting for the worktree">CI jobs start once the MR worktree is ready.</EmptyState>;
  if (run.status === "skipped")
    return <EmptyState title="No CI for this run">The review was skipped{run.error ? `: ${run.error}` : ""}, so no CI jobs ran.</EmptyState>;
  return (
    <EmptyState title="No CI for this run">
      This project has no CI recipes. Add a <code className="font-mono">ci:</code> section to .warden.yml (for example lint and unit tests) and they run in the MR worktree on the next review.
    </EmptyState>
  );
}

function JobPanel({ job, data, rerunning, onRerun }: { job: CiJob; data: RunDetail; rerunning: boolean; onRerun: () => void }) {
  const { navigate } = useNav();
  const missing = job.status === "not_installed";
  const running = job.status === "running";
  const log = useCiLog(missing ? null : job.id);
  const text = log.data ?? "";
  const lines = useMemo(() => (text ? text.split("\n") : []), [text]);
  const firstError = useMemo(() => lines.findIndex((l) => classifyLogLine(l) === "error"), [lines]);
  const failures = useMemo(() => parseFailures(text || job.failure_excerpt), [text, job.failure_excerpt]);
  const diffPaths = useMemo(() => data.files.map(filePath), [data.files]);
  const summary = summarizeLog(text);
  const [raw, setRaw] = useState(false);
  const [focus, setFocus] = useState<LogFocus | null>(null);
  const ciFinding = data.findings.find((f) => f.source === "ci" && f.status !== "dismissed");
  const failedJob = job.status === "failed" || job.status === "timed_out";

  if (missing) {
    const tool = missingTool(job);
    return (
      <div className="flex min-h-0 flex-1 flex-col">
        <JobRow job={job} label={`${job.name} · not installed`} actions={<RerunButton disabled={rerunning} onClick={onRerun} />} />
        <EmptyState title={`${tool} is not installed`}>
          <p>This job is skipped until {tool} is on PATH. Install it, then re-run the job.</p>
          <div className="mt-3 flex items-center justify-center gap-2">
            <code className="rounded-[5px] border bg-surface-2 px-2 py-1 font-mono text-xs text-foreground">{installHint(tool)}</code>
            <Button size="sm" onClick={() => copyText(installHint(tool), "Install command copied")}>
              <Copy /> Copy
            </Button>
          </div>
        </EmptyState>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <JobRow
        job={job}
        label={`${job.name} · ${summary ?? (running ? "running" : ciStatusLabel(job.status))}`}
        actions={
          <>
            <Button disabled={firstError < 0} onClick={() => setFocus((f) => ({ line: firstError, n: (f?.n ?? 0) + 1 }))}>
              <ArrowDownToLine /> Jump to first error
            </Button>
            <Button variant="ghost" aria-pressed={raw} className={raw ? "bg-accent" : undefined} onClick={() => setRaw((v) => !v)}>
              Raw log
            </Button>
            <RerunButton disabled={running || rerunning} onClick={onRerun} />
          </>
        }
      />
      <ParsedFailures failures={failures} diffPaths={diffPaths} runId={data.run.id} />
      {failedJob && ciFinding && (
        <div className="mx-4 mt-2 flex shrink-0 items-center gap-2 rounded-lg border bg-card px-3 py-2 text-xs">
          <Badge tone="blue">
            <Sparkles className="h-3 w-3" /> Claude
          </Badge>
          <span className="min-w-0 flex-1 truncate">
            Likely cause: <b className="font-semibold">{ciFinding.title}</b>
          </span>
          <Button size="sm" onClick={() => navigate({ screen: "review", runId: data.run.id, findingId: ciFinding.id })}>
            Open finding on {ciFinding.file.split("/").pop()}:{ciFinding.line}
          </Button>
        </div>
      )}
      <div className="flex min-h-0 flex-1 flex-col px-4 pb-3 pt-3">
        <LogViewer
          lines={lines}
          loading={log.isLoading}
          running={running}
          raw={raw}
          focus={focus}
          emptyText={job.status === "pending" ? "Not started yet." : job.status === "skipped" ? `Skipped${job.failure_excerpt ? `: ${job.failure_excerpt}` : "."}` : undefined}
        />
      </div>
    </div>
  );
}

function JobRow({ job, label, actions }: { job: CiJob; label: string; actions: React.ReactNode }) {
  return (
    <div className="flex shrink-0 items-center gap-2 px-4 pt-3">
      <JobStatusIcon status={job.status} />
      <b className="shrink-0 text-[13px] font-semibold">{label}</b>
      <span className="min-w-0 flex-1 truncate font-mono text-2xs text-muted-foreground" title={job.command}>
        {job.duration_ms > 0 && job.status !== "running" ? `${formatDuration(job.duration_ms)} · ` : ""}
        {job.command}
      </span>
      <div className="flex shrink-0 items-center gap-1.5">{actions}</div>
    </div>
  );
}

function RerunButton({ disabled, onClick }: { disabled: boolean; onClick: () => void }) {
  return (
    <Button disabled={disabled} onClick={onClick}>
      <RotateCw /> Re-run job
    </Button>
  );
}
