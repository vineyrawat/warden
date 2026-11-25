// TanStack Query hooks over api.ts, plus the event bridge that invalidates them on AppEvents.
import { useEffect, useSyncExternalStore } from "react";
import { useMutation, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import type { AppEvent } from "@/bindings/AppEvent";
import type { FindingStatus } from "@/bindings/FindingStatus";
import * as api from "./api";

export const qk = {
  engine: ["engine"] as const,
  tools: ["tools"] as const,
  usage: ["usage"] as const,
  settings: ["settings"] as const,
  forges: ["forges"] as const,
  projects: ["projects"] as const,
  projectConfig: (id: number) => ["project-config", id] as const,
  mrs: (projectId: number | null) => ["mrs", projectId] as const,
  run: (id: number) => ["run", id] as const,
  runs: (mrId: number | null, limit: number) => ["runs", mrId, limit] as const,
  runRows: (limit: number) => ["run-rows", limit] as const,
  ciLog: (jobId: number) => ["ci-log", jobId] as const,
  audit: (runId: number | null, limit: number) => ["audit", runId, limit] as const,
  stats: (days: number) => ["stats", days] as const,
  ruleStats: (projectId: number | null, days: number) => ["rule-stats", projectId, days] as const,
  simulate: (projectId: number, overridesJson: string, limitRuns: number) =>
    ["simulate-guardrails", projectId, overridesJson, limitRuns] as const,
  previewNote: (findingId: number) => ["preview-note", findingId] as const,
  pendingFindings: ["pending-findings"] as const,
};

// ---- live progress store (run_progress events are transient, not refetchable) -----------------

export type RunProgress = { turn: number; message: string; at: number };
const progress = new Map<number, RunProgress>();
const progressListeners = new Set<() => void>();
let progressVersion = 0;

function setProgress(runId: number, p: RunProgress) {
  progress.set(runId, p);
  progressVersion++;
  progressListeners.forEach((l) => l());
}

export function useRunProgress(runId: number | null | undefined): RunProgress | undefined {
  useSyncExternalStore(
    (cb) => {
      progressListeners.add(cb);
      return () => progressListeners.delete(cb);
    },
    () => progressVersion,
  );
  return runId == null ? undefined : progress.get(runId);
}

// ---- event bridge ----------------------------------------------------------------------------

export function handleEvent(qc: QueryClient, e: AppEvent) {
  switch (e.type) {
    case "run_updated":
      qc.invalidateQueries({ queryKey: qk.run(e.run.id) });
      qc.invalidateQueries({ queryKey: qk.pendingFindings });
      qc.invalidateQueries({ queryKey: ["stats"] });
      qc.invalidateQueries({ queryKey: ["run-rows"] });
      qc.invalidateQueries({ queryKey: ["runs"] });
      qc.invalidateQueries({ queryKey: ["mrs"] });
      qc.invalidateQueries({ queryKey: qk.usage });
      qc.invalidateQueries({ queryKey: ["audit"] });
      break;
    case "run_progress":
      setProgress(e.run_id, { turn: e.turn, message: e.message, at: Date.now() });
      break;
    case "ci_log":
      // Append only to a log we already hold; otherwise the next read_ci_log fetch returns it in full.
      if (qc.getQueryData<string>(qk.ciLog(e.job_id)) !== undefined)
        qc.setQueryData<string>(qk.ciLog(e.job_id), (old) => (old ? `${old}\n${e.line}` : e.line));
      break;
    case "ci_job_updated":
      qc.invalidateQueries({ queryKey: qk.run(e.job.run_id) });
      if (e.job.status !== "running") qc.invalidateQueries({ queryKey: qk.ciLog(e.job.id) });
      qc.invalidateQueries({ queryKey: ["run-rows"] });
      qc.invalidateQueries({ queryKey: ["mrs"] });
      break;
    case "findings_updated":
      qc.invalidateQueries({ queryKey: qk.run(e.run_id) });
      qc.invalidateQueries({ queryKey: qk.pendingFindings });
      qc.invalidateQueries({ queryKey: ["preview-note"] });
      qc.invalidateQueries({ queryKey: ["stats"] });
      qc.invalidateQueries({ queryKey: ["rule-stats"] });
      qc.invalidateQueries({ queryKey: ["run-rows"] });
      qc.invalidateQueries({ queryKey: ["mrs"] });
      qc.invalidateQueries({ queryKey: ["audit"] });
      break;
    case "mrs_updated":
      qc.invalidateQueries({ queryKey: ["mrs"] });
      break;
    case "notice": {
      const fn = e.level === "error" ? toast.error : e.level === "warn" ? toast.warning : toast.info;
      fn(e.message);
      break;
    }
  }
}

/** Subscribe once (App root) to backend events and keep the query cache fresh. */
export function useEventBridge(onAny?: (e: AppEvent) => void) {
  const qc = useQueryClient();
  useEffect(
    () =>
      api.onEvent((e) => {
        handleEvent(qc, e);
        onAny?.(e);
      }),
    [qc, onAny],
  );
}

// ---- queries ---------------------------------------------------------------------------------

export const useEngineStatus = () => useQuery({ queryKey: qk.engine, queryFn: api.get_engine_status, staleTime: 60_000 });
export const useTools = () => useQuery({ queryKey: qk.tools, queryFn: api.detect_tools, staleTime: 5 * 60_000 });
export const useUsage = () => useQuery({ queryKey: qk.usage, queryFn: api.get_usage, refetchInterval: 30_000 });
export const useSettings = () => useQuery({ queryKey: qk.settings, queryFn: api.get_settings });
export const useForges = () => useQuery({ queryKey: qk.forges, queryFn: api.forge_status, staleTime: 60_000 });
export const useProjects = () => useQuery({ queryKey: qk.projects, queryFn: api.list_projects });
export const useProjectConfig = (id: number | null) =>
  useQuery({ queryKey: qk.projectConfig(id ?? -1), queryFn: () => api.get_project_config(id!), enabled: id != null });
export const useMrs = (projectId: number | null) =>
  useQuery({ queryKey: qk.mrs(projectId), queryFn: () => api.list_mrs(projectId) });
export const useRun = (runId: number | null | undefined) =>
  useQuery({ queryKey: qk.run(runId ?? -1), queryFn: () => api.get_run(runId!), enabled: runId != null });
export const useRuns = (mrId: number | null, limit = 100) =>
  useQuery({ queryKey: qk.runs(mrId, limit), queryFn: () => api.list_runs(mrId, limit) });
export const useRunRows = (limit = 500) =>
  useQuery({ queryKey: qk.runRows(limit), queryFn: () => api.list_run_rows(limit) });
export const useCiLog = (jobId: number | null | undefined) =>
  useQuery({ queryKey: qk.ciLog(jobId ?? -1), queryFn: () => api.read_ci_log(jobId!), enabled: jobId != null, staleTime: Infinity });
export const useAudit = (runId: number | null, limit = 200) =>
  useQuery({ queryKey: qk.audit(runId, limit), queryFn: () => api.list_audit(runId, limit) });

export const useStats = (days: number) => useQuery({ queryKey: qk.stats(days), queryFn: () => api.get_stats(days) });
export const useRuleStats = (projectId: number | null, days: number) =>
  useQuery({ queryKey: qk.ruleStats(projectId, days), queryFn: () => api.rule_stats(projectId, days) });
/** Live guardrail preview; re-runs whenever the (JSON-serialised) overrides change. */
export function useSimulateGuardrails(projectId: number | null, overrides: api.ProjectOverrides, limitRuns = 10) {
  const json = JSON.stringify(overrides ?? {});
  return useQuery({
    queryKey: qk.simulate(projectId ?? -1, json, limitRuns),
    queryFn: () => api.simulate_guardrails(projectId!, JSON.parse(json) as api.ProjectOverrides, limitRuns),
    enabled: projectId != null,
    placeholderData: (prev) => prev,
  });
}
export const usePreviewNote = (findingId: number | null | undefined) =>
  useQuery({ queryKey: qk.previewNote(findingId ?? -1), queryFn: () => api.preview_note(findingId!), enabled: findingId != null });
export const usePendingFindings = () => useQuery({ queryKey: qk.pendingFindings, queryFn: api.list_pending_findings });

// ---- mutations -------------------------------------------------------------------------------

export function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

const onError = (e: unknown) => toast.error(errorMessage(e));

export function useSetFindingStatus() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, status }: { id: number; status: FindingStatus; runId: number }) => api.set_finding_status(id, status),
    onSuccess: (_d, v) => {
      qc.invalidateQueries({ queryKey: qk.run(v.runId) });
      qc.invalidateQueries({ queryKey: ["mrs"] });
      qc.invalidateQueries({ queryKey: qk.pendingFindings });
    },
    onError,
  });
}

