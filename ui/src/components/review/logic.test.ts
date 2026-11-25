import { describe, expect, it } from "vitest";

import type { Finding } from "@/bindings/Finding";
import { buildDiffRows } from "@/lib/diff";
import { createSeed, makeFile, makeHunk } from "@/lib/mock-data";
import {
  afterDecision,
  anchorText,
  applyOverrides,
  ciTabLabel,
  codeContext,
  diffOrder,
  droppedFindings,
  droppedSummary,
  initialFindingId,
  nextUndecidedId,
  postDisabledReason,
  progressCounts,
  progressLine,
  pruneOverrides,
  reviewFiles,
  reviewKeyAction,
  sourceTag,
  stepId,
  stripMarkers,
  suggestionDiff,
  undoTarget,
  verdictInfo,
  walkthroughFindings,
} from "./logic";

const seed = createSeed(Date.parse("2026-11-25T10:00:00Z"));
const run101 = seed.findings.filter((f) => f.run_id === 101);
const files101 = seed.files[101];

const S = (id: number, status: Finding["status"]) => ({ id, status });

describe("walkthrough ordering", () => {
  it("steps through kept findings most severe first and lists dropped ones apart", () => {
    expect(walkthroughFindings(run101).map((f) => f.id)).toEqual([1001, 1002, 1003, 1004]);
    expect(droppedFindings(run101).map((f) => f.id)).toEqual([1005, 1006]);
  });

  it("opens the focused finding, else the first undecided", () => {
    const list = [S(1, "approved"), S(2, "draft"), S(3, "draft")];
    expect(initialFindingId(list, 3)).toBe(3);
    expect(initialFindingId(list, 99)).toBe(2);
    expect(initialFindingId([S(1, "posted")])).toBe(1);
    expect(initialFindingId([])).toBeNull();
  });

  it("finds the next undecided finding, wrapping and skipping the current one", () => {
    const list = [S(1, "draft"), S(2, "approved"), S(3, "dismissed"), S(4, "draft"), S(5, "posted")];
    expect(nextUndecidedId(list, 1)).toBe(4);
    expect(nextUndecidedId(list, 4)).toBe(1);
    expect(nextUndecidedId(list, 4, -1)).toBe(1);
    expect(nextUndecidedId(list, null)).toBe(1);
    expect(nextUndecidedId([S(1, "draft")], 1)).toBeNull();
    expect(nextUndecidedId([], null)).toBeNull();
  });

  it("auto-advances after a decision, or stays when nothing is left", () => {
    const list = [S(1, "approved"), S(2, "draft"), S(3, "draft")];
    expect(afterDecision(list, 1)).toBe(2);
    expect(afterDecision([S(1, "approved"), S(2, "dismissed")], 2)).toBe(2);
  });

  it("steps prev/next clamped to the ends", () => {
    const list = [S(1, "draft"), S(2, "draft"), S(3, "draft")];
    expect(stepId(list, 2, 1)).toBe(3);
    expect(stepId(list, 3, 1)).toBe(3);
    expect(stepId(list, 1, -1)).toBe(1);
    expect(stepId(list, 42, 1)).toBe(1);
    expect(stepId([], 1, 1)).toBeNull();
  });

  it("undoes the current decision, else the latest one still standing", () => {
    const list = [S(1, "approved"), S(2, "draft"), S(3, "dismissed")];
    expect(undoTarget(list, 1, [])).toBe(1);
    expect(undoTarget(list, 2, [1, 3])).toBe(3);
    expect(undoTarget([S(1, "draft"), S(2, "draft")], 2, [1])).toBeNull();
  });
});

describe("progress", () => {
  it("counts decisions and renders the progress line", () => {
    const p = progressCounts([S(1, "approved"), S(2, "approved"), S(3, "draft"), S(4, "draft"), S(5, "draft")]);
    expect(p).toEqual({ total: 5, approved: 2, dismissed: 0, undecided: 3, posted: 0 });
    expect(progressLine(2, p)).toBe("Finding 3 of 5 · 2 approved · 0 dismissed");
    expect(progressLine(0, progressCounts([S(1, "posted")]))).toBe("Finding 1 of 1 · 0 approved · 0 dismissed · 1 posted");
  });
});

describe("optimistic overrides", () => {
  it("applies overrides but never over posted findings", () => {
    const out = applyOverrides([S(1, "draft"), S(2, "posted")], new Map([[1, "approved"], [2, "draft"]] as const));
    expect(out.map((f) => f.status)).toEqual(["approved", "posted"]);
  });

  it("prunes overrides once the server agrees", () => {
    const m = new Map<number, Finding["status"]>([[1, "approved"], [2, "dismissed"]]);
    const next = pruneOverrides([S(1, "approved"), S(2, "draft")], m);
    expect([...next]).toEqual([[2, "dismissed"]]);
    expect(pruneOverrides([S(2, "draft")], next)).toBe(next);
  });
});

describe("keys", () => {
  it("maps single keys and ignores modifiers and form fields", () => {
    expect(reviewKeyAction({ key: "a" })).toBe("approve");
    expect(reviewKeyAction({ key: "E" })).toBe("edit");
    expect(reviewKeyAction({ key: "d" })).toBe("dismiss");
    expect(reviewKeyAction({ key: "u" })).toBe("undo");
    expect(reviewKeyAction({ key: "ArrowLeft" })).toBe("prev");
    expect(reviewKeyAction({ key: "ArrowRight" })).toBe("next");
    expect(reviewKeyAction({ key: "a", metaKey: true })).toBeNull();
    expect(reviewKeyAction({ key: "a", editable: true })).toBeNull();
    expect(reviewKeyAction({ key: "x" })).toBeNull();
  });
});

