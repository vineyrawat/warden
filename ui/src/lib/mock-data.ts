// Sample data for the mock backend: the same repos, MRs and findings as the docs/plan.html mockups.
import type { AppSettings } from "@/bindings/AppSettings";
import type { AuditEvent } from "@/bindings/AuditEvent";
import type { CiJob } from "@/bindings/CiJob";
import type { DiffLine } from "@/bindings/DiffLine";
import type { DiffPosition } from "@/bindings/DiffPosition";
import type { EngineStatus } from "@/bindings/EngineStatus";
import type { FileDiff } from "@/bindings/FileDiff";
import type { Finding } from "@/bindings/Finding";
import type { FindingStatus } from "@/bindings/FindingStatus";
import type { ForgeStatus } from "@/bindings/ForgeStatus";
import type { Hunk } from "@/bindings/Hunk";
import type { MergeRequest } from "@/bindings/MergeRequest";
import type { Project } from "@/bindings/Project";
import type { ReviewRun } from "@/bindings/ReviewRun";
import type { RuleConfig } from "@/bindings/RuleConfig";
import type { ToolStatus } from "@/bindings/ToolStatus";
import type { WardenConfig } from "@/bindings/WardenConfig";

// ---- diff builders ---------------------------------------------------------------------------

/**
 * Build a hunk from marker-prefixed lines (`+added`, `-deleted`, ` context`), numbering old/new lines
 * from the given starts. The header is computed like git does.
 */
export function makeHunk(oldStart: number, newStart: number, section: string, raw: string[]): Hunk {
  let o = oldStart;
  let n = newStart;
  let oldLines = 0;
  let newLines = 0;
  const lines: DiffLine[] = raw.map((r) => {
    const mark = r[0];
    const text = r.slice(1);
    if (mark === "+") {
      newLines++;
      return { kind: "add", old_line: null, new_line: n++, text };
    }
    if (mark === "-") {
      oldLines++;
      return { kind: "del", old_line: o++, new_line: null, text };
    }
    oldLines++;
    newLines++;
    return { kind: "context", old_line: o++, new_line: n++, text };
  });
  const header = `@@ -${oldStart},${oldLines} +${newStart},${newLines} @@${section ? ` ${section}` : ""}`;
  return { old_start: oldStart, old_lines: oldLines, new_start: newStart, new_lines: newLines, header, lines };
}

export function makeFile(path: string, hunks: Hunk[], opts: Partial<FileDiff> = {}): FileDiff {
  return { old_path: path, new_path: path, is_new: false, is_deleted: false, is_binary: false, hunks, ...opts };
}

export function newFile(path: string, body: string[]): FileDiff {
  return makeFile(path, [makeHunk(0, 1, "", body.map((l) => `+${l}`))], { is_new: true, old_path: "/dev/null" });
}

// ---- config ----------------------------------------------------------------------------------

export function defaultRules(): RuleConfig[] {
  const base = { applies_to: "**/*.py", instruction: null, pattern: null, semgrep: null } as const;
  return [
    { ...base, id: "FR-001", title: "Whitelisted methods must check permissions", severity: "major", instruction: "Any @frappe.whitelist() function that reads or writes documents must call frappe.has_permission, doc.check_permission or frappe.only_for first." },
    { ...base, id: "FR-002", title: "No f-string or %-formatted SQL", severity: "blocker", pattern: "frappe\\.db\\.sql\\(\\s*f[\"']" },
    { ...base, id: "FR-004", title: "ignore_permissions=True needs a comment saying why", severity: "major", instruction: "ignore_permissions=True must be accompanied by a comment explaining why bypassing permissions is safe." },
    { ...base, id: "FR-007", title: "No frappe.db.commit() in request handlers", severity: "major", pattern: "frappe\\.db\\.commit\\(\\s*\\)" },
    { ...base, applies_to: null, id: "SEC-001", title: "Hardcoded secret", severity: "blocker", instruction: "Flag hardcoded secrets (API keys, tokens, passwords, private keys) in added code." },
  ];
}

export function defaultConfig(): WardenConfig {
  return {
    version: 1,
    review: {
      focus: ["security", "correctness", "frappe-permissions", "performance"],
      ignore_paths: ["yarn.lock", "**/*.min.js", "**/dist/**"],
      max_notes: 10,
      min_confidence: 0.7,
      severity_floor: "minor",
      nits: false,
      extra_instructions: "",
    },
    rules: defaultRules(),
    ci: {
      recipes: [
        { name: "lint", run: "ruff check {changed_py} && npx eslint {changed_ts}", use: null, jobs: [], cwd: ".", timeout: "5m", env: {} },
        { name: "unit", run: "bench --site crm_test.localhost run-tests --app entries_crm --failfast", use: null, jobs: [], cwd: "../..", timeout: "15m", env: {} },
        { name: "gitlab-ci", run: null, use: "gitlab-ci-local", jobs: ["build"], cwd: null, timeout: null, env: {} },
      ],
    },
    posting: {
      mode: "draft",
      auto_post: { severities: ["blocker", "major"], min_confidence: 0.85 },
      summary_note: true,
      max_notes_per_hour: 30,
      mr_status: { mode: "off", request_changes_on: ["blocker", "major"] },
    },
  };
}

function crmConfig(): WardenConfig {
  const c = defaultConfig();
  c.posting.mode = "assist";
  c.rules = [
    ...c.rules.map((r) => (r.id === "FR-002" || r.id === "FR-007" ? { ...r, pattern: null, semgrep: `rules/${r.id.toLowerCase()}.yml` } : r)),
    { id: "UI-002", title: "New screens use @entries/ui components, not raw HTML controls", applies_to: "frontend/**/*.tsx", instruction: "New screens should use @entries/ui components instead of raw <button>, <input> or <select>.", pattern: null, semgrep: null, severity: "minor" },
    { id: "GJ-003", title: "Doctype JSON changes must match a generator run", applies_to: "**/doctype/**/*.json", instruction: "Doctype JSON edits must correspond to a gen_doctypes.py run; flag hand edits.", pattern: null, semgrep: null, severity: "major" },
  ];
  return c;
}

// ---- history (merged MRs, last 14 days) --------------------------------------------------------

type HistTemplate = Pick<Finding, "severity" | "category" | "title" | "body" | "file"> & { rule_id: string | null; line: number; source?: string };

