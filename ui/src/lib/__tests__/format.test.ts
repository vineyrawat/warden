import { describe, expect, it } from "vitest";
import {
  buildFileTree,
  ciJobPill,
  ciSummaryPill,
  classifyLogLine,
  findingsToMarkdown,
  formatClock,
  formatCount,
  formatDuration,
  installHint,
  modeDescription,
  mrStatusText,
  ruleKind,
  sortFindings,
  splitGlobs,
} from "../format";
import { buildDiffRows } from "../diff";
import { createSeed, makeHunk } from "../mock-data";
import type { MrRow } from "@/bindings/MrRow";

describe("formatting", () => {
  it("formats durations like the mockup", () => {
    expect(formatDuration(18_000)).toBe("18s");
    expect(formatDuration(252_000)).toBe("4m 12s");
    expect(formatDuration(600_000)).toBe("10m");
    expect(formatDuration(3_780_000)).toBe("1h 3m");
    expect(formatDuration(0)).toBe("—");
  });

  it("formats counts and clocks", () => {
    expect(formatCount(412)).toBe("412");
    expect(formatCount(1900)).toBe("1.9k");
    expect(formatCount(12_400)).toBe("12k");
    expect(formatClock(102)).toBe("1:42");
    expect(formatClock(-3)).toBe("0:00");
  });

  it("splits glob lists", () => {
    expect(splitGlobs("develop, feat/*\nrelease/*,,")).toEqual(["develop", "feat/*", "release/*"]);
  });

  it("derives rule kind from the set field", () => {
    expect(ruleKind({ instruction: "x", pattern: null, semgrep: null })).toBe("claude");
    expect(ruleKind({ instruction: null, pattern: "a", semgrep: null })).toBe("regex");
    expect(ruleKind({ instruction: null, pattern: null, semgrep: "r.yml" })).toBe("semgrep");
  });

  it("describes posting modes", () => {
    expect(modeDescription("assist")).toContain("blockers and majors at confidence ≥ 0.85");
    expect(modeDescription("draft")).toMatch(/approve/);
  });

  it("has install hints for optional tools", () => {
    expect(installHint("semgrep")).toBe("brew install semgrep");
    expect(installHint("weird-tool")).toMatch(/weird-tool/);
  });
});

describe("findings helpers", () => {
  const seed = createSeed(Date.parse("2026-11-25T10:00:00Z"));
  const run101 = seed.findings.filter((f) => f.run_id === 101);

  it("sorts most severe first, then by confidence", () => {
    const sorted = sortFindings(run101);
    expect(sorted.map((f) => f.rule_id ?? f.title)).toEqual(["FR-002", "FR-001", "FR-007", "Row state lost on page change", "Missing loading state", "Hook name"]);
  });

  it("builds a file tree with counts and worst severity", () => {
    const tree = buildFileTree(seed.files[101], run101);
    const api = tree.find((g) => g.dir === "entries_crm/api/")!;
    const links = api.files.find((f) => f.name === "import_links.py")!;
    expect(links.count).toBe(3);
    expect(links.maxSeverity).toBe("blocker");
    expect(api.files.find((f) => f.name === "importer.py")!.count).toBe(0);
    expect(tree.map((g) => g.dir)).toEqual(["entries_crm/api/", "frontend/src/import/", "tests/"]);
  });

  it("exports markdown with suggestion blocks", () => {
    const md = findingsToMarkdown({ iid: 31, title: "feat(import): link review rows to leads" }, { summary: "Two issues.", head_sha: "7f3b2d1abc" }, run101.slice(0, 1));
    expect(md).toContain("## Warden review · !31");
    expect(md).toContain("### [Blocker] SQL built with an f-string from request input");
    expect(md).toContain("```suggestion");
    expect(md).toContain("— Warden · 7f3b2d1");
  });
});

