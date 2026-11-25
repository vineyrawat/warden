import { describe, expect, it } from "vitest";
import type { Finding } from "@/bindings/Finding";
import type { PendingFinding } from "@/bindings/PendingFinding";
import {
  actionTargets,
  approvedSet,
  cursorAfterRemoval,
  filterPending,
  forgeName,
  groupChecked,
  groupPending,
  labelReports,
  moveCursor,
  nextUndecided,
  postAllLabel,
  postConfirmText,
  postSet,
  projectNames,
  pruneSelection,
  queueKeyAction,
  queueMeta,
  rangeIds,
  reportLine,
  selectRange,
  settleCursor,
  toggleAll,
  toggleGroup,
  toggleId,
  visibleIds,
} from "./queue-logic";

let nextId = 1;
function pf(over: Partial<Finding> & { mr?: number; iid?: number; project?: string; run?: number; url?: string } = {}): PendingFinding {
  const { mr = 1, iid = 41, project = "entries_crm", run = 10, url = "https://gitlab.com/e/crm/-/merge_requests/41", ...f } = over;
  const id = f.id ?? nextId++;
  return {
    finding: {
      id,
      run_id: run,
      file: "importer.py",
      line: 17,
      side: "new",
      severity: "major",
      category: "bug",
      rule_id: null,
      title: `Finding ${id}`,
      body: "Body",
      suggestion: null,
      evidence: null,
      confidence: 0.8,
      source: "claude",
      status: "draft",
      guardrail_reason: null,
      fingerprint: `fp${id}`,
      forge_note_id: null,
      position: null,
      ...f,
    },
    run_id: run,
    mr_id: mr,
    mr_iid: iid,
    mr_title: `MR ${iid}`,
    project_name: project,
    web_url: url,
  };
}

const rows = [
  pf({ id: 1, severity: "major", line: 18 }),
  pf({ id: 2, severity: "blocker", confidence: 0.9, status: "approved" }),
  pf({ id: 3, severity: "blocker", confidence: 0.95, title: "SQL built with an f-string", rule_id: "FR-002" }),
  pf({ id: 4, mr: 2, iid: 9, project: "entries_lnd", run: 20, severity: "minor", file: "api.py", line: 52, status: "approved" }),
  pf({ id: 5, mr: 2, iid: 9, project: "entries_lnd", run: 20, severity: "minor", file: "quiz_attempt.json", line: 0 }),
];
const byId = new Map(rows.map((r) => [r.finding.id, r]));

describe("filterPending", () => {
  it("filters by project, severity and search terms", () => {
    expect(filterPending(rows, { project: "entries_lnd", severity: null, search: "" }).map((r) => r.finding.id)).toEqual([4, 5]);
    expect(filterPending(rows, { project: null, severity: "blocker", search: "" }).map((r) => r.finding.id)).toEqual([2, 3]);
    expect(filterPending(rows, { project: null, severity: null, search: "f-string fr-002" }).map((r) => r.finding.id)).toEqual([3]);
    expect(filterPending(rows, { project: null, severity: null, search: "!9 api.py:52" }).map((r) => r.finding.id)).toEqual([4]);
    expect(filterPending(rows, { project: null, severity: null, search: "nothing-like-this" })).toEqual([]);
  });

  it("lists project names", () => {
    expect(projectNames(rows)).toEqual(["entries_crm", "entries_lnd"]);
  });
});