describe("code context", () => {
  const file = makeFile("a.py", [
    makeHunk(1, 1, "", Array.from({ length: 20 }, (_, i) => `+line ${i + 1}`)),
    makeHunk(40, 40, "def b", [" keep", "-old", "+new"]),
  ]);

  it("takes ±6 rows around the anchored line from its hunk", () => {
    const ctx = codeContext(file, { line: 10, side: "new" })!;
    expect(ctx.lines.map((l) => l.new_line)).toEqual([4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
    expect(ctx.lines[ctx.anchor].text).toBe("line 10");
  });

  it("clips at hunk edges and supports old-side anchors", () => {
    const top = codeContext(file, { line: 2, side: "new" })!;
    expect(top.lines[0].new_line).toBe(1);
    expect(top.anchor).toBe(1);
    const old = codeContext(file, { line: 41, side: "old" })!;
    expect(old.lines[old.anchor]).toMatchObject({ kind: "del", text: "old" });
    expect(old.header).toBe("@@ -40,2 +40,2 @@ def b");
  });

  it("returns null outside the diff", () => {
    expect(codeContext(file, { line: 30, side: "new" })).toBeNull();
    expect(codeContext(undefined, { line: 1, side: "new" })).toBeNull();
    expect(anchorText(file, { line: 41, side: "new" })).toBe("new");
  });

  it("works on the seeded run", () => {
    const f = run101.find((x) => x.id === 1001)!;
    const ctx = codeContext(files101.find((x) => x.new_path === f.file), f)!;
    expect(ctx.lines[ctx.anchor].text).toContain("frappe.db.sql(f");
  });

  it("renders a suggestion as − old line, + new lines", () => {
    expect(suggestionDiff("x = 1", "x = 2\ny = 3\n")).toEqual([
      { kind: "del", text: "x = 1" },
      { kind: "add", text: "x = 2" },
      { kind: "add", text: "y = 3" },
    ]);
    expect(suggestionDiff(null, "z")).toEqual([{ kind: "add", text: "z" }]);
  });
});

describe("diff tab", () => {
  it("adds placeholder files for findings outside the diff and orders by file then line", () => {
    const files = reviewFiles([makeFile("b.py", []), makeFile("a.py", [])], [{ file: "a.py" }, { file: "c.py" }]);
    expect(files.map((f) => f.new_path)).toEqual(["b.py", "a.py", "c.py"]);
    const order = diffOrder(files, [
      { id: 1, file: "c.py", line: 1 },
      { id: 2, file: "a.py", line: 9 },
      { id: 3, file: "b.py", line: 5 },
      { id: 4, file: "a.py", line: 2 },
    ]);
    expect(order.map((f) => f.id)).toEqual([3, 4, 2, 1]);
  });

  it("places strip markers by row position", () => {
    const file = files101[0];
    const mine = run101.filter((f) => f.file === file.new_path);
    const { rows, unanchored } = buildDiffRows(file, mine);
    const marks = stripMarkers(rows, unanchored);
    expect(marks.map((m) => m.id).sort()).toEqual(mine.map((f) => f.id).sort());
    expect(marks.every((m) => m.pct >= 0 && m.pct < 100)).toBe(true);
  });
});

describe("labels", () => {
  it("summarises guardrail drops like the mockup", () => {
    expect(droppedSummary(droppedFindings(run101))).toBe("Guardrails dropped 2: one below the confidence threshold, one nit (nits are off).");
    expect(droppedSummary([{ guardrail_reason: "duplicate of an existing discussion" }, { guardrail_reason: "near-duplicate of an existing discussion note" }])).toBe(
      "Guardrails dropped 2: two duplicates of open threads.",
    );
    expect(droppedSummary([])).toBe("Guardrails dropped nothing.");
  });

  it("labels sources, verdicts and CI", () => {
    const rules = [{ id: "FR-007", title: "", applies_to: null, instruction: null, pattern: "x", semgrep: null, severity: "major" as const }];
    expect(sourceTag({ source: "rule", rule_id: "FR-007" }, rules)).toBe("regex");
    expect(sourceTag({ source: "rule", rule_id: "FR-404" })).toBe("rule");
    expect(sourceTag({ source: "claude", rule_id: "FR-001" })).toBe("Claude");
    expect(sourceTag({ source: "ci", rule_id: null })).toBe("CI");
    expect(verdictInfo("request_changes")).toEqual({ label: "Request changes", tone: "orange" });
    expect(verdictInfo(null)).toBeNull();
    expect(ciTabLabel([{ status: "passed" }, { status: "failed" }]).label).toBe("1 failed");
    expect(ciTabLabel([{ status: "passed" }, { status: "running" }]).label).toBe("running");
    expect(ciTabLabel([{ status: "passed" }]).label).toBe("1 passed");
    expect(ciTabLabel([]).label).toBe("none");
  });

  it("explains why posting is disabled", () => {
    const base = { isLocal: false, paused: false, approved: 2, active: false, forgeName: "GitLab" };
    expect(postDisabledReason(base)).toBeNull();
    expect(postDisabledReason({ ...base, paused: true })).toMatch(/paused/);
    expect(postDisabledReason({ ...base, approved: 0 })).toMatch(/Approve at least one/);
    expect(postDisabledReason({ ...base, isLocal: true })).toMatch(/Local review/);
    expect(postDisabledReason({ ...base, active: true })).toMatch(/finish/);
  });
});