/** Finding templates keyed by rule id (or `claude:<category>` for findings without a rule). */
const HIST_TEMPLATES: Record<string, HistTemplate> = {
  "FR-007": { rule_id: "FR-007", severity: "major", category: "correctness", title: "Explicit commit inside request", body: "Frappe commits at the end of the request. Committing here makes a partial update permanent if a later step raises.", file: "entries_crm/api/leads.py", line: 48, source: "rule" },
  "FR-001": { rule_id: "FR-001", severity: "major", category: "frappe-permissions", title: "Whitelisted method has no permission check", body: "Any logged-in user can call this method. Call frappe.has_permission on the target document before reading or writing it.", file: "entries_crm/api/assign.py", line: 21 },
  "FR-002": { rule_id: "FR-002", severity: "blocker", category: "security", title: "SQL built with an f-string", body: "Request input is interpolated into SQL. Pass values as query parameters instead.", file: "entries_crm/api/reports.py", line: 33, source: "rule" },
  "FR-004": { rule_id: "FR-004", severity: "major", category: "frappe-permissions", title: "ignore_permissions=True without a comment", body: "Bypassing permissions needs a comment that says why it is safe here.", file: "entries_crm/api/importer.py", line: 112 },
  "SEC-001": { rule_id: "SEC-001", severity: "blocker", category: "security", title: "Hardcoded API key", body: "A live-looking API key is committed in this file. Move it to site config and rotate it.", file: "entries_platform/billing/stripe.py", line: 9 },
  "claude:correctness": { rule_id: null, severity: "major", category: "correctness", title: "Off-by-one in pagination", body: "`start + page_size + 1` skips one row per page. Use `start + page_size`.", file: "entries_crm/api/list_view.py", line: 64 },
  "claude:tests": { rule_id: null, severity: "minor", category: "tests", title: "Test depends on wall-clock time", body: "The assertion compares against now(); freeze time so the test does not fail near midnight.", file: "entries_crm/tests/test_reminders.py", line: 40 },
  "claude:performance": { rule_id: null, severity: "minor", category: "performance", title: "Query inside a loop", body: "frappe.get_doc runs once per row. Fetch the rows with one frappe.get_all call instead.", file: "entries_desk/sla/report.py", line: 77 },
  "claude:ux": { rule_id: null, severity: "minor", category: "ux", title: "No empty state", body: "The table renders an empty grid when there are no rows. Show a short message instead.", file: "frontend/src/leads/AssignTable.tsx", line: 52 },
};

/** Status cycles per rule. FR-007 is noisy: dismissed 6 times out of 11 (~55%). */
const HIST_STATUS: Record<string, FindingStatus[]> = {
  "FR-007": ["dismissed", "posted", "dismissed", "dismissed", "resolved", "dismissed", "posted", "dismissed", "rejected", "dismissed", "posted"],
  default: ["resolved", "posted", "resolved", "dismissed", "posted", "rejected", "resolved"],
};

/** Order in which findings are drawn for successive history runs. */
const HIST_ROTATION = ["FR-007", "claude:correctness", "FR-001", "FR-007", "claude:tests", "FR-004", "FR-007", "claude:performance", "FR-002", "claude:ux", "FR-007", "SEC-001"];

/** [mrId, daysAgo, hour of day (UTC-ish), status] — no runs 6 and 7 days ago (weekend). */
const HIST_RUNS: [number, number, number, "posted" | "failed" | "skipped"][] = [
  [10, 1, 10, "posted"], [11, 1, 15, "posted"], [12, 2, 9, "posted"], [10, 2, 14, "posted"],
  [13, 3, 11, "posted"], [14, 3, 16, "posted"], [15, 4, 10, "failed"], [15, 4, 13, "posted"],
  [16, 5, 9, "posted"], [17, 5, 12, "posted"], [18, 5, 17, "posted"], [19, 8, 10, "posted"],
  [12, 8, 15, "posted"], [11, 9, 11, "posted"], [14, 9, 14, "skipped"], [16, 10, 10, "posted"],
  [18, 10, 16, "posted"], [13, 11, 9, "posted"], [17, 11, 13, "posted"], [19, 12, 12, "posted"],
  [10, 13, 10, "posted"], [11, 13, 15, "posted"],
];

