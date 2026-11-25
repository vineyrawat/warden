// Pure display helpers shared by the screens (unit tested in format.test.ts).
import type { BadgeTone } from "@/components/ui/badge";
import type { CiJob } from "@/bindings/CiJob";
import type { CiStatus } from "@/bindings/CiStatus";
import type { FileDiff } from "@/bindings/FileDiff";
import type { Finding } from "@/bindings/Finding";
import type { MergeRequest } from "@/bindings/MergeRequest";
import type { MrRow } from "@/bindings/MrRow";
import type { PostingMode } from "@/bindings/PostingMode";
import type { ReviewRun } from "@/bindings/ReviewRun";
import type { RuleConfig } from "@/bindings/RuleConfig";
import type { RuleKind } from "@/bindings/RuleKind";
import type { RunStatus } from "@/bindings/RunStatus";
import type { Severity } from "@/bindings/Severity";

export const SEVERITIES: Severity[] = ["blocker", "major", "minor", "nit"];
export const POSTING_MODES: PostingMode[] = ["draft", "assist", "auto"];

export function severityRank(s: Severity): number {
  return { blocker: 3, major: 2, minor: 1, nit: 0 }[s];
}

export function severityTone(s: Severity): BadgeTone {
  return ({ blocker: "red", major: "orange", minor: "yellow", nit: "grey" } as const)[s];
}

export function capitalize(s: string): string {
  return s.length ? s[0].toUpperCase() + s.slice(1) : s;
}

export function modeLabel(m: PostingMode): string {
  return capitalize(m);
}

export function modeDescription(m: PostingMode, autoSeverities: Severity[] = ["blocker", "major"], minConf = 0.85): string {
  switch (m) {
    case "draft":
      return "Draft: nothing posts until you approve it in Warden.";
    case "assist": {
      const sev = autoSeverities.map((s) => `${s}s`).join(" and ") || "no severities";
      return `Assist: ${sev} at confidence ≥ ${minConf.toFixed(2)} post automatically, everything else waits for approval.`;
    }
    case "auto":
      return "Auto: every finding that passes the guardrails posts without approval.";
  }
}

/** Most severe first, then highest confidence, then file/line for stability. */
export function sortFindings<T extends Pick<Finding, "severity" | "confidence" | "file" | "line">>(list: T[]): T[] {
  return [...list].sort(
    (a, b) =>
      severityRank(b.severity) - severityRank(a.severity) ||
      b.confidence - a.confidence ||
      a.file.localeCompare(b.file) ||
      a.line - b.line,
  );
}

export function maxSeverity(list: Pick<Finding, "severity">[]): Severity | null {
  let best: Severity | null = null;
  for (const f of list) if (best === null || severityRank(f.severity) > severityRank(best)) best = f.severity;
  return best;
}

/** Findings that can still be selected for posting. */
export function isPostable(f: Pick<Finding, "status">): boolean {
  return f.status === "draft" || f.status === "approved" || f.status === "rejected";
}