describe("groupPending", () => {
  it("groups by MR in backend order, most severe first", () => {
    const g = groupPending(rows, "mr");
    expect(g.map((x) => x.key)).toEqual(["mr:1", "mr:2"]);
    expect(g[0].label).toBe("!41 MR 41");
    expect(g[0].mr).toMatchObject({ iid: 41, project: "entries_crm", runId: 10 });
    expect(g[0].rows.map((r) => r.finding.id)).toEqual([3, 2, 1]);
    expect(g[1].rows.map((r) => r.finding.id)).toEqual([4, 5]);
  });

  it("groups by severity, skipping empty ones", () => {
    const g = groupPending(rows, "severity");
    expect(g.map((x) => x.label)).toEqual(["Blocker", "Major", "Minor"]);
    expect(g[0].rows.map((r) => r.finding.id)).toEqual([3, 2]);
  });

  it("lists visible ids, skipping collapsed groups", () => {
    const g = groupPending(rows, "mr");
    expect(visibleIds(g, new Set())).toEqual([3, 2, 1, 4, 5]);
    expect(visibleIds(g, new Set(["mr:1"]))).toEqual([4, 5]);
  });
});

describe("selection", () => {
  const order = [3, 2, 1, 4, 5];

  it("toggles one id", () => {
    expect([...toggleId(new Set([1]), 2)]).toEqual([1, 2]);
    expect([...toggleId(new Set([1, 2]), 1)]).toEqual([2]);
  });

  it("computes ranges in display order, either direction", () => {
    expect(rangeIds(order, 2, 4)).toEqual([2, 1, 4]);
    expect(rangeIds(order, 4, 2)).toEqual([2, 1, 4]);
    expect(rangeIds(order, null, 1)).toEqual([1]);
    expect(rangeIds(order, 99, 1)).toEqual([1]);
    expect(rangeIds(order, 1, 99)).toEqual([]);
  });

  it("adds a range to the selection", () => {
    expect([...selectRange(new Set([5]), order, 3, 1)].sort()).toEqual([1, 2, 3, 5]);
  });

  it("selects all visible, then clears them", () => {
    const all = toggleAll(new Set([2]), [3, 2, 1]);
    expect([...all].sort()).toEqual([1, 2, 3]);
    expect([...toggleAll(new Set([...all, 9]), [3, 2, 1])]).toEqual([9]);
    expect([...toggleAll(new Set(), [])]).toEqual([]);
  });

  it("handles group checkboxes", () => {
    expect(groupChecked(new Set(), [1, 2])).toBe(false);
    expect(groupChecked(new Set([1]), [1, 2])).toBe("indeterminate");
    expect(groupChecked(new Set([1, 2]), [1, 2])).toBe(true);
    expect([...toggleGroup(new Set([1]), [1, 2])].sort()).toEqual([1, 2]);
    expect([...toggleGroup(new Set([1, 2, 5]), [1, 2])]).toEqual([5]);
  });

  it("prunes ids that no longer exist", () => {
    expect([...pruneSelection(new Set([1, 7]), [1, 2])]).toEqual([1]);
  });

  it("picks targets: the selection in display order, else the cursor", () => {
    expect(actionTargets(new Set([4, 3]), 1, order)).toEqual([3, 4]);
    expect(actionTargets(new Set(), 1, order)).toEqual([1]);
    expect(actionTargets(new Set(), null, order)).toEqual([]);
  });
});

describe("cursor", () => {
  const order = [3, 2, 1, 4, 5];

  it("moves and clamps", () => {
    expect(moveCursor(order, 3, 1)).toBe(2);
    expect(moveCursor(order, 3, -1)).toBe(3);
    expect(moveCursor(order, 5, 1)).toBe(5);
    expect(moveCursor(order, null, 1)).toBe(3);
    expect(moveCursor([], 1, 1)).toBeNull();
  });

  it("settles on the row now at the old index when the cursor row leaves", () => {
    expect(settleCursor(order, 1, 2)).toBe(1);
    expect(settleCursor([3, 2, 4, 5], 1, 2)).toBe(4);
    expect(settleCursor([3, 2], 1, 4)).toBe(2);
    expect(settleCursor([], 1, 0)).toBeNull();
  });

  it("moves past removed rows", () => {
    expect(cursorAfterRemoval(order, 2, [2, 1])).toBe(4);
    expect(cursorAfterRemoval(order, 5, [5])).toBe(4);
    expect(cursorAfterRemoval(order, 3, [1])).toBe(3);
    expect(cursorAfterRemoval(order, 3, order)).toBeNull();
  });

  it("finds the next undecided row, wrapping", () => {
    expect(nextUndecided(order, byId, 3)).toBe(1);
    expect(nextUndecided(order, byId, 5)).toBe(3);
    expect(nextUndecided(order, byId, 3, new Set([1, 5]))).toBeNull();
  });
});