function historyData(now: number): { mrs: MergeRequest[]; runs: ReviewRun[]; findings: Finding[] } {
  const sha = (n: number, salt: string) => fakeFingerprint(`${salt}${n}`, "sha", null).slice(0, 40);
  const mrDefs: [number, number, number, string, string, number, number][] = [
    [10, 1, 30, "feat(leads): bulk assign owner", "feat/bulk-assign", 212, 18],
    [11, 1, 25, "fix(import): dedupe rows by email", "fix/import-dedupe", 58, 22],
    [12, 1, 24, "feat(ai): summarise call notes", "feat/ai-call-notes", 340, 41],
    [13, 1, 23, "chore: bump frappe to v15.42", "chore/frappe-15-42", 12, 12],
    [14, 3, 27, "feat(desk): canned replies", "feat/canned-replies", 410, 9],
    [15, 3, 26, "fix(desk): assignment round robin", "fix/round-robin", 77, 31],
    [16, 2, 18, "feat(platform): entitlement cache", "feat/entitlement-cache", 156, 20],
    [17, 2, 17, "fix(platform): plan downgrade guard", "fix/downgrade-guard", 44, 6],
    [18, 1, 22, "feat(reports): pipeline by stage", "feat/pipeline-report", 268, 35],
    [19, 3, 25, "fix(desk): email threading", "fix/email-threading", 93, 40],
  ];
  const remote: Record<number, string> = { 1: "entries/entries_crm", 2: "entries/entries_platform", 3: "entries/entries_desk" };
  const mrs: MergeRequest[] = mrDefs.map(([id, project_id, iid, title, source_branch, additions, deletions]) => ({
    id,
    project_id,
    iid,
    title,
    description: "",
    author: id % 3 === 0 ? "priya" : "vinay",
    source_branch,
    target_branch: "develop",
    web_url: `https://gitlab.com/${remote[project_id]}/-/merge_requests/${iid}`,
    draft: false,
    labels: ["ai-reviewed"],
    head_sha: sha(id, "h"),
    base_sha: sha(id, "b"),
    start_sha: sha(id, "b"),
    last_reviewed_sha: sha(id, "h"),
    additions,
    deletions,
    state: "merged",
    updated_at: iso(now, 60 * 24 * 1),
  }));

  const today = new Date(now);
  const at = (daysAgo: number, hour: number) => {
    const d = new Date(Date.UTC(today.getUTCFullYear(), today.getUTCMonth(), today.getUTCDate() - daysAgo, hour, 12));
    return d.getTime();
  };

  const runs: ReviewRun[] = [];
  const findings: Finding[] = [];
  const perRule: Record<string, number> = {};
  let rot = 0;
  let fid = 800;
  HIST_RUNS.forEach(([mrId, daysAgo, hour, status], i) => {
    const id = 60 + i;
    const mr = mrs.find((m) => m.id === mrId)!;
    const started = at(daysAgo, hour);
    const duration = status === "skipped" ? 0 : 95_000 + ((i * 37_919) % 320_000);
    runs.push({
      id,
      mr_id: mrId,
      base_sha: mr.base_sha,
      head_sha: mr.head_sha,
      since_sha: null,
      status,
      model: "opus",
      turns: status === "skipped" ? 0 : 8 + ((i * 7) % 18),
      duration_ms: duration,
      cli_session_id: status === "skipped" ? null : `hist-${id}-${mr.iid}`,
      summary: status === "posted" ? `Reviewed !${mr.iid}: ${mr.title}.` : null,
      verdict: status === "posted" ? (i % 4 === 0 ? "approve" : i % 4 === 1 ? "request_changes" : "comment") : null,
      error: status === "failed" ? "claude exited after the review timeout (600s)" : status === "skipped" ? "draft MR" : null,
      started_at: new Date(started).toISOString(),
      ended_at: new Date(started + duration).toISOString(),
    });
    if (status !== "posted") return;
    const count = 2 + (i % 3);
    for (let k = 0; k < count; k++) {
      const key = HIST_ROTATION[rot++ % HIST_ROTATION.length];
      const t = HIST_TEMPLATES[key];
      const cycle = HIST_STATUS[key] ?? HIST_STATUS.default;
      const n = perRule[key] ?? 0;
      perRule[key] = n + 1;
      const st = cycle[n % cycle.length];
      const confidence = Math.round((0.66 + ((fid * 13) % 30) / 100) * 100) / 100;
      const shas = { base: mr.base_sha, start: mr.start_sha, head: mr.head_sha };
      const line = t.line + ((i * 3 + k) % 9);
      findings.push({
        id: fid++,
        run_id: id,
        file: t.file,
        line,
        side: "new",
        severity: t.severity,
        category: t.category,
        rule_id: t.rule_id,
        title: t.title,
        body: t.body,
        suggestion: null,
        evidence: t.rule_id ? null : "quoted from the diff",
        confidence: st === "rejected" ? Math.min(confidence, 0.62) : confidence,
        source: t.source ?? "claude",
        status: st,
        guardrail_reason: st === "rejected" ? `confidence ${Math.min(confidence, 0.62).toFixed(2)} is below the threshold 0.70` : null,
        fingerprint: fakeFingerprint(t.file, t.title, t.rule_id),
        forge_note_id: st === "posted" || st === "resolved" ? `disc-h${id}${k}` : null,
        position: pos(shas, t.file, line),
      });
    }
  });
  return { mrs, runs, findings };
}

// ---- seed ------------------------------------------------------------------------------------

export type Seed = {
  settings: AppSettings;
  engine: EngineStatus;
  tools: ToolStatus[];
  forges: ForgeStatus[];
  remoteProjects: [string, string][];
  projects: Project[];
  configs: Record<number, WardenConfig>;
  overrides: Record<number, Record<string, unknown>>;
  mrs: MergeRequest[];
  runs: ReviewRun[];
  findings: Finding[];
  ciJobs: CiJob[];
  files: Record<number, FileDiff[]>;
  logs: Record<number, string>;
  audit: AuditEvent[];
};

const iso = (now: number, minutesAgo: number) => new Date(now - minutesAgo * 60_000).toISOString();

export const SHAS = {
  mr31: { base: "a1c9e04b7d2e4f1a9c0b3d5e7f9a1b2c3d4e5f60", start: "a1c9e04b7d2e4f1a9c0b3d5e7f9a1b2c3d4e5f60", head: "7f3b2d1c9e8a7b6c5d4e3f2a1b0c9d8e7f6a5b4c" },
  mr29: { base: "5d2a91c0e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8", start: "5d2a91c0e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b8", head: "c44e0a9b8c7d6e5f4a3b2c1d0e9f8a7b6c5d4e3f" },
  mr28: { base: "0b7c3e2d1f0a9b8c7d6e5f4a3b2c1d0e9f8a7b6c", start: "0b7c3e2d1f0a9b8c7d6e5f4a3b2c1d0e9f8a7b6c", head: "9e1f0aa3b2c1d0e9f8a7b6c5d4e3f2a1b0c9d8e7" },
  mr27: { base: "be047ba1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7", start: "be047ba1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7", head: "a819b62f0e1d2c3b4a5f6e7d8c9b0a1f2e3d4c5b" },
  mr26: { base: "3f9e2d1c0b9a8f7e6d5c4b3a2f1e0d9c8b7a6f5e", start: "3f9e2d1c0b9a8f7e6d5c4b3a2f1e0d9c8b7a6f5e", head: "e8d7c6b5a4f3e2d1c0b9a8f7e6d5c4b3a2f1e0d9" },
};

function pos(shas: { base: string; start: string; head: string }, path: string, newLine: number | null, oldLine: number | null = null): DiffPosition {
  return { base_sha: shas.base, start_sha: shas.start, head_sha: shas.head, old_path: path, new_path: path, old_line: oldLine, new_line: newLine };
}

/** Deterministic fake fingerprint (the backend uses sha256; the UI only treats it as an opaque id). */
export function fakeFingerprint(file: string, title: string, rule: string | null): string {
  const s = `${file}:${title.toLowerCase().replace(/\s+/g, " ").trim()}:${rule ?? ""}`;
  let h1 = 0x811c9dc5;
  let h2 = 0x01000193;
  for (let i = 0; i < s.length; i++) {
    h1 = Math.imul(h1 ^ s.charCodeAt(i), 16777619) >>> 0;
    h2 = Math.imul(h2 + s.charCodeAt(i), 2246822519) >>> 0;
  }
  return (h1.toString(16).padStart(8, "0") + h2.toString(16).padStart(8, "0")).repeat(4);
}