/** "18s", "4m 12s", "1h 3m", "—" for 0. */
export function formatDuration(ms: number): string {
  if (!ms || ms < 0) return "—";
  const total = Math.round(ms / 1000);
  if (total < 60) return `${total}s`;
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h}h ${m}m`;
  return s ? `${m}m ${s}s` : `${m}m`;
}

/** 412 -> "412", 1900 -> "1.9k", 12000 -> "12k". */
export function formatCount(n: number): string {
  if (n < 1000) return String(n);
  const k = n / 1000;
  return `${k >= 10 ? Math.round(k) : Number(k.toFixed(1))}k`;
}

export function shortSha(sha: string | null | undefined): string {
  return sha ? sha.slice(0, 7) : "—";
}

export function formatRelative(iso: string | null | undefined, now: number = Date.now()): string {
  if (!iso) return "—";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return iso;
  const diff = Math.round((now - t) / 1000);
  if (diff < 45) return "just now";
  if (diff < 3600) return `${Math.round(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.round(diff / 3600)}h ago`;
  return `${Math.round(diff / 86400)}d ago`;
}

export function formatClock(totalSecs: number): string {
  const s = Math.max(0, Math.round(totalSecs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export function pollLabel(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const m = secs / 60;
  return `${Number.isInteger(m) ? m : m.toFixed(1)} min`;
}

// ---- run status ------------------------------------------------------------------------------

const RUN_LABEL: Record<RunStatus, string> = {
  queued: "Queued",
  preparing: "Preparing worktree",
  checking: "Running checks",
  reviewing: "Reviewing",
  filtering: "Filtering",
  awaiting_approval: "Awaiting approval",
  posting: "Posting",
  posted: "Posted",
  failed: "Failed",
  skipped: "Skipped",
  cancelled: "Cancelled",
};

export function runStatusLabel(s: RunStatus): string {
  return RUN_LABEL[s];
}

export function runStatusTone(s: RunStatus): BadgeTone {
  switch (s) {
    case "awaiting_approval":
      return "orange";
    case "posted":
      return "green";
    case "failed":
      return "red";
    case "skipped":
    case "cancelled":
      return "grey";
    default:
      return "blue";
  }
}

export function isRunActive(s: RunStatus): boolean {
  return ["queued", "preparing", "checking", "reviewing", "filtering", "posting"].includes(s);
}

/** Inbox status pill text, e.g. "Awaiting approval · 6 findings", "Reviewing · turn 9 / 30". */
export function mrStatusText(row: MrRow, maxTurns: number, liveTurn?: number): string {
  const run = row.latest_run;
  if (!run) return row.draft ? "Skipped · draft MR" : "Not reviewed";
  const c = row.finding_counts;
  switch (run.status) {
    case "awaiting_approval": {
      // Like the mockup: every finding of the run, including ones guardrails held back.
      const n = c.draft + c.rejected;
      const ciFail = row.ci_summary.failed > 0 ? " + CI" : "";
      return `Awaiting approval · ${n} finding${n === 1 ? "" : "s"}${ciFail}`;
    }
    case "reviewing":
      return `Reviewing · turn ${liveTurn ?? run.turns} / ${maxTurns}`;
    case "posted":
      return `Posted · ${c.posted} note${c.posted === 1 ? "" : "s"}`;
    case "skipped":
      return row.draft ? "Skipped · draft MR" : `Skipped${run.error ? ` · ${run.error}` : ""}`;
    case "failed":
      return `Failed${run.error ? ` · ${truncate(run.error, 40)}` : ""}`;
    default:
      return runStatusLabel(run.status);
  }
}

export function truncate(s: string, n: number): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

// ---- CI --------------------------------------------------------------------------------------

export function ciStatusTone(s: CiStatus): BadgeTone {
  switch (s) {
    case "passed":
      return "green";
    case "failed":
    case "timed_out":
      return "red";
    case "running":
    case "pending":
      return "blue";
    default:
      return "grey";
  }
}

export function ciStatusLabel(s: CiStatus): string {
  return { pending: "pending", running: "running", passed: "passed", failed: "failed", skipped: "skipped", not_installed: "not installed", timed_out: "timed out", cancelled: "cancelled" }[s];
}

/** Pill shown on a job row: duration when finished, else the status word. */
export function ciJobPill(job: Pick<CiJob, "status" | "duration_ms">): string {
  if ((job.status === "passed" || job.status === "failed" || job.status === "timed_out") && job.duration_ms > 0)
    return formatDuration(job.duration_ms);
  return ciStatusLabel(job.status);
}

export function ciSummaryPill(row: Pick<MrRow, "ci_summary" | "latest_run">): { label: string; tone: BadgeTone } {
  const s = row.ci_summary;
  if (s.total === 0) return { label: row.latest_run?.status === "skipped" || !row.latest_run ? "skipped" : "none", tone: "grey" };
  if (s.failed > 0) return { label: `${s.failed} failed`, tone: "red" };
  if (s.running > 0) return { label: "running", tone: "blue" };
  return { label: `${s.passed} / ${s.total}`, tone: s.passed === s.total ? "green" : "grey" };
}

export type LogTone = "error" | "ok" | "meta" | "plain";

/** Colour a CI log line like the mockup: failures red, passes green, commands/meta muted. */
export function classifyLogLine(line: string): LogTone {
  const l = line.trim();
  if (!l) return "plain";
  if (/^\$ /.test(l) || /^(Running|Ran|Collecting|  File |\s*at )/.test(line) || /^#/.test(l)) {
    if (/FAILED|ERROR/.test(l)) return "error";
    return "meta";
  }
  if (/\b(FAIL|FAILED|ERROR|Error|error:|AssertionError|Traceback|panicked|✗)\b/.test(l) || /\bfailed\b/.test(l)) return "error";
  if (/(\.\.\. ok$|\bPASS(ED)?\b|\bpassed\b|✓|\bOK\b|Job succeeded|All checks passed)/.test(l)) return "ok";
  return "plain";
}

// ---- rules -----------------------------------------------------------------------------------

export function ruleKind(r: Pick<RuleConfig, "instruction" | "pattern" | "semgrep">): RuleKind {
  if (r.semgrep) return "semgrep";
  if (r.pattern) return "regex";
  return "claude";
}

export function ruleKindTone(k: RuleKind): BadgeTone {
  return k === "claude" ? "blue" : "grey";
}

// ---- file tree -------------------------------------------------------------------------------

export function filePath(f: Pick<FileDiff, "new_path" | "old_path" | "is_deleted">): string {
  return f.is_deleted ? f.old_path : f.new_path;
}

export type TreeFile = { path: string; name: string; count: number; maxSeverity: Severity | null; additions: number; deletions: number };
export type TreeGroup = { dir: string; files: TreeFile[] };

export function fileStats(f: FileDiff): { additions: number; deletions: number } {
  let additions = 0;
  let deletions = 0;
  for (const h of f.hunks)
    for (const l of h.lines) {
      if (l.kind === "add") additions++;
      else if (l.kind === "del") deletions++;
    }
  return { additions, deletions };
}

/** Group diff files by directory (keeping diff order of first appearance) with finding counts. */
export function buildFileTree(files: FileDiff[], findings: Pick<Finding, "file" | "severity" | "status">[]): TreeGroup[] {
  const groups: TreeGroup[] = [];
  const byDir = new Map<string, TreeGroup>();
  const known = new Set<string>();
  const add = (path: string, f?: FileDiff) => {
    if (known.has(path)) return;
    known.add(path);
    const idx = path.lastIndexOf("/");
    const dir = idx >= 0 ? path.slice(0, idx + 1) : "";
    const name = idx >= 0 ? path.slice(idx + 1) : path;
    const mine = findings.filter((x) => x.file === path && x.status !== "dismissed");
    const stats = f ? fileStats(f) : { additions: 0, deletions: 0 };
    let g = byDir.get(dir);
    if (!g) {
      g = { dir, files: [] };
      byDir.set(dir, g);
      groups.push(g);
    }
    g.files.push({ path, name, count: mine.length, maxSeverity: maxSeverity(mine), ...stats });
  };
  files.forEach((f) => add(filePath(f), f));
  // Findings on files that are not part of the diff (e.g. CI-anchored) still get a row.
  findings.forEach((f) => add(f.file));
  return groups;
}

// ---- markdown export -------------------------------------------------------------------------

export function findingsToMarkdown(mr: Pick<MergeRequest, "iid" | "title">, run: Pick<ReviewRun, "summary" | "head_sha">, findings: Finding[]): string {
  const lines: string[] = [];
  lines.push(`## Warden review · !${mr.iid} ${mr.title}`, "");
  if (run.summary) lines.push(run.summary, "");
  const list = sortFindings(findings);
  if (list.length === 0) lines.push("_No findings._");
  for (const f of list) {
    const rule = f.rule_id ? ` · ${f.rule_id}` : "";
    lines.push(`### [${capitalize(f.severity)}] ${f.title}`);
    lines.push(`\`${f.file}:${f.line}\`${rule} · confidence ${f.confidence.toFixed(2)}`, "");
    lines.push(f.body);
    if (f.suggestion) lines.push("", "```suggestion", f.suggestion, "```");
    lines.push("");
  }
  lines.push(`— Warden · ${shortSha(run.head_sha)}`);
  return lines.join("\n");
}

// ---- tools -----------------------------------------------------------------------------------

const INSTALL_HINTS: Record<string, string> = {
  semgrep: "brew install semgrep",
  gitleaks: "brew install gitleaks",
  "gitlab-ci-local": "npm install -g gitlab-ci-local",
  act: "brew install act",
  docker: "Install Docker Desktop (docker.com)",
  rg: "brew install ripgrep",
  git: "xcode-select --install",
  claude: "npm install -g @anthropic-ai/claude-code, then run claude to log in",
};

export function installHint(tool: string): string {
  return INSTALL_HINTS[tool] ?? `Install ${tool} and make sure it is on PATH`;
}

export function splitGlobs(s: string): string[] {
  return s
    .split(/[,\n]/)
    .map((x) => x.trim())
    .filter(Boolean);
}
