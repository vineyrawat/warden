import { describe, expect, it } from "vitest";
import type { MrRow } from "@/bindings/MrRow";
import type { ReviewRun } from "@/bindings/ReviewRun";
import type { RunStatus } from "@/bindings/RunStatus";
import {
  approvePostPlan,
  buildGroups,
  ciDot,
  defaultBaseBranch,
  findingsBySeverity,
  groupOf,
  matchesFilter,
  moveSelection,
  pendingFindings,
  resolveSelection,
  reviewActionLabel,
  severityCounts,
  shortRelative,
  verdictMeta,
  visibleIds,
} from "./inbox-logic";

function run(status: RunStatus, started = "2026-11-25T10:00:00Z"): ReviewRun {
  return {
    id: 1,
    mr_id: 1,
    base_sha: "",
    head_sha: "",
    since_sha: null,
    status,
    model: "",
    turns: 0,
    duration_ms: 0,
    cli_session_id: null,
    summary: null,
    verdict: null,
    error: null,
    started_at: started,
    ended_at: null,
  };
}

function row(id: number, p: Partial<MrRow> = {}): MrRow {
  return {
    id,
    project_id: 1,
    iid: id,
    title: `MR ${id}`,
    description: "",
    author: "rahul",
    source_branch: `feat/${id}`,
    target_branch: "develop",
    web_url: "",
    draft: false,
    labels: [],
    head_sha: "",
    base_sha: "",
    start_sha: "",
    last_reviewed_sha: null,
    additions: 0,
    deletions: 0,
    state: "opened",
    updated_at: "2026-11-25T10:00:00Z",
    project_name: "entries_crm",
    latest_run: null,
    finding_counts: { draft: 0, posted: 0, rejected: 0 },
    ci_summary: { passed: 0, failed: 0, running: 0, total: 0 },
    ...p,
  };
}

const counts = (draft: number, posted = 0, rejected = 0) => ({ draft, posted, rejected });

describe("groupOf", () => {
  it("puts findings to decide, failures and unreviewed MRs in Needs you", () => {
    expect(groupOf(row(1, { latest_run: run("awaiting_approval"), finding_counts: counts(3) }))).toBe("needs");
    expect(groupOf(row(1, { latest_run: run("failed") }))).toBe("needs");
    expect(groupOf(row(1, { latest_run: run("posted"), finding_counts: counts(1, 2) }))).toBe("needs");
    expect(groupOf(row(1))).toBe("needs");
  });
  it("puts every active status in Reviewing", () => {
    for (const s of ["queued", "preparing", "checking", "reviewing", "filtering", "posting"] as RunStatus[])
      expect(groupOf(row(1, { latest_run: run(s) }))).toBe("reviewing");
  });
  it("puts posted and clean reviews in Done", () => {
    expect(groupOf(row(1, { latest_run: run("posted"), finding_counts: counts(0, 2) }))).toBe("done");
    expect(groupOf(row(1, { latest_run: run("awaiting_approval"), finding_counts: counts(0, 0, 2) }))).toBe("done");
  });
  it("puts skipped, cancelled and unreviewed draft MRs in Skipped", () => {
    expect(groupOf(row(1, { latest_run: run("skipped") }))).toBe("skipped");
    expect(groupOf(row(1, { latest_run: run("cancelled") }))).toBe("skipped");
    expect(groupOf(row(1, { draft: true }))).toBe("skipped");
  });
});

describe("buildGroups", () => {
  it("returns four groups in order and sorts Needs you by kind then findings", () => {
    const rows = [
      row(1),
      row(2, { latest_run: run("failed") }),
      row(3, { latest_run: run("awaiting_approval"), finding_counts: counts(1) }),
      row(4, { latest_run: run("awaiting_approval"), finding_counts: counts(4) }),
      row(5, { latest_run: run("reviewing") }),
      row(6, { latest_run: run("posted"), finding_counts: counts(0, 1) }),
    ];
    const g = buildGroups(rows);
    expect(g.map((x) => x.key)).toEqual(["needs", "reviewing", "done", "skipped"]);
    expect(g[0].rows.map((r) => r.id)).toEqual([4, 3, 2, 1]);
    expect(g[1].rows.map((r) => r.id)).toEqual([5]);
    expect(g[2].rows.map((r) => r.id)).toEqual([6]);
    expect(g[3].rows).toEqual([]);
  });
  it("orders Done and Reviewing most recent first", () => {
    const g = buildGroups([
      row(1, { latest_run: run("posted"), updated_at: "2026-11-24T10:00:00Z" }),
      row(2, { latest_run: run("posted"), updated_at: "2026-11-25T10:00:00Z" }),
      row(3, { latest_run: run("queued", "2026-11-25T09:00:00Z") }),
      row(4, { latest_run: run("reviewing", "2026-11-25T11:00:00Z") }),
    ]);
    expect(g[2].rows.map((r) => r.id)).toEqual([2, 1]);
    expect(g[1].rows.map((r) => r.id)).toEqual([4, 3]);
  });
});

