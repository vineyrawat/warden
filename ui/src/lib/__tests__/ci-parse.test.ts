import { describe, expect, it } from "vitest";
import type { CiJob } from "@/bindings/CiJob";
import {
  failureLocation,
  groupStages,
  jobStage,
  logSections,
  matchDiffPath,
  missingTool,
  normalizePath,
  parseFailures,
  summarizeLog,
} from "@/lib/ci-parse";
import { LOG_GITLAB_BUILD, LOG_UNIT_FAIL } from "@/lib/mock-data";

const job = (name: string, kind = "recipe", command = ""): Pick<CiJob, "name" | "kind" | "command"> => ({ name, kind, command });

describe("parseFailures: pytest", () => {
  it("reads the short summary and attaches traceback line numbers", () => {
    const log = [
      "$ pytest -q tests/",
      "..F.F",
      "=================================== FAILURES ===================================",
      "________________________ test_import_rows_rejects_guest ________________________",
      "",
      "    def test_import_rows_rejects_guest():",
      ">       with pytest.raises(frappe.PermissionError):",
      "E       Failed: DID NOT RAISE <class 'frappe.exceptions.PermissionError'>",
      "",
      "tests/test_importer.py:22: Failed",
      "___________________________ TestRollback.test_rollback ___________________________",
      "tests/test_importer.py:41: in test_rollback",
      "    assert count == 0",
      "E       assert 399 == 0",
      "tests/test_importer.py:44: AssertionError",
      "=========================== short test summary info ============================",
      "FAILED tests/test_importer.py::test_import_rows_rejects_guest - AssertionError: PermissionError not raised",
      "FAILED tests/test_importer.py::TestRollback::test_rollback",
      "========================= 2 failed, 41 passed in 91.20s =========================",
    ].join("\n");
    expect(parseFailures(log)).toEqual([
      { file: "tests/test_importer.py", line: 22, test: "test_import_rows_rejects_guest", message: "AssertionError: PermissionError not raised", kind: "test" },
      { file: "tests/test_importer.py", line: 44, test: "test_rollback", message: "assert 399 == 0", kind: "test" },
    ]);
  });

  it("handles summary lines without a traceback (mockup log)", () => {
    const f = parseFailures("FAILED test_importer.py::test_import_rows_rollback - 399 != 0\nERROR test_x.py::test_setup");
    expect(f).toEqual([
      { file: "test_importer.py", test: "test_import_rows_rollback", message: "399 != 0", kind: "test" },
      { file: "test_x.py", test: "test_setup", message: "Error", kind: "test" },
    ]);
  });
});

describe("parseFailures: Frappe bench run-tests / unittest", () => {
  it("parses the mock unit log (verbose FAIL + inline traceback)", () => {
    expect(parseFailures(LOG_UNIT_FAIL.join("\n"))).toEqual([
      {
        file: "entries_desk/sla/test_sla.py",
        line: 141,
        test: "test_resume_after_reopen",
        message: "AssertionError: 7200.0 != 3600.0 : remaining seconds after reopen",
        kind: "test",
      },
    ]);
  });

  it("parses FAIL:/ERROR: blocks with absolute bench paths", () => {
    const log = [
      "$ bench --site test.localhost run-tests --app entries_crm",
      "test_import_rows_creates_log (entries_crm.tests.test_importer.TestImporter.test_import_rows_creates_log) ... ok",
      "test_import_rows_rejects_guest (entries_crm.tests.test_importer.TestImporter.test_import_rows_rejects_guest) ... FAIL",
      "test_missing_doctype (entries_crm.tests.test_importer.TestImporter.test_missing_doctype) ... ERROR",
      "",
      "======================================================================",
      "FAIL: test_import_rows_rejects_guest (entries_crm.tests.test_importer.TestImporter.test_import_rows_rejects_guest)",
      "----------------------------------------------------------------------",
      "Traceback (most recent call last):",
      '  File "/home/frappe/frappe-bench/apps/entries_crm/entries_crm/tests/test_importer.py", line 22, in test_import_rows_rejects_guest',
      "    with self.assertRaises(frappe.PermissionError):",
      "AssertionError: PermissionError not raised",
      "",
      "======================================================================",
      "ERROR: test_missing_doctype (entries_crm.tests.test_importer.TestImporter.test_missing_doctype)",
      "----------------------------------------------------------------------",
      "Traceback (most recent call last):",
      '  File "/home/frappe/frappe-bench/apps/frappe/frappe/model/document.py", line 120, in get_doc',
      "frappe.exceptions.DoesNotExistError: DocType Lead Import not found",
      "",
      "----------------------------------------------------------------------",
      "Ran 3 tests in 91.2s",
      "",
      "FAILED (failures=1, errors=1)",
    ].join("\n");
    expect(parseFailures(log)).toEqual([
      { file: "entries_crm/tests/test_importer.py", line: 22, test: "test_import_rows_rejects_guest", message: "AssertionError: PermissionError not raised", kind: "test" },
      { file: "entries_crm/tests/test_importer.py", test: "test_missing_doctype", message: "DoesNotExistError: DocType Lead Import not found", kind: "test" },
    ]);
  });
});