export function useUpdateFinding() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { id: number; runId: number; title: string; body: string; suggestion: string | null }) =>
      api.update_finding(v.id, v.title, v.body, v.suggestion),
    onSuccess: (_d, v) => {
      qc.invalidateQueries({ queryKey: qk.run(v.runId) });
      qc.invalidateQueries({ queryKey: qk.pendingFindings });
      qc.invalidateQueries({ queryKey: qk.previewNote(v.id) });
      toast.success("Finding updated");
    },
    onError,
  });
}

export function usePostFindings() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ runId, ids }: { runId: number; ids: number[] }) => api.post_findings(runId, ids),
    onSuccess: (r, v) => {
      qc.invalidateQueries({ queryKey: qk.run(v.runId) });
      qc.invalidateQueries({ queryKey: ["mrs"] });
      qc.invalidateQueries({ queryKey: qk.pendingFindings });
      const parts = [`${r.posted} posted`];
      if (r.fallback) parts.push(`${r.fallback} as file notes`);
      if (r.failed) parts.push(`${r.failed} failed`);
      if (r.skipped) parts.push(`${r.skipped} skipped`);
      (r.failed ? toast.warning : toast.success)(parts.join(" · "));
    },
    onError,
  });
}

export function usePostFindingsBulk() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (ids: number[]) => api.post_findings_bulk(ids),
    onSuccess: (reports) => {
      qc.invalidateQueries({ queryKey: ["mrs"] });
      qc.invalidateQueries({ queryKey: ["runs"] });
      qc.invalidateQueries({ queryKey: ["run-rows"] });
      qc.invalidateQueries({ queryKey: ["run"] });
      qc.invalidateQueries({ queryKey: qk.pendingFindings });
      qc.invalidateQueries({ queryKey: ["stats"] });
      const sum = reports.reduce(
        (a, r) => ({ posted: a.posted + r.posted, fallback: a.fallback + r.fallback, failed: a.failed + r.failed, skipped: a.skipped + r.skipped }),
        { posted: 0, fallback: 0, failed: 0, skipped: 0 },
      );
      const parts = [`${sum.posted} posted`];
      if (sum.fallback) parts.push(`${sum.fallback} as file notes`);
      if (sum.failed) parts.push(`${sum.failed} failed`);
      if (sum.skipped) parts.push(`${sum.skipped} skipped`);
      (sum.failed ? toast.warning : toast.success)(parts.join(" · "));
    },
    onError,
  });
}