describe("CI helpers", () => {
  it("colours log lines", () => {
    expect(classifyLogLine("test_resume_after_reopen ... FAIL")).toBe("error");
    expect(classifyLogLine("AssertionError: 7200.0 != 3600.0")).toBe("error");
    expect(classifyLogLine("test_first_response_breach ... ok")).toBe("ok");
    expect(classifyLogLine("$ bench --site x run-tests")).toBe("meta");
    expect(classifyLogLine("Ran 12 tests in 251.4s · FAILED (failures=1)")).toBe("error");
    expect(classifyLogLine("Running 12 tests")).toBe("meta");
    expect(classifyLogLine("plain output")).toBe("plain");
  });

  it("shows job durations when finished", () => {
    expect(ciJobPill({ status: "failed", duration_ms: 252_000 })).toBe("4m 12s");
    expect(ciJobPill({ status: "running", duration_ms: 0 })).toBe("running");
  });

  it("summarises CI for the inbox", () => {
    const row = (passed: number, failed: number, running: number, total: number) =>
      ({ ci_summary: { passed, failed, running, total }, latest_run: null }) as Pick<MrRow, "ci_summary" | "latest_run">;
    expect(ciSummaryPill(row(3, 0, 0, 3))).toEqual({ label: "3 / 3", tone: "green" });
    expect(ciSummaryPill(row(3, 1, 1, 5))).toEqual({ label: "1 failed", tone: "red" });
    expect(ciSummaryPill(row(2, 0, 1, 3))).toEqual({ label: "running", tone: "blue" });
    expect(ciSummaryPill(row(0, 0, 0, 0)).label).toBe("skipped");
  });
});

describe("mrStatusText", () => {
  const base = { draft: false, finding_counts: { draft: 4, posted: 0, rejected: 2 }, ci_summary: { passed: 3, failed: 0, running: 0, total: 3 } };
  const run = (status: string, turns = 0) => ({ status, turns, error: null }) as unknown as MrRow["latest_run"];
  it("matches the mockup wording", () => {
    expect(mrStatusText({ ...base, latest_run: run("awaiting_approval") } as MrRow, 30)).toBe("Awaiting approval · 6 findings");
    expect(mrStatusText({ ...base, latest_run: run("reviewing", 9) } as MrRow, 30)).toBe("Reviewing · turn 9 / 30");
    expect(mrStatusText({ ...base, latest_run: run("reviewing", 9) } as MrRow, 30, 12)).toBe("Reviewing · turn 12 / 30");
    expect(mrStatusText({ ...base, ci_summary: { ...base.ci_summary, failed: 1 }, finding_counts: { draft: 2, posted: 0, rejected: 0 }, latest_run: run("awaiting_approval") } as MrRow, 30)).toBe(
      "Awaiting approval · 2 findings + CI",
    );
    expect(mrStatusText({ ...base, draft: true, latest_run: run("skipped") } as MrRow, 30)).toBe("Skipped · draft MR");
  });
});

describe("diff rows", () => {
  it("numbers hunks like git and anchors findings to lines", () => {
    const h = makeHunk(10, 10, "import json", [" a", "+b", "+c", "-d", " e"]);
    expect(h.header).toBe("@@ -10,3 +10,4 @@ import json");
    expect(h.lines.map((l) => [l.kind, l.old_line, l.new_line])).toEqual([
      ["context", 10, 10],
      ["add", null, 11],
      ["add", null, 12],
      ["del", 11, null],
      ["context", 12, 13],
    ]);
    const file = { old_path: "x.py", new_path: "x.py", is_new: false, is_deleted: false, is_binary: false, hunks: [h] };
    const f = (id: number, line: number, side = "new") => ({ id, line, side, file: "x.py" }) as never;
    const { rows, unanchored } = buildDiffRows(file, [f(1, 12), f(2, 11, "old"), f(3, 99)]);
    const withFindings = rows.filter((r) => r.type === "line" && r.findings.length).map((r) => (r.type === "line" ? [r.line.text, r.findings.map((x) => x.id)] : null));
    expect(withFindings).toEqual([
      ["c", [1]],
      ["d", [2]],
    ]);
    expect(unanchored.map((x) => x.id)).toEqual([3]);
    expect(rows[0]).toMatchObject({ type: "hunk", header: h.header });
  });
});