type FindingInput = Omit<Finding, "fingerprint" | "forge_note_id" | "position" | "guardrail_reason" | "evidence" | "suggestion" | "rule_id" | "side" | "source"> &
  Partial<Pick<Finding, "forge_note_id" | "guardrail_reason" | "evidence" | "suggestion" | "rule_id" | "side" | "source">>;

function finding(shas: { base: string; start: string; head: string }, f: FindingInput): Finding {
  const rule = f.rule_id ?? null;
  return {
    side: "new",
    source: "claude",
    suggestion: null,
    evidence: null,
    guardrail_reason: null,
    forge_note_id: null,
    ...f,
    rule_id: rule,
    fingerprint: fakeFingerprint(f.file, f.title, rule),
    position: pos(shas, f.file, f.line),
  };
}

// File contents for the MRs ----------------------------------------------------------------------

function filesMr31(): FileDiff[] {
  return [
    makeFile("entries_crm/api/import_links.py", [
      makeHunk(10, 10, "import json", [
        " import frappe",
        "+",
        "+@frappe.whitelist()",
        "+def link_import_rows(import_name, rows):",
        "+    rows = json.loads(rows)",
        "+    for r in rows:",
        "+        frappe.db.sql(f\"update `tabECRM Lead` set import_ref='{import_name}' where name='{r}'\")",
        "+    frappe.db.commit()",
        "-# legacy linker removed",
        " ",
        " ",
        " def _review_rows(import_name):",
        "-    return frappe.get_all(\"ECRM Import Row\", filters={\"parent\": import_name})",
        "+    return frappe.get_all(",
        "+        \"ECRM Import Row\",",
        "+        filters={\"parent\": import_name},",
        "+        fields=[\"name\", \"status\", \"lead\", \"row_hash\"],",
        "+        order_by=\"idx asc\",",
        "+    )",
      ]),
      makeHunk(40, 49, "def _review_rows(import_name):", [
        " ",
        " ",
        "+@frappe.whitelist()",
        "+def get_link_status(import_name):",
        "+    doc = frappe.get_doc(\"ECRM Data Import\", import_name)",
        "+    doc.check_permission(\"read\")",
        "+    rows = _review_rows(import_name)",
        "+    linked = [r for r in rows if r.lead]",
        "+    return {\"total\": len(rows), \"linked\": len(linked), \"pending\": len(rows) - len(linked)}",
        "+",
        "+",
        "+def _row_matches_lead(row, lead):",
        "+    \"\"\"Match on normalised email first, then on phone digits.\"\"\"",
        "+    if row.get(\"email\") and lead.get(\"email\"):",
        "+        return row[\"email\"].strip().lower() == lead[\"email\"].strip().lower()",
        "+    digits = lambda s: \"\".join(ch for ch in (s or \"\") if ch.isdigit())",
        "+    return bool(digits(row.get(\"phone\"))) and digits(row.get(\"phone\")) == digits(lead.get(\"phone\"))",
        " def on_import_complete(doc, method=None):",
        "     frappe.enqueue(\"entries_crm.api.import_links.relink\", import_name=doc.name)",
      ]),
    ]),
    makeFile("entries_crm/api/importer.py", [
      makeHunk(118, 118, "class RowImporter:", [
        "     def commit_row(self, row):",
        "         doc = self.build_doc(row)",
        "-        doc.insert()",
        "+        doc.insert(ignore_permissions=True)",
        "+        row.lead = doc.name",
        "         self.created += 1",
        "         return doc",
      ]),
    ]),
    makeFile("frontend/src/import/ReviewTable.tsx", [
      makeHunk(36, 36, "export function ReviewTable({ importName }: Props) {", [
        "   const [page, setPage] = useState(0);",
        "   const { rows, total } = useImportRows(importName, page);",
        "+  const { link, linking } = useImportLinks(importName);",
        "+  const [selected, setSelected] = useState<Set<string>>(new Set());",
        " ",
        "   return (",
        "     <Table>",
      ]),
      makeHunk(80, 82, "export function ReviewTable({ importName }: Props) {", [
        "       <Pagination",
        "         page={page}",
        "         total={total}",
        "-        onChange={setPage}",
        "+        onChange={(p) => {",
        "+          setPage(p);",
        "+          setSelected(new Set());",
        "+        }}",
        "       />",
        "+      <Button disabled={!selected.size} onClick={() => link([...selected])}>",
        "+        Link {selected.size} rows",
        "+      </Button>",
      ]),
    ]),
    newFile("frontend/src/import/useImportLinks.ts", [
      "import { useMutation, useQueryClient } from \"@tanstack/react-query\";",
      "import { call } from \"@/lib/frappe\";",
      "",
      "export function useImportLinks(importName: string) {",
      "  const qc = useQueryClient();",
      "  const m = useMutation({",
      "    mutationFn: (rows: string[]) =>",
      "      call(\"entries_crm.api.import_links.link_import_rows\", { import_name: importName, rows: JSON.stringify(rows) }),",
      "    onSuccess: () => qc.invalidateQueries({ queryKey: [\"import-rows\", importName] }),",
      "  });",
      "  return { link: m.mutate, linking: m.isPending };",
      "}",
    ]),
    newFile("tests/test_import_links.py", [
      "import json",
      "import frappe",
      "from frappe.tests.utils import FrappeTestCase",
      "",
      "from entries_crm.api.import_links import link_import_rows",
      "",
      "",
      "class TestImportLinks(FrappeTestCase):",
      "    def test_links_rows_to_leads(self):",
      "        imp = make_import(rows=2)",
      "        link_import_rows(imp.name, json.dumps([r.lead for r in imp.rows]))",
      "        self.assertEqual(frappe.db.count(\"ECRM Lead\", {\"import_ref\": imp.name}), 2)",
    ]),
  ];
}