describe("parseFailures: linters and type checkers", () => {
  it("parses ruff and flake8 concise output", () => {
    const log = [
      "$ ruff check entries_crm/api/importer.py",
      "entries_crm/api/importer.py:3:8: F401 [*] `os` imported but unused",
      "./entries_crm/api/importer.py:58:89: E501 line too long (104 > 88)",
      "Found 2 errors.",
    ].join("\n");
    expect(parseFailures(log)).toEqual([
      { file: "entries_crm/api/importer.py", line: 3, col: 8, code: "F401", message: "`os` imported but unused", kind: "lint" },
      { file: "entries_crm/api/importer.py", line: 58, col: 89, code: "E501", message: "line too long (104 > 88)", kind: "lint" },
    ]);
  });

  it("parses ruff's full output format", () => {
    const log = ["F841 Local variable `rows` is assigned to but never used", "  --> entries_crm/api/importer.py:12:5", "   |"].join("\n");
    expect(parseFailures(log)).toEqual([{ file: "entries_crm/api/importer.py", line: 12, col: 5, code: "F841", message: "Local variable `rows` is assigned to but never used", kind: "lint" }]);
  });

  it("parses eslint stylish output, errors only", () => {
    const log = [
      "$ npx eslint frontend/src/import/ReviewTable.tsx",
      "",
      "/Users/me/worktrees/entries_crm/41/frontend/src/import/ReviewTable.tsx",
      "  12:7   error    'rows' is assigned a value but never used  @typescript-eslint/no-unused-vars",
      "  40:1   warning  Unexpected console statement               no-console",
      "",
      "✖ 2 problems (1 error, 1 warning)",
    ].join("\n");
    expect(parseFailures(log)).toEqual([
      {
        file: "/Users/me/worktrees/entries_crm/41/frontend/src/import/ReviewTable.tsx",
        line: 12,
        col: 7,
        code: "@typescript-eslint/no-unused-vars",
        message: "'rows' is assigned a value but never used",
        kind: "lint",
      },
    ]);
  });

  it("parses eslint unix format", () => {
    expect(parseFailures("src/a.ts:4:10: 'x' is not defined. [Error/no-undef]\nsrc/a.ts:5:1: Unexpected console [Warning/no-console]")).toEqual([
      { file: "src/a.ts", line: 4, col: 10, code: "no-undef", message: "'x' is not defined.", kind: "lint" },
    ]);
  });

  it("parses tsc in both formats", () => {
    const log = [
      "src/import/useImportLinks.ts(22,5): error TS2322: Type 'string' is not assignable to type 'number'.",
      "src/import/ReviewTable.tsx:40:12 - error TS2339: Property 'rowz' does not exist on type 'Props'.",
      "Found 2 errors in 2 files.",
    ].join("\n");
    expect(parseFailures(log)).toEqual([
      { file: "src/import/useImportLinks.ts", line: 22, col: 5, code: "TS2322", message: "Type 'string' is not assignable to type 'number'.", kind: "type" },
      { file: "src/import/ReviewTable.tsx", line: 40, col: 12, code: "TS2339", message: "Property 'rowz' does not exist on type 'Props'.", kind: "type" },
    ]);
  });

  it("parses mypy errors", () => {
    expect(parseFailures('entries_crm/api/importer.py:31: error: Incompatible return value type (got "str", expected "int")  [return-value]')).toEqual([
      { file: "entries_crm/api/importer.py", line: 31, code: "return-value", message: 'Incompatible return value type (got "str", expected "int")', kind: "type" },
    ]);
  });
});