describe("matchesFilter", () => {
  const r = row(41, { title: "Import review: row-by-row importer", author: "rahul", source_branch: "feat/import-review" });
  it("matches iid with or without !", () => {
    expect(matchesFilter(r, "!41")).toBe(true);
    expect(matchesFilter(r, "41")).toBe(true);
    expect(matchesFilter(r, "!4")).toBe(false);
  });
  it("matches title, author, project and branch case-insensitively", () => {
    expect(matchesFilter(r, "IMPORTER")).toBe(true);
    expect(matchesFilter(r, "rahul")).toBe(true);
    expect(matchesFilter(r, "entries_crm")).toBe(true);
    expect(matchesFilter(r, "feat/import")).toBe(true);
    expect(matchesFilter(r, "acme")).toBe(false);
    expect(matchesFilter(r, "  ")).toBe(true);
  });
});

describe("selection", () => {
  const groups = buildGroups([row(1), row(2), row(3, { latest_run: run("reviewing") })]);
  it("skips collapsed groups", () => {
    expect(visibleIds(groups, new Set())).toEqual([2, 1, 3]);
    expect(visibleIds(groups, new Set(["needs"]))).toEqual([3]);
  });
  it("moves and clamps", () => {
    const ids = [2, 1, 3];
    expect(moveSelection(ids, 2, 1)).toBe(1);
    expect(moveSelection(ids, 3, 1)).toBe(3);
    expect(moveSelection(ids, 2, -1)).toBe(2);
    expect(moveSelection(ids, null, 1)).toBe(2);
    expect(moveSelection(ids, 99, -1)).toBe(3);
    expect(moveSelection([], 1, 1)).toBeNull();
  });
  it("resolves to the first row when the selection is hidden", () => {
    expect(resolveSelection([2, 1], 1)).toBe(1);
    expect(resolveSelection([2, 1], 7)).toBe(2);
    expect(resolveSelection([], 7)).toBeNull();
  });
});

describe("findings", () => {
  const f = (id: number, severity: "blocker" | "major" | "minor" | "nit", status: "draft" | "approved" | "dismissed" | "posted" | "rejected", confidence = 0.9) => ({
    id,
    severity,
    status,
    confidence,
    file: "a.py",
    line: id,
  });
  it("plans approve + post", () => {
    const plan = approvePostPlan([f(1, "blocker", "draft"), f(2, "major", "approved"), f(3, "minor", "dismissed"), f(4, "minor", "rejected"), f(5, "nit", "draft")]);
    expect(plan.approve).toEqual([1, 5]);
    expect(plan.post).toEqual([1, 5, 2]);
  });
  it("counts pending findings by severity", () => {
    const list = [f(1, "blocker", "draft"), f(2, "blocker", "approved"), f(3, "major", "draft"), f(4, "minor", "posted")];
    expect(severityCounts(pendingFindings(list))).toEqual([
      { severity: "blocker", count: 2 },
      { severity: "major", count: 1 },
    ]);
  });
  it("groups by severity with dismissed last", () => {
    const g = findingsBySeverity([f(1, "major", "dismissed", 0.99), f(2, "major", "draft", 0.5), f(3, "blocker", "draft")]);
    expect(g.map((x) => x.severity)).toEqual(["blocker", "major"]);
    expect(g[1].findings.map((x) => x.id)).toEqual([2, 1]);
  });
});

describe("display helpers", () => {
  it("ciDot", () => {
    expect(ciDot({ passed: 0, failed: 0, running: 0, total: 0 }).tone).toBe("none");
    expect(ciDot({ passed: 2, failed: 1, running: 0, total: 3 }).tone).toBe("red");
    expect(ciDot({ passed: 2, failed: 0, running: 1, total: 3 }).tone).toBe("blue");
    expect(ciDot({ passed: 3, failed: 0, running: 0, total: 3 }).tone).toBe("green");
    expect(ciDot({ passed: 2, failed: 0, running: 0, total: 3 }).tone).toBe("orange");
  });
  it("verdictMeta", () => {
    expect(verdictMeta("request_changes")?.label).toBe("Request changes");
    expect(verdictMeta("approve")?.tone).toBe("green");
    expect(verdictMeta(null)).toBeNull();
  });
  it("shortRelative", () => {
    const now = Date.parse("2026-11-25T12:00:00Z");
    expect(shortRelative("2026-11-25T10:00:00Z", now)).toBe("2h");
    expect(shortRelative("2026-11-25T11:59:50Z", now)).toBe("now");
    expect(shortRelative("2026-11-24T12:00:00Z", now)).toBe("1d");
  });
  it("defaultBaseBranch skips globs", () => {
    expect(defaultBaseBranch(["release/*", "develop"])).toBe("develop");
    expect(defaultBaseBranch([])).toBe("main");
    expect(defaultBaseBranch(undefined)).toBe("main");
  });
  it("reviewActionLabel", () => {
    expect(reviewActionLabel(row(1))).toBe("Start review");
    expect(reviewActionLabel(row(1, { latest_run: run("skipped") }))).toBe("Review anyway");
    expect(reviewActionLabel(row(1, { latest_run: run("failed") }))).toBe("Retry");
    expect(reviewActionLabel(row(1, { latest_run: run("posted") }))).toBe("Re-run");
  });
});