function filesMr29(): FileDiff[] {
  return [
    makeFile("entries_crm/ai/audit.py", [
      makeHunk(1, 1, "", [
        " import frappe",
        "+from frappe.utils import now_datetime",
        " ",
        "+",
        "+def record_tool_call(session, tool, args, result_size):",
        "+    frappe.get_doc({",
        "+        \"doctype\": \"ECRM AI Tool Call\",",
        "+        \"session\": session,",
        "+        \"tool\": tool,",
        "+        \"args\": frappe.as_json(args),",
        "+        \"result_size\": result_size,",
        "+        \"called_at\": now_datetime(),",
        "+    }).insert(ignore_permissions=True)",
      ]),
    ]),
    makeFile("entries_crm/ai/agent.py", [
      makeHunk(64, 64, "class Agent:", [
        "     def run_tool(self, name, args):",
        "         result = self.tools[name](**args)",
        "+        record_tool_call(self.session, name, args, len(str(result)))",
        "         return result",
      ]),
    ]),
  ];
}

function filesMr28(): FileDiff[] {
  return [
    makeFile("entries_desk/sla/timer.py", [
      makeHunk(52, 52, "class SlaTimer:", [
        "     def on_reopen(self, ticket):",
        "         \"\"\"Resume the SLA clock when a resolved ticket is reopened.\"\"\"",
        "         self.resumed_at = now_datetime()",
        "-        remaining = self.target_seconds - self.elapsed_seconds + self.paused_seconds",
        "+        self.paused_seconds = 0",
        "+        remaining = self.target_seconds - self.elapsed_seconds + self.paused_seconds",
        "         self.due_at = add_to_date(self.resumed_at, seconds=remaining)",
        "         self.save_state(ticket)",
      ]),
    ]),
    makeFile("entries_desk/sla/test_sla.py", [
      makeHunk(130, 130, "class TestSla(FrappeTestCase):", [
        "     def test_resume_after_reopen(self):",
        "         t = make_ticket(sla=\"2h\")",
        "+        pause(t, hours=1)",
        "         resolve(t)",
        "         reopen(t)",
      ]),
    ]),
  ];
}

function filesMr27(): FileDiff[] {
  return [
    makeFile("entries_crm/api/ai_tools.py", [
      makeHunk(22, 22, "def run_report(report_name, filters=None):", [
        " @frappe.whitelist()",
        " def run_report(report_name, filters=None):",
        "+    frappe.has_permission(\"Report\", \"read\", report_name, throw=True)",
        "     filters = frappe.parse_json(filters or \"{}\")",
        "-    return frappe.get_doc(\"Report\", report_name).get_data(filters=filters, ignore_permissions=True)",
        "+    return frappe.get_doc(\"Report\", report_name).get_data(filters=filters)",
      ]),
    ]),
  ];
}

export const LOG_UNIT_FAIL = [
  "$ bench --site desk_test.localhost run-tests --app entries_desk --module entries_desk.sla.test_sla",
  "Running 12 tests",
  "test_business_hours_only ... ok",
  "test_first_response_breach ... ok",
  "test_holiday_calendar ... ok",
  "test_pause_on_customer_reply ... ok",
  "test_resume_after_reopen ... FAIL",
  "AssertionError: 7200.0 != 3600.0 : remaining seconds after reopen",
  '  File "entries_desk/sla/test_sla.py", line 141, in test_resume_after_reopen',
  "Ran 12 tests in 251.4s · FAILED (failures=1)",
];

export const LOG_GITLAB_BUILD = [
  "$ gitlab-ci-local --job build",
  "Using docker executor with image python:3.11-slim",
  "build starting python:3.11-slim (build)",
  "build $ pip install -e .",
  "build > Collecting frappe-bench",
  "build > Installing collected packages: entries_desk",
  "build > Successfully installed entries_desk-0.4.2",
  "build $ python -m compileall -q entries_desk",
  "build $ yarn --cwd frontend build",
  "build > vite v6.4.6 building for production...",
  "build > ✓ 1284 modules transformed.",
  "build > dist/assets/index.js  412.18 kB │ gzip: 121.40 kB",
  "build > ✓ built in 9.81s",
  "build finished in 1m 48s",
  "PASS build",
];

const passLog = (cmd: string, tail: string[]) => [`$ ${cmd}`, ...tail].join("\n");