describe("parseFailures: scanners", () => {
  it("parses gitleaks verbose findings", () => {
    const log = [
      "$ gitleaks detect --no-git --source . -v",
      "Finding:     api_key = \"sk_live_REDACTED\"",
      "Secret:      sk_live_REDACTED",
      "RuleID:      stripe-access-token",
      "Entropy:     4.21",
      "File:        entries_crm/integrations/stripe.py",
      "Line:        14",
      "Fingerprint: entries_crm/integrations/stripe.py:stripe-access-token:14",
      "",
      "WRN leaks found: 1",
    ].join("\n");
    expect(parseFailures(log)).toEqual([{ file: "entries_crm/integrations/stripe.py", line: 14, code: "stripe-access-token", message: "Secret detected (stripe-access-token)", kind: "secret" }]);
  });

  it("falls back to the gitleaks summary when there are no details", () => {
    expect(parseFailures("WRN leaks found: 3")).toEqual([{ file: "", message: "gitleaks found 3 leaks", kind: "secret" }]);
  });

  it("parses semgrep text output", () => {
    const log = [
      "┌─────────────────┐",
      "│ 1 Code Finding  │",
      "└─────────────────┘",
      "",
      "    entries_crm/api/importer.py",
      "    ❯❱ rules.frappe-sql-injection",
      "          User input reaches frappe.db.sql without parameters.",
      "",
      "           42┆ frappe.db.sql(f\"select * from tabLead where name='{name}'\")",
      "",
      "Ran 14 rules on 2 files: 1 finding.",
    ].join("\n");
    expect(parseFailures(log)).toEqual([
      { file: "entries_crm/api/importer.py", line: 42, code: "frappe-sql-injection", message: "User input reaches frappe.db.sql without parameters.", kind: "scan" },
    ]);
  });

  it("parses semgrep --json and summary-only output", () => {
    const json = JSON.stringify({ results: [{ check_id: "rules.no-eval", path: "app/x.py", start: { line: 3, col: 1 }, extra: { message: "eval() on input" } }] });
    expect(parseFailures(`$ semgrep --json\n${json}`)).toEqual([{ file: "app/x.py", line: 3, col: 1, code: "no-eval", message: "eval() on input", kind: "scan" }]);
    expect(parseFailures("Ran 14 rules on 2 files: 2 findings.")).toEqual([{ file: "", message: "semgrep reported 2 findings", kind: "scan" }]);
    expect(parseFailures("Ran 14 rules on 2 files: 0 findings.")).toEqual([]);
  });

  it("returns nothing for clean logs", () => {
    expect(parseFailures(LOG_GITLAB_BUILD.join("\n"))).toEqual([]);
    expect(parseFailures("$ ruff check x.py\nAll checks passed!")).toEqual([]);
    expect(parseFailures("")).toEqual([]);
    expect(parseFailures(null)).toEqual([]);
  });
});

describe("paths", () => {
  it("normalizes bench paths", () => {
    expect(normalizePath("/home/frappe/frappe-bench/apps/entries_crm/entries_crm/api/x.py")).toBe("entries_crm/api/x.py");
    expect(normalizePath("./src/a.ts")).toBe("src/a.ts");
  });
  it("matches diff paths by suffix", () => {
    const diff = ["entries_crm/tests/test_importer.py", "frontend/src/import/ReviewTable.tsx"];
    expect(matchDiffPath("test_importer.py", diff)).toBe("entries_crm/tests/test_importer.py");
    expect(matchDiffPath("/Users/me/wt/41/frontend/src/import/ReviewTable.tsx", diff)).toBe("frontend/src/import/ReviewTable.tsx");
    expect(matchDiffPath("entries_crm/tests/test_other.py", diff)).toBeNull();
    expect(matchDiffPath("", diff)).toBeNull();
  });
  it("formats locations", () => {
    expect(failureLocation({ file: "a.py", line: 3 })).toBe("a.py:3");
    expect(failureLocation({ file: "a.py" })).toBe("a.py");
  });
});

