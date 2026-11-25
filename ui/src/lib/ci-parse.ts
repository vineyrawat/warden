// Pure helpers for the Local CI screen: failure parsers per tool, stage assignment, log sections.
// Unit tested in __tests__/ci-parse.test.ts.
import type { CiJob } from "@/bindings/CiJob";

// ---- parsed failures ---------------------------------------------------------------------------

export type FailureKind = "test" | "lint" | "type" | "secret" | "scan";

export type ParsedFailure = {
  /** Path as printed by the tool (absolute Frappe bench paths are trimmed to the app). "" for summary-only entries. */
  file: string;
  line?: number;
  col?: number;
  /** Test name for test failures. */
  test?: string;
  /** Lint code / rule id (F401, TS2322, no-unused-vars, generic-api-key). */
  code?: string;
  message: string;
  kind: FailureKind;
};

const PY_EXC = /^([A-Za-z_][\w.]*(?:Error|Exception|Failure|Exit|Interrupt)|AssertionError|Failed)(?::\s?(.*))?$/;

/** `/home/f/frappe-bench/apps/entries_crm/entries_crm/api/x.py` -> `entries_crm/api/x.py`; strips `./`. */
export function normalizePath(p: string): string {
  let s = p.trim().replace(/^["']|["']$/g, "");
  const apps = s.match(/\/apps\/[^/]+\/(.+)$/);
  if (apps) s = apps[1];
  return s.replace(/^\.\//, "");
}

/** `entries_desk.sla.test_sla.TestSLA.test_x` -> `entries_desk/sla/test_sla.py`. */
function modulePath(dotted: string): string | undefined {
  const parts = dotted.split(".");
  const cls = parts.findIndex((p) => /^[A-Z]/.test(p));
  const mod = cls >= 0 ? parts.slice(0, cls) : parts.slice(0, -1);
  return mod.length > 1 ? `${mod.join("/")}.py` : undefined;
}

type Draft = ParsedFailure & { order: number };

/** Parse failures from a CI log (pytest, Frappe bench run-tests / unittest, ruff, flake8, mypy, eslint, tsc, gitleaks, semgrep). */
export function parseFailures(log: string | null | undefined): ParsedFailure[] {
  if (!log) return [];
  const lines = log.split(/\r?\n/).map((l) => l.replace(/\x1b\[[0-9;]*m/g, ""));
  const out: Draft[] = [];
  let order = 0;
  const push = (f: ParsedFailure) => out.push({ ...f, order: order++ });

  // pytest state
  const pyLineByTest = new Map<string, { file: string; line: number }>();
  const pyMsgByTest = new Map<string, string>();
  let pyBlock: string | null = null;

  // unittest state
  type Ut = { test: string; file?: string; line?: number; message?: string; fromModule?: string; kind: "FAIL" | "ERROR" };
  const ut = new Map<string, Ut>();
  let utCur: Ut | null = null;

  // eslint stylish state
  let eslintFile: string | null = null;
  // ruff "full" format state
  let ruffPending: { code: string; message: string } | null = null;
  // gitleaks state
  let leak: { rule?: string; file?: string; line?: number } | null = null;
  const flushLeak = () => {
    if (leak?.file) push({ file: normalizePath(leak.file), line: leak.line, code: leak.rule, message: `Secret detected${leak.rule ? ` (${leak.rule})` : ""}`, kind: "secret" });
    leak = null;
  };
  let leaksSummary = 0;
  // semgrep text state
  let sgFile: string | null = null;
  let sgRule: { id: string; msg: string[] } | null = null;
  let sgSummary = 0;

  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i];
    const line = raw.trimEnd();
    const t = line.trim();
    if (!t) {
      if (leak) flushLeak();
      continue;
    }
    let m: RegExpMatchArray | null;

    // ---- semgrep --json (one line)
    if (t.startsWith('{"') && t.includes('"results"')) {
      try {
        const j = JSON.parse(t) as { results?: { check_id?: string; path?: string; start?: { line?: number; col?: number }; extra?: { message?: string } }[] };
        for (const r of j.results ?? [])
          push({ file: normalizePath(r.path ?? ""), line: r.start?.line, col: r.start?.col, code: r.check_id?.split(".").pop(), message: (r.extra?.message ?? r.check_id ?? "semgrep finding").trim(), kind: "scan" });
        sgSummary = -1;
      } catch {
        /* not JSON */
      }
      continue;
    }

    // ---- pytest
    if ((m = t.match(/^(FAILED|ERROR) (\S+?\.py)::(\S+?)(?: - (.*))?$/))) {
      const nodeTest = m[3];
      const test = nodeTest.split("::").pop()!.replace(/\[.*\]$/, "");
      const key = nodeTest.replace(/::/g, ".");
      const loc = pyLineByTest.get(key) ?? pyLineByTest.get(test);
      push({ file: normalizePath(m[2]), line: loc?.line, test, message: (m[4] ?? pyMsgByTest.get(key) ?? pyMsgByTest.get(test) ?? (m[1] === "ERROR" ? "Error" : "Failed")).trim(), kind: "test" });
      continue;
    }
    if ((m = t.match(/^_{3,} (?:ERROR at \w+ of )?(\S+?) _{3,}$/))) {
      pyBlock = m[1].replace(/\[.*\]$/, "");
      continue;
    }
    if (pyBlock && (m = t.match(/^E\s+(.+)$/))) {
      if (!pyMsgByTest.has(pyBlock)) pyMsgByTest.set(pyBlock, m[1].trim());
      continue;
    }
    if ((m = t.match(/^(\S+?\.py):(\d+): (?:in \w+|[A-Za-z_][\w.]*)$/))) {
      // pytest traceback location. Last one in a block is where it raised.
      if (pyBlock) {
        pyLineByTest.set(pyBlock, { file: normalizePath(m[1]), line: Number(m[2]) });
        const short = pyBlock.split(".").pop()!;
        if (short !== pyBlock) pyLineByTest.set(short, { file: normalizePath(m[1]), line: Number(m[2]) });
      }
      continue;
    }

    // ---- unittest / Frappe bench run-tests
    if ((m = t.match(/^(FAIL|ERROR): (\w+) \(([\w.]+)\)/))) {
      const test = m[2];
      utCur = ut.get(test) ?? { test, kind: m[1] as "FAIL" | "ERROR" };
      utCur.fromModule ??= modulePath(m[3]);
      ut.set(test, utCur);
      continue;
    }
    if ((m = t.match(/^(\w+)(?: \([\w.]+\))? \.\.\. (FAIL|ERROR)$/))) {
      utCur = ut.get(m[1]) ?? { test: m[1], kind: m[2] as "FAIL" | "ERROR" };
      ut.set(m[1], utCur);
      continue;
    }
    if ((m = t.match(/^File "([^"]+)", line (\d+), in (\S+)/))) {
      const fn = m[3];
      const target = ut.get(fn) ?? utCur;
      const sitePkg = /site-packages|\/frappe\/frappe\/|\/lib\/python/.test(m[1]);
      if (target && (fn === target.test || (!target.file && !sitePkg))) {
        target.file = normalizePath(m[1]);
        target.line = Number(m[2]);
      }
      continue;
    }
    if (utCur && (m = t.match(PY_EXC)) && !/^(FAIL|ERROR)/.test(t)) {
      utCur.message ??= (m[2] ? `${m[1].split(".").pop()}: ${m[2]}` : m[1]).trim();
      continue;
    }
    if (/^Ran \d+ tests?/.test(t) || /^={5,}$/.test(t)) {
      if (/^Ran /.test(t)) utCur = null;
      continue;
    }

    // ---- tsc
    if ((m = t.match(/^(\S+?\.[cm]?tsx?)\((\d+),(\d+)\): error (TS\d+): (.+)$/)) || (m = t.match(/^(\S+?\.[cm]?tsx?):(\d+):(\d+) - error (TS\d+): (.+)$/))) {
      push({ file: normalizePath(m[1]), line: Number(m[2]), col: Number(m[3]), code: m[4], message: m[5], kind: "type" });
      continue;
    }

    // ---- mypy
    if ((m = t.match(/^(\S+?\.pyi?):(\d+):(?:(\d+):)? error: (.+?)(?:\s+\[([\w-]+)\])?$/))) {
      push({ file: normalizePath(m[1]), line: Number(m[2]), col: m[3] ? Number(m[3]) : undefined, code: m[5], message: m[4], kind: "type" });
      continue;
    }

    // ---- ruff / flake8 (concise): path:line:col: CODE msg
    if ((m = t.match(/^(\S+?\.\w+):(\d+):(\d+): ([A-Z]{1,4}\d{1,4})\b:? (?:\[\*\] )?(.+)$/))) {
      push({ file: normalizePath(m[1]), line: Number(m[2]), col: Number(m[3]), code: m[4], message: m[5], kind: "lint" });
      continue;
    }
    // ---- ruff full format: "F401 [*] msg" then " --> path:line:col"
    if ((m = t.match(/^([A-Z]{1,4}\d{2,4}) (?:\[\*\] )?(.+)$/))) {
      ruffPending = { code: m[1], message: m[2] };
      continue;
    }
    if (ruffPending && (m = t.match(/^-->\s*(\S+?):(\d+):(\d+)$/))) {
      push({ file: normalizePath(m[1]), line: Number(m[2]), col: Number(m[3]), code: ruffPending.code, message: ruffPending.message, kind: "lint" });
      ruffPending = null;
      continue;
    }

    // ---- eslint unix / compact
    if ((m = t.match(/^(\S+?\.(?:[cm]?[jt]sx?|vue|svelte)):(\d+):(\d+): (.+?) \[(Error|Warning)\/(\S+)\]$/))) {
      if (m[5] === "Error") push({ file: normalizePath(m[1]), line: Number(m[2]), col: Number(m[3]), code: m[6], message: m[4], kind: "lint" });
      continue;
    }
    // ---- eslint stylish: file header then "  12:7  error  msg  rule"
    if (/^\S/.test(raw) && /\.(?:[cm]?[jt]sx?|vue|svelte)$/.test(t) && !/\s/.test(t)) {
      eslintFile = t;
      continue;
    }
    if (eslintFile && (m = raw.match(/^\s+(\d+):(\d+)\s+(error|warning)\s+(.+?)(?:\s{2,}([@\w/-]+))?$/))) {
      if (m[3] === "error") push({ file: normalizePath(eslintFile), line: Number(m[1]), col: Number(m[2]), code: m[5], message: m[4].trim(), kind: "lint" });
      continue;
    }
    if (/^✖ \d+ problems?/.test(t)) {
      eslintFile = null;
      continue;
    }

    // ---- gitleaks (verbose report blocks)
    if ((m = t.match(/^(Finding|Secret|RuleID|Entropy|File|Line|Commit|Author|Email|Date|Fingerprint|Match|Tags|StartLine):\s*(.*)$/))) {
      const [, k, v] = m;
      if (k === "Finding" && leak?.file) flushLeak();
      leak ??= {};
      if (k === "RuleID") leak.rule = v.trim();
      else if (k === "File") leak.file = v.trim();
      else if (k === "Line" || k === "StartLine") leak.line = Number(v) || undefined;
      else if (k === "Fingerprint") flushLeak();
      continue;
    }
    if ((m = t.match(/leaks found: (\d+)/))) {
      leaksSummary = Number(m[1]);
      continue;
    }

    // ---- semgrep text output
    if ((m = t.match(/^Ran \d+ rules? on \d+ files?: (\d+) findings?/))) {
      if (sgSummary >= 0) sgSummary = Number(m[1]);
      continue;
    }
    if (/^\s{2,}\S+\.(?:py|[cm]?[jt]sx?|vue|go|rb|java|rs|php|ya?ml|json|html|sh)$/.test(raw)) {
      sgFile = t;
      sgRule = null;
      continue;
    }
    if (sgFile && (m = t.match(/^(?:❯❯❱|❯❱|❱|>>>?)\s*(\S+)$/))) {
      sgRule = { id: m[1], msg: [] };
      continue;
    }
    if (sgFile && sgRule && (m = t.match(/^(\d+)┆/))) {
      push({ file: normalizePath(sgFile), line: Number(m[1]), code: sgRule.id.split(".").pop(), message: sgRule.msg.join(" ").trim() || sgRule.id, kind: "scan" });
      sgRule = null;
      continue;
    }
    if (sgFile && sgRule && !/^(?:Details|Autofix|⋮┆)/.test(t)) {
      sgRule.msg.push(t);
      continue;
    }
  }
  if (leak) flushLeak();

  for (const u of ut.values()) {
    push({ file: u.file ?? u.fromModule ?? "", line: u.file ? u.line : undefined, test: u.test, message: u.message ?? (u.kind === "ERROR" ? "Error" : "Failed"), kind: "test" });
  }
  if (leaksSummary > 0 && !out.some((f) => f.kind === "secret"))
    push({ file: "", message: `gitleaks found ${leaksSummary} leak${leaksSummary === 1 ? "" : "s"}`, kind: "secret" });
  if (sgSummary > 0 && !out.some((f) => f.kind === "scan"))
    push({ file: "", message: `semgrep reported ${sgSummary} finding${sgSummary === 1 ? "" : "s"}`, kind: "scan" });

  // Dedupe (same failure printed in a traceback and a summary).
  const seen = new Set<string>();
  const result: ParsedFailure[] = [];
  for (const d of out.sort((a, b) => a.order - b.order)) {
    const key = d.test ? `t|${d.file}|${d.test}` : `${d.kind}|${d.file}|${d.line ?? ""}|${d.code ?? ""}|${d.message}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const { order: _o, ...f } = d;
    void _o;
    result.push(Object.fromEntries(Object.entries(f).filter(([, v]) => v !== undefined)) as ParsedFailure);
  }
  return result;
}

/** Diff path that a tool-printed path refers to (exact or suffix match on path segments), else null. */
export function matchDiffPath(file: string, diffPaths: string[]): string | null {
  if (!file) return null;
  const f = normalizePath(file);
  for (const p of diffPaths) if (p === f) return p;
  for (const p of diffPaths) if (f.endsWith(`/${p}`) || p.endsWith(`/${f}`)) return p;
  return null;
}

/** `path:line` label. */
export function failureLocation(f: Pick<ParsedFailure, "file" | "line">): string {
  return f.line ? `${f.file}:${f.line}` : f.file;
}

/** One-line test summary from a log: pytest's final line, or unittest's Ran/FAILED pair. */
export function summarizeLog(log: string | null | undefined): string | null {
  if (!log) return null;
  const lines = log.split(/\r?\n/);
  for (let i = lines.length - 1; i >= 0; i--) {
    const t = lines[i].trim();
    const py = t.match(/^=+ (.*\b(?:passed|failed|error|errors)\b.*?)(?: in [\d.]+s(?: \([^)]*\))?)? =+$/);
    if (py) return py[1];
    const ran = t.match(/^Ran (\d+) tests?/);
    if (ran) {
      const total = Number(ran[1]);
      const tail = t + " " + (lines[i + 1] ?? "") + " " + (lines[i + 2] ?? "");
      const fm = tail.match(/FAILED \(([^)]*)\)/);
      if (fm) {
        let bad = 0;
        for (const part of fm[1].split(",")) {
          const kv = part.trim().match(/^(failures|errors)=(\d+)$/);
          if (kv) bad += Number(kv[2]);
        }
        return `${bad} failed, ${total - bad} passed`;
      }
      if (/\bOK\b/.test(tail)) return `${total} passed`;
    }
  }
  return null;
}

// ---- stages ------------------------------------------------------------------------------------

export const STAGE_ORDER = ["prepare", "check", "test", "scan"] as const;

const STAGE_LABEL: Record<string, string> = { prepare: "Prepare", check: "Check", test: "Test", scan: "Scan", "gitlab-ci": "GitLab CI" };

export function stageLabel(stage: string): string {
  return STAGE_LABEL[stage] ?? stage.charAt(0).toUpperCase() + stage.slice(1);
}

/** Stage for a job, by name first, then by command. gitlab-ci-local jobs get their own stage. */
export function jobStage(job: Pick<CiJob, "name" | "kind" | "command">): string {
  const n = job.name.toLowerCase();
  const c = (job.command ?? "").toLowerCase();
  if (job.kind === "gitlab_ci_local" || n.startsWith("gitlab-ci")) return "gitlab-ci";
  if (/semgrep|gitleaks|secret|trufflehog|bandit|\bsast\b|\baudit\b|scan/.test(n)) return "scan";
  if (/^(worktree|setup|install|deps|bootstrap|prepare|checkout|migrate|build)\b/.test(n)) return "prepare";
  if (/lint|format|fmt|ruff|eslint|flake8|prettier|black|typecheck|types|tsc|mypy|style/.test(n)) return "check";
  if (/test|pytest|unit|spec|jest|vitest|e2e|integration/.test(n)) return "test";
  if (/semgrep|gitleaks|secret/.test(c)) return "scan";
  if (/run-tests|pytest|vitest|jest|cargo test|go test|npm (run )?test|yarn test/.test(c)) return "test";
  if (/ruff|eslint|flake8|prettier|black|tsc|mypy/.test(c)) return "check";
  if (/pip install|npm (ci|install)|yarn install|bench setup/.test(c)) return "prepare";
  return "check";
}

export type StageGroup<J> = { stage: string; label: string; jobs: J[] };

/** Jobs grouped into stages: prepare, check, test, scan, then any other stage in order of appearance. Empty stages are dropped. */
export function groupStages<J extends Pick<CiJob, "name" | "kind" | "command">>(jobs: J[]): StageGroup<J>[] {
  const byStage = new Map<string, J[]>();
  for (const j of jobs) {
    const s = jobStage(j);
    const list = byStage.get(s) ?? [];
    list.push(j);
    byStage.set(s, list);
  }
  const known = STAGE_ORDER.filter((s) => byStage.has(s)) as string[];
  const extra = [...byStage.keys()].filter((s) => !(STAGE_ORDER as readonly string[]).includes(s));
  return [...known, ...extra].map((stage) => ({ stage, label: stageLabel(stage), jobs: byStage.get(stage)! }));
}

/** Tool to install for a `not_installed` job (feeds format.ts installHint). */
export function missingTool(job: Pick<CiJob, "name" | "kind" | "command">): string {
  if (job.kind === "gitlab_ci_local") return "gitlab-ci-local";
  const hay = `${job.name} ${job.command}`.toLowerCase();
  for (const t of ["gitlab-ci-local", "semgrep", "gitleaks", "act", "docker", "rg"]) if (new RegExp(`\\b${t}\\b`).test(hay)) return t;
  return job.command.trim().split(/\s+/)[0] || job.name;
}

// ---- log sections ------------------------------------------------------------------------------

export type LogSection = {
  /** Command after `$ ` (or null for output before the first command). */
  title: string | null;
  /** Index of the first line (the `$` line itself for titled sections). */
  start: number;
  /** Exclusive end index. */
  end: number;
};

const CMD = /^\s*(?:[\w:.-]+ )?\$ (.+)$/;

/** Split log lines into sections, one per `$ command` line. */
export function logSections(lines: string[]): LogSection[] {
  const out: LogSection[] = [];
  let cur: LogSection | null = null;
  lines.forEach((l, i) => {
    const m = l.match(CMD);
    if (m) {
      if (cur) cur.end = i;
      cur = { title: m[1].trim(), start: i, end: lines.length };
      out.push(cur);
    } else if (!cur) {
      cur = { title: null, start: i, end: lines.length };
      out.push(cur);
    }
  });
  return out;
}