describe("posting copy", () => {
  it("summarises approved findings", () => {
    const s = approvedSet(rows);
    expect(s).toEqual({ ids: [2, 4], mrs: 2 });
    expect(postAllLabel(s)).toBe("Post all approved (2 across 2 MRs)");
    expect(postAllLabel({ ids: [1], mrs: 1 })).toBe("Post all approved (1 across 1 MR)");
  });

  it("summarises an arbitrary set", () => {
    expect(postSet(rows, [1, 3, 99])).toEqual({ ids: [1, 3], mrs: 1 });
  });

  it("writes the confirm question", () => {
    expect(postConfirmText(7, 3)).toBe("Post 7 notes to 3 merge requests on GitLab?");
    expect(postConfirmText(1, 1, "GitHub")).toBe("Post 1 note to 1 merge request on GitHub?");
  });

  it("names the forge from MR URLs", () => {
    expect(forgeName([{ web_url: "https://gitlab.com/x" }])).toBe("GitLab");
    expect(forgeName([{ web_url: "https://github.com/x" }])).toBe("GitHub");
    expect(forgeName([{ web_url: "https://github.com/x" }, { web_url: "https://git.corp/x" }])).toBe("GitLab and GitHub");
    expect(forgeName([])).toBe("GitLab");
  });

  it("writes the header meta", () => {
    expect(queueMeta(rows)).toBe("5 findings · 2 merge requests");
    expect(queueMeta(rows.slice(0, 1))).toBe("1 finding · 1 merge request");
  });

  it("pairs reports with MRs in run id order", () => {
    const reports = [
      { posted: 2, fallback: 0, failed: 0, skipped: 0 },
      { posted: 1, fallback: 1, failed: 1, skipped: 0 },
    ];
    const out = labelReports([4, 2, 3], rows, reports);
    expect(out.map((r) => [r.runId, r.label])).toEqual([
      [10, "!41 MR 41"],
      [20, "!9 MR 9"],
    ]);
    expect(reportLine(out[0].report)).toBe("2 posted");
    expect(reportLine(out[1].report)).toBe("1 posted · 1 as file note · 1 failed");
  });
});

describe("queueKeyAction", () => {
  it("maps keys", () => {
    expect(queueKeyAction({ key: "ArrowDown" })).toBe("down");
    expect(queueKeyAction({ key: "ArrowUp" })).toBe("up");
    expect(queueKeyAction({ key: " " })).toBe("toggle");
    expect(queueKeyAction({ key: "x" })).toBe("toggle");
    expect(queueKeyAction({ key: "a" })).toBe("approve");
    expect(queueKeyAction({ key: "D" })).toBe("dismiss");
    expect(queueKeyAction({ key: "e" })).toBe("edit");
    expect(queueKeyAction({ key: "p" })).toBe("preview");
    expect(queueKeyAction({ key: "a", metaKey: true })).toBe("selectAll");
    expect(queueKeyAction({ key: "a", ctrlKey: true })).toBe("selectAll");
    expect(queueKeyAction({ key: "Escape" })).toBe("escape");
  });

  it("ignores fields, other modified keys and Space on controls", () => {
    expect(queueKeyAction({ key: "a", editable: true })).toBeNull();
    expect(queueKeyAction({ key: "d", metaKey: true })).toBeNull();
    expect(queueKeyAction({ key: "a", altKey: true })).toBeNull();
    expect(queueKeyAction({ key: " ", onControl: true })).toBeNull();
    expect(queueKeyAction({ key: "x", onControl: true })).toBe("toggle");
    expect(queueKeyAction({ key: "q" })).toBeNull();
  });
});