describe("summarizeLog", () => {
  it("reads pytest and unittest summaries", () => {
    expect(summarizeLog("========= 2 failed, 41 passed in 91.2s =========")).toBe("2 failed, 41 passed");
    expect(summarizeLog(LOG_UNIT_FAIL.join("\n"))).toBe("1 failed, 11 passed");
    expect(summarizeLog("Ran 4 tests in 94.2s · OK")).toBe("4 passed");
    expect(summarizeLog("Ran 3 tests in 91.2s\n\nFAILED (failures=1, errors=1)")).toBe("2 failed, 1 passed");
    expect(summarizeLog("All checks passed!")).toBeNull();
  });
});

describe("stages", () => {
  it("assigns stages from job name and command", () => {
    expect(jobStage(job("lint", "recipe", "ruff check {changed_py}"))).toBe("check");
    expect(jobStage(job("format", "recipe", "black --check ."))).toBe("check");
    expect(jobStage(job("unit", "recipe", "bench --site t run-tests --app entries_crm --module entries_crm.ai.test_audit"))).toBe("test");
    expect(jobStage(job("pytest"))).toBe("test");
    expect(jobStage(job("py", "recipe", "pytest -q"))).toBe("test");
    expect(jobStage(job("backend", "recipe", "bench --site t run-tests --app x"))).toBe("test");
    expect(jobStage(job("secrets", "check", "secret scanner · diff only"))).toBe("scan");
    expect(jobStage(job("semgrep", "check", "frappe ruleset · 14 rules"))).toBe("scan");
    expect(jobStage(job("leaks", "recipe", "gitleaks detect"))).toBe("scan");
    expect(jobStage(job("worktree"))).toBe("prepare");
    expect(jobStage(job("deps", "recipe", "pip install -e ."))).toBe("prepare");
    expect(jobStage(job("gitlab-ci:build", "gitlab_ci_local", "gitlab-ci-local --job build"))).toBe("gitlab-ci");
    expect(jobStage(job("docs", "recipe", "mkdocs build"))).toBe("check");
  });

  it("groups jobs in stage order and drops empty stages", () => {
    const jobs = [
      job("unit", "recipe", "bench run-tests"),
      job("gitlab-ci:build", "gitlab_ci_local"),
      job("semgrep", "check"),
      job("lint"),
      job("secrets", "check"),
    ];
    const g = groupStages(jobs);
    expect(g.map((s) => [s.label, s.jobs.map((j) => j.name)])).toEqual([
      ["Check", ["lint"]],
      ["Test", ["unit"]],
      ["Scan", ["semgrep", "secrets"]],
      ["GitLab CI", ["gitlab-ci:build"]],
    ]);
    expect(groupStages([])).toEqual([]);
  });

  it("names the tool to install for not-installed jobs", () => {
    expect(missingTool(job("gitlab-ci", "gitlab_ci_local", "gitlab-ci-local --job build"))).toBe("gitlab-ci-local");
    expect(missingTool(job("semgrep", "check", "frappe ruleset"))).toBe("semgrep");
    expect(missingTool(job("audit", "recipe", "pip-audit -r requirements.txt"))).toBe("pip-audit");
  });
});

describe("logSections", () => {
  it("starts a section at each $ command, with a preamble when output comes first", () => {
    const lines = ["warming up", "$ ruff check a.py", "All checks passed!", "$ npx eslint b.ts", "✓ 0 problems"];
    expect(logSections(lines)).toEqual([
      { title: null, start: 0, end: 1 },
      { title: "ruff check a.py", start: 1, end: 3 },
      { title: "npx eslint b.ts", start: 3, end: 5 },
    ]);
  });
  it("treats gitlab-ci-local `job $ cmd` lines as sections", () => {
    const s = logSections(LOG_GITLAB_BUILD);
    expect(s.map((x) => x.title)).toEqual(["gitlab-ci-local --job build", "pip install -e .", "python -m compileall -q entries_desk", "yarn --cwd frontend build"]);
    expect(s[s.length - 1].end).toBe(LOG_GITLAB_BUILD.length);
  });
  it("returns no sections for an empty log", () => {
    expect(logSections([])).toEqual([]);
  });
});