export function createSeed(now: number = Date.now()): Seed {
  const settings: AppSettings = {
    claude_cli_path: "/opt/homebrew/bin/claude",
    model: "opus",
    max_turns: 30,
    review_timeout_secs: 600,
    max_reviews_per_hour: 6,
    posting_paused: false,
    mcp_port: 7811,
    worktree_root: "~/.warden/worktrees",
    data_dir: "~/Library/Application Support/Warden",
  };

  const projects: Project[] = [
    { id: 1, forge: "gitlab", base_url: "https://gitlab.com", remote_id: "entries/entries_crm", name: "entries_crm", local_path: "~/Desktop/Entries/benches/crmv15/apps/entries_crm", target_branches: ["develop", "feat/*"], mode: "assist", poll_interval_secs: 120, enabled: true },
    { id: 2, forge: "gitlab", base_url: "https://gitlab.com", remote_id: "entries/entries_platform", name: "entries_platform", local_path: "~/Desktop/Entries/benches/crmv15/apps/entries_platform", target_branches: ["develop"], mode: "draft", poll_interval_secs: 300, enabled: true },
    { id: 3, forge: "gitlab", base_url: "https://gitlab.com", remote_id: "entries/entries_desk", name: "entries_desk", local_path: "~/Desktop/Entries/benches/crmv15/apps/entries_desk", target_branches: ["develop"], mode: "draft", poll_interval_secs: 300, enabled: true },
    { id: 4, forge: "gitlab", base_url: "https://gitlab.com", remote_id: "entries/entries_project", name: "entries_project", local_path: "~/Desktop/Entries/benches/crmv15/apps/entries_project", target_branches: ["develop"], mode: "draft", poll_interval_secs: 300, enabled: true },
  ];

  const mr = (m: Partial<MergeRequest> & Pick<MergeRequest, "id" | "project_id" | "iid" | "title">, shas: { base: string; start: string; head: string }): MergeRequest => ({
    description: "",
    author: "vinay",
    source_branch: "",
    target_branch: "develop",
    web_url: "",
    draft: false,
    labels: [],
    head_sha: shas.head,
    base_sha: shas.base,
    start_sha: shas.start,
    last_reviewed_sha: null,
    additions: 0,
    deletions: 0,
    state: "opened",
    updated_at: iso(now, 10),
    ...m,
  });

  const mrs: MergeRequest[] = [
    mr({ id: 1, project_id: 1, iid: 31, title: "feat(import): link review rows to leads", description: "Adds an endpoint to link reviewed import rows to existing leads, plus the review table UI.\n\nCloses #212.", source_branch: "feat/import-links", web_url: "https://gitlab.com/entries/entries_crm/-/merge_requests/31", additions: 412, deletions: 38, last_reviewed_sha: SHAS.mr31.head, labels: ["import"], updated_at: iso(now, 18) }, SHAS.mr31),
    mr({ id: 2, project_id: 1, iid: 29, title: "feat(ai): tool-call audit trail", source_branch: "feat/ai-audit", target_branch: "feat/ai-integration", web_url: "https://gitlab.com/entries/entries_crm/-/merge_requests/29", additions: 188, deletions: 12, labels: ["ai"], updated_at: iso(now, 3) }, SHAS.mr29),
    mr({ id: 3, project_id: 3, iid: 28, title: "fix(desk): SLA timer on reopen", source_branch: "fix/sla-reopen", web_url: "https://gitlab.com/entries/entries_desk/-/merge_requests/28", additions: 64, deletions: 21, last_reviewed_sha: SHAS.mr28.head, updated_at: iso(now, 40) }, SHAS.mr28),
    mr({ id: 4, project_id: 1, iid: 27, title: "fix(security): close review findings", source_branch: "fix/security-review", web_url: "https://gitlab.com/entries/entries_crm/-/merge_requests/27", additions: 96, deletions: 40, last_reviewed_sha: SHAS.mr27.head, labels: ["security", "ai-reviewed"], updated_at: iso(now, 160) }, SHAS.mr27),
    mr({ id: 5, project_id: 4, iid: 26, title: "Draft: projects wave 0 extraction", source_branch: "wave-0", web_url: "https://gitlab.com/entries/entries_project/-/merge_requests/26", additions: 1900, deletions: 310, draft: true, updated_at: iso(now, 300) }, SHAS.mr26),
  ];

  const run = (r: Partial<ReviewRun> & Pick<ReviewRun, "id" | "mr_id" | "status">, shas: { base: string; head: string }, startedMinAgo: number): ReviewRun => ({
    base_sha: shas.base,
    head_sha: shas.head,
    since_sha: null,
    model: "opus",
    turns: 0,
    duration_ms: 0,
    cli_session_id: null,
    summary: null,
    verdict: null,
    error: null,
    started_at: iso(now, startedMinAgo),
    ended_at: null,
    ...r,
  });

  const runs: ReviewRun[] = [
    run({ id: 104, mr_id: 4, status: "posted", turns: 14, duration_ms: 152_000, cli_session_id: "5b0f3c2e-11aa-4c55-9e1d-7c1f0e2d4a90", summary: "Permission checks are now enforced on run_report and ignore_permissions is gone. One remaining note on error handling.", verdict: "comment", ended_at: iso(now, 157) }, SHAS.mr27, 160),
    run({ id: 105, mr_id: 5, status: "skipped", error: "draft MR", ended_at: iso(now, 299) }, SHAS.mr26, 299),
    run({ id: 103, mr_id: 3, status: "awaiting_approval", turns: 17, duration_ms: 388_000, cli_session_id: "0e9a77b1-3b0c-4e7e-8f1f-2a5d0c6e9b13", summary: "The unit failure is caused by the MR: on_reopen() zeroes paused_seconds before computing the remaining time. Lint, secrets and semgrep pass.", verdict: "request_changes", ended_at: iso(now, 31) }, SHAS.mr28, 38),
    run({ id: 101, mr_id: 1, status: "awaiting_approval", turns: 22, duration_ms: 250_000, cli_session_id: "c2b8e5f4-6d1a-4f3e-9b2c-8a7d6e5f4c3b", summary: "2 blocking issues in the new linker endpoint (injection, permissions). Tests pass 3/3. Frontend looks fine apart from pagination state.", verdict: "request_changes", ended_at: iso(now, 14) }, SHAS.mr31, 18),
    run({ id: 102, mr_id: 2, status: "reviewing", turns: 9, cli_session_id: "8d4c2a1e-0f9b-4e8d-a7c6-b5e4d3c2b1a0" }, SHAS.mr29, 2),
  ];

  const findings: Finding[] = [
    // !31 — the mockup findings
    finding(SHAS.mr31, { id: 1001, run_id: 101, file: "entries_crm/api/import_links.py", line: 16, severity: "blocker", category: "security", rule_id: "FR-002", title: "SQL built with an f-string from request input", body: "`import_name` and each `r` come straight from the request, so this is injectable. Use `frappe.db.set_value` or a parameterised query.", suggestion: 'frappe.db.set_value("ECRM Lead", r, "import_ref", import_name)', evidence: "rows = json.loads(rows) on line 14; no sanitising before line 16 (semgrep FR-002 hit)", confidence: 0.98, status: "draft" }),
    finding(SHAS.mr31, { id: 1002, run_id: 101, file: "entries_crm/api/import_links.py", line: 13, severity: "major", category: "frappe-permissions", rule_id: "FR-001", title: "Whitelisted method has no permission check", body: "Any logged-in user can call this and relink rows of an import they cannot read. Check access to the import before touching rows.", suggestion: 'frappe.get_doc("ECRM Data Import", import_name).check_permission("write")', confidence: 0.91, status: "draft" }),
    finding(SHAS.mr31, { id: 1003, run_id: 101, file: "entries_crm/api/import_links.py", line: 17, severity: "major", category: "correctness", rule_id: "FR-007", title: "Explicit commit inside request", body: "Frappe commits at the end of the request. Committing per call here breaks rollback if a later row fails and leaves the import half-linked.", source: "rule", confidence: 0.9, status: "draft" }),
    finding(SHAS.mr31, { id: 1004, run_id: 101, file: "frontend/src/import/ReviewTable.tsx", line: 87, severity: "minor", category: "correctness", title: "Row state lost on page change", body: "Clearing `selected` on every page change drops rows the user ticked on earlier pages, so 'Link N rows' only links the current page. Keep the selection keyed by row name across pages.", evidence: "setSelected(new Set()) inside the Pagination onChange handler", confidence: 0.78, status: "draft" }),
    finding(SHAS.mr31, { id: 1005, run_id: 101, file: "frontend/src/import/ReviewTable.tsx", line: 38, severity: "minor", category: "ux", title: "Missing loading state", body: "`linking` from useImportLinks is never used, so the Link button can be clicked twice while the request is in flight.", confidence: 0.62, status: "rejected", guardrail_reason: "conf 0.62 · below threshold 0.70" }),
    finding(SHAS.mr31, { id: 1006, run_id: 101, file: "frontend/src/import/useImportLinks.ts", line: 4, severity: "nit", category: "style", title: "Hook name", body: "The hook both links and reports linking state; `useLinkImportRows` would say what it does and match the endpoint name.", confidence: 0.74, status: "rejected", guardrail_reason: "nits off" }),
    // !28 — SLA timer
    finding(SHAS.mr28, { id: 1101, run_id: 103, file: "entries_desk/sla/timer.py", line: 55, severity: "major", category: "correctness", title: "Paused time counted twice after reopen", body: "`on_reopen()` now resets `paused_seconds` to 0 before computing the remaining time, so the hour spent paused is counted twice. `test_resume_after_reopen` fails with 7200 != 3600; the test expectation looks right, the change in this MR is the bug.", suggestion: "remaining = self.target_seconds - self.elapsed_seconds + self.paused_seconds\nself.paused_seconds = 0", evidence: "unit: AssertionError: 7200.0 != 3600.0 : remaining seconds after reopen (test_sla.py:141)", source: "ci", confidence: 0.88, status: "draft" }),
    finding(SHAS.mr28, { id: 1102, run_id: 103, file: "entries_desk/sla/test_sla.py", line: 132, severity: "minor", category: "tests", title: "Test pauses for a fixed hour without freezing time", body: "`pause(t, hours=1)` relies on wall-clock time in other helpers; wrap the test in `freeze_time` so it is not flaky near midnight.", evidence: "other SLA tests use frappe.tests.utils.freeze_time", confidence: 0.74, status: "draft" }),
    // !27 — posted
    finding(SHAS.mr27, { id: 1201, run_id: 104, file: "entries_crm/api/ai_tools.py", line: 24, severity: "major", category: "frappe-permissions", rule_id: "FR-004", title: "Report data fetched with ignore_permissions", body: "get_data was called with ignore_permissions=True, so row-level permissions on the report's doctype were skipped.", confidence: 0.93, status: "resolved", forge_note_id: "disc-88f1" }),
    finding(SHAS.mr27, { id: 1202, run_id: 104, file: "entries_crm/api/ai_tools.py", line: 24, severity: "minor", category: "correctness", title: "has_permission result ignored when report is missing", body: "If report_name does not exist, has_permission raises DoesNotExistError before the permission error, which leaks report names. Check existence with frappe.db.exists first.", confidence: 0.81, status: "posted", forge_note_id: "disc-88f2" }),
    finding(SHAS.mr27, { id: 1203, run_id: 104, file: "entries_crm/api/ai_tools.py", line: 25, severity: "minor", category: "correctness", title: "filters parsed without type check", body: "parse_json accepts a list as well as a dict; get_data expects a dict and fails with a 500 when a list is sent.", confidence: 0.79, status: "posted", forge_note_id: "disc-88f3" }),
  ];

  const job = (j: Partial<CiJob> & Pick<CiJob, "id" | "run_id" | "name" | "status">): CiJob => ({
    kind: "recipe",
    command: "",
    duration_ms: 0,
    log_path: `~/Library/Application Support/Warden/logs/${j.run_id}/${j.name}.log`,
    failure_excerpt: null,
    ...j,
  });

  const ciJobs: CiJob[] = [
    job({ id: 501, run_id: 101, name: "lint", command: "ruff check {changed_py} && npx eslint {changed_ts}", status: "passed", duration_ms: 14_000 }),
    job({ id: 502, run_id: 101, name: "secrets", kind: "check", command: "secret scanner · diff only", status: "passed", duration_ms: 2_000 }),
    job({ id: 503, run_id: 101, name: "unit", command: "bench --site crm_test.localhost run-tests --app entries_crm --module entries_crm.tests.test_import_links", status: "passed", duration_ms: 96_000 }),

    job({ id: 511, run_id: 102, name: "lint", command: "ruff check {changed_py}", status: "passed", duration_ms: 9_000 }),
    job({ id: 512, run_id: 102, name: "secrets", kind: "check", command: "secret scanner · diff only", status: "passed", duration_ms: 2_000 }),
    job({ id: 513, run_id: 102, name: "unit", command: "bench --site crm_test.localhost run-tests --app entries_crm --module entries_crm.ai.test_audit", status: "running" }),

    job({ id: 521, run_id: 103, name: "lint", command: "ruff + eslint · changed files", status: "passed", duration_ms: 18_000 }),
    job({ id: 522, run_id: 103, name: "secrets", kind: "check", command: "gitleaks · diff only", status: "passed", duration_ms: 3_000 }),
    job({ id: 523, run_id: 103, name: "semgrep", kind: "check", command: "frappe ruleset · 14 rules", status: "passed", duration_ms: 11_000 }),
    job({ id: 524, run_id: 103, name: "unit", command: "bench --site desk_test.localhost run-tests --app entries_desk --module entries_desk.sla.test_sla", status: "failed", duration_ms: 252_000, failure_excerpt: LOG_UNIT_FAIL.slice(6).join("\n") }),
    job({ id: 525, run_id: 103, name: "gitlab-ci:build", kind: "gitlab_ci_local", command: "gitlab-ci-local --job build", status: "running" }),

    job({ id: 531, run_id: 104, name: "lint", command: "ruff check {changed_py}", status: "passed", duration_ms: 11_000 }),
    job({ id: 532, run_id: 104, name: "secrets", kind: "check", command: "secret scanner · diff only", status: "passed", duration_ms: 2_000 }),
    job({ id: 533, run_id: 104, name: "unit", command: "bench --site crm_test.localhost run-tests --app entries_crm --module entries_crm.tests.test_ai_tools", status: "passed", duration_ms: 71_000 }),
  ];

  const logs: Record<number, string> = {
    501: passLog("ruff check entries_crm/api/import_links.py entries_crm/api/importer.py tests/test_import_links.py && npx eslint frontend/src/import/ReviewTable.tsx frontend/src/import/useImportLinks.ts", ["All checks passed!", "✓ eslint: 0 problems"]),
    502: passLog("warden secret-scan --diff", ["scanned 5 files, 0 secrets"]),
    503: passLog("bench --site crm_test.localhost run-tests --app entries_crm --module entries_crm.tests.test_import_links", ["Running 4 tests", "test_links_rows_to_leads ... ok", "test_skips_unmatched ... ok", "test_status_counts ... ok", "test_relink_on_complete ... ok", "Ran 4 tests in 94.2s · OK"]),
    511: passLog("ruff check entries_crm/ai/audit.py entries_crm/ai/agent.py", ["All checks passed!"]),
    512: passLog("warden secret-scan --diff", ["scanned 2 files, 0 secrets"]),
    513: ["$ bench --site crm_test.localhost run-tests --app entries_crm --module entries_crm.ai.test_audit", "Running 6 tests"].join("\n"),
    521: passLog("ruff check entries_desk/sla/timer.py entries_desk/sla/test_sla.py", ["All checks passed!"]),
    522: passLog("gitleaks detect --no-git --source <diff>", ["no leaks found"]),
    523: passLog("semgrep --config rules/ --json", ["Ran 14 rules on 2 files: 0 findings."]),
    524: LOG_UNIT_FAIL.join("\n"),
    525: LOG_GITLAB_BUILD.slice(0, 4).join("\n"),
    531: passLog("ruff check entries_crm/api/ai_tools.py", ["All checks passed!"]),
    532: passLog("warden secret-scan --diff", ["scanned 1 file, 0 secrets"]),
    533: passLog("bench --site crm_test.localhost run-tests --app entries_crm --module entries_crm.tests.test_ai_tools", ["Running 3 tests", "test_run_report_requires_read ... ok", "test_run_report_filters ... ok", "test_run_report_missing ... ok", "Ran 3 tests in 69.0s · OK"]),
  };

  let auditId = 9000;
  const ev = (run_id: number | null, kind: string, payload: unknown, minAgo: number): AuditEvent => ({ id: ++auditId, run_id, kind, payload, created_at: iso(now, minAgo) });
  const audit: AuditEvent[] = [
    ev(104, "engine", { event: "start", model: "opus", max_turns: 30 }, 160),
    ev(104, "tool_call", { tool: "mr_get_overview" }, 159.8),
    ev(104, "tool_call", { tool: "mr_get_diff", args: { page: 1 } }, 159.6),
    ev(104, "engine", { event: "result", turns: 14, duration_ms: 152000 }, 157.4),
    ev(104, "post", { finding_id: 1201, note: "disc-88f1", kind: "line" }, 157),
    ev(104, "post", { finding_id: 1202, note: "disc-88f2", kind: "line" }, 157),
    ev(104, "post", { finding_id: 1203, note: "disc-88f3", kind: "line" }, 157),
    ev(104, "post", { summary: true, note: "note-4410" }, 157),
    ev(104, "post", { resolved: "disc-88f1", reason: "line fixed in a819b62" }, 60),
    ev(105, "guardrail", { skipped: "draft MR" }, 299),
    ev(103, "engine", { event: "start", model: "opus", max_turns: 30 }, 38),
    ev(103, "tool_call", { tool: "ci_get_results" }, 37),
    ev(103, "tool_call", { tool: "repo_read_file", args: { path: "entries_desk/sla/timer.py", start_line: 40, end_line: 80 } }, 36),
    ev(103, "tool_call", { tool: "review_add_finding", args: { file: "entries_desk/sla/timer.py", line: 55 }, result: "accepted" }, 33),
    ev(103, "engine", { event: "result", turns: 17, duration_ms: 388000 }, 31),
    ev(101, "engine", { event: "start", model: "opus", max_turns: 30 }, 18),
    ev(101, "tool_call", { tool: "mr_get_overview" }, 17.9),
    ev(101, "tool_call", { tool: "mr_get_diff", args: { file: "entries_crm/api/import_links.py" } }, 17.7),
    ev(101, "tool_call", { tool: "repo_search", args: { query: "link_import_rows" } }, 17.2),
    ev(101, "guardrail", { finding: "Missing loading state", decision: "rejected", reason: "conf 0.62 · below threshold 0.70" }, 15.1),
    ev(101, "guardrail", { finding: "Hook name", decision: "rejected", reason: "nits off" }, 15),
    ev(101, "engine", { event: "result", turns: 22, duration_ms: 250000, verdict: "request_changes" }, 14),
    ev(102, "engine", { event: "start", model: "opus", max_turns: 30 }, 2),
    ev(102, "tool_call", { tool: "mr_get_overview" }, 1.9),
  ];

  const hist = historyData(now);
  mrs.push(...hist.mrs);
  runs.push(...hist.runs);
  findings.push(...hist.findings);

  return {
    settings,
    engine: {
      cli_path: "/opt/homebrew/bin/claude",
      version: "2.1.330 (Claude Code)",
      logged_in: true,
      subscription: "Claude Max",
      api_key_in_env: false,
      ok: true,
      message: "Ready · subscription login",
    },
    tools: [
      { name: "claude", installed: true, version: "2.1.330" },
      { name: "git", installed: true, version: "2.50.1" },
      { name: "rg", installed: true, version: "14.1.1" },
      { name: "docker", installed: true, version: "28.3.2" },
      { name: "semgrep", installed: false, version: null },
      { name: "gitleaks", installed: false, version: null },
      { name: "gitlab-ci-local", installed: false, version: null },
      { name: "act", installed: false, version: null },
    ],
    forges: [{ forge: "gitlab", base_url: "https://gitlab.com", user: "warden-bot", ok: true }],
    remoteProjects: [
      ["entries/entries_crm", "entries_crm"],
      ["entries/entries_platform", "entries_platform"],
      ["entries/entries_desk", "entries_desk"],
      ["entries/entries_project", "entries_project"],
      ["entries/entries_lnd", "entries_lnd"],
      ["entries/entries_ui", "entries_ui"],
      ["entries/beacon", "beacon"],
      ["entries/warden", "warden"],
    ],
    projects,
    configs: { 1: crmConfig(), 2: defaultConfig(), 3: defaultConfig(), 4: defaultConfig() },
    overrides: { 1: { disabled_rules: ["GJ-003"] } },
    mrs,
    runs,
    findings,
    ciJobs,
    files: { 101: filesMr31(), 102: filesMr29(), 103: filesMr28(), 104: filesMr27(), 105: [] },
    logs,
    audit,
  };
}