export function useStartReview(onStarted?: (runId: number) => void) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (mrId: number) => api.start_review(mrId),
    onSuccess: (runId) => {
      qc.invalidateQueries({ queryKey: ["mrs"] });
      qc.invalidateQueries({ queryKey: qk.usage });
      onStarted?.(runId);
    },
    onError,
  });
}

export function usePollNow() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: api.poll_now,
    onSuccess: () => qc.invalidateQueries({ queryKey: ["mrs"] }),
    onError,
  });
}

export function useSetPostingPaused() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (paused: boolean) => api.set_posting_paused(paused),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: qk.settings });
      qc.invalidateQueries({ queryKey: qk.usage });
    },
    onError,
  });
}

export function useSaveSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: api.save_settings,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: qk.settings });
      qc.invalidateQueries({ queryKey: qk.usage });
      qc.invalidateQueries({ queryKey: qk.engine });
    },
    onError,
  });
}

export function useRerunCiJob() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ runId, name }: { runId: number; name: string }) => api.rerun_ci_job(runId, name),
    onSuccess: (job) => {
      qc.setQueryData(qk.ciLog(job.id), "");
      qc.invalidateQueries({ queryKey: qk.run(job.run_id) });
    },
    onError,
  });
}

export function useCancelRun() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (runId: number) => api.cancel_run(runId),
    onSuccess: (_d, runId) => {
      qc.invalidateQueries({ queryKey: qk.run(runId) });
      qc.invalidateQueries({ queryKey: ["mrs"] });
    },
    onError,
  });
}
