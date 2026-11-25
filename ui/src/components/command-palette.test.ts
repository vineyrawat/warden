import { describe, expect, it } from "vitest";

import { filterCommands, fuzzyScore, mrPriority, type Command } from "./command-palette";

const cmd = (id: string, group: Command["group"], label: string, extra: Partial<Command> = {}): Command => ({ id, group, label, run: () => {}, ...extra });

const commands: Command[] = [
  cmd("mr-41", "mrs", "Import review: row-by-row importer", { prefix: "!41", keywords: "entries_crm rahul feat/import-review develop" }),
  cmd("mr-412", "mrs", "Bump deps", { prefix: "!412", keywords: "entries_platform vinay" }),
  cmd("mr-43", "mrs", "Acme: sales workflow stages", { prefix: "!43", keywords: "entries_crm ananya" }),
  cmd("post-41", "mrs", "Post approved notes to !41", { prefix: "!41", needsQuery: true, boost: -10 }),
  cmd("review-41", "actions", "Re-run review on !41", { needsQuery: true, keywords: "Import review entries_crm" }),
  cmd("pause", "actions", "Pause posting everywhere"),
  cmd("poll", "actions", "Poll now", { keywords: "refresh fetch merge requests" }),
  cmd("pane-general", "settings", "Settings: General", { keywords: "setup health checklist" }),
  cmd("guardrails-1", "settings", "Open entries_crm guardrails", { needsQuery: true }),
  cmd("go-inbox", "goto", "Inbox", { needsQuery: true }),
];

const ids = (q: string) => filterCommands(commands, q).flatMap((g) => g.items.map((c) => c.id));

describe("fuzzyScore", () => {
  it("matches substrings, word starts and subsequences", () => {
    expect(fuzzyScore("pause", "Pause posting everywhere")).not.toBeNull();
    expect(fuzzyScore("ppe", "Pause posting everywhere")).not.toBeNull();
    expect(fuzzyScore("xyz", "Pause posting everywhere")).toBeNull();
  });

  it("prefers a whole-word match over a prefix of a longer token", () => {
    expect(fuzzyScore("41", "!41 Import review")!).toBeGreaterThan(fuzzyScore("41", "!412 Bump deps")!);
  });

  it("weights keywords below visible text", () => {
    expect(fuzzyScore("crm", "entries_crm guardrails")!).toBeGreaterThan(fuzzyScore("crm", "Import review", "entries_crm")!);
  });

  it("requires every token to match", () => {
    expect(fuzzyScore("rerun 41", "Re-run review on !41")).not.toBeNull();
    expect(fuzzyScore("re-run 41", "Re-run review on !41")!).toBeGreaterThan(fuzzyScore("rerun 41", "Re-run review on !41")!);
    expect(fuzzyScore("review 99", "Re-run review on !41")).toBeNull();
  });

  it("is case-insensitive and ignores surrounding space", () => {
    expect(fuzzyScore("  ACME ", "Acme: sales workflow stages")).not.toBeNull();
  });
});

describe("filterCommands", () => {
  it("hides per-MR actions until there is a query", () => {
    const empty = ids("");
    expect(empty).toContain("mr-41");
    expect(empty).toContain("pause");
    expect(empty).not.toContain("post-41");
    expect(empty).not.toContain("review-41");
    expect(empty).not.toContain("go-inbox");
  });

  it("keeps the fixed group order and drops empty groups", () => {
    const groups = filterCommands(commands, "41").map((g) => g.id);
    expect(groups).toEqual(["mrs", "actions"]);
  });

  it("ranks the exact MR number first within its group", () => {
    const mrs = filterCommands(commands, "41")[0].items.map((c) => c.id);
    expect(mrs[0]).toBe("mr-41");
    expect(mrs.indexOf("mr-412")).toBeGreaterThan(mrs.indexOf("post-41"));
  });

  it("finds commands by keywords", () => {
    expect(ids("ananya")).toEqual(["mr-43"]);
    expect(ids("health")).toEqual(["pane-general"]);
  });

  it("caps each group", () => {
    const many = Array.from({ length: 20 }, (_, i) => cmd(`m${i}`, "mrs", `Change ${i}`));
    expect(filterCommands(many, "")[0].items).toHaveLength(6);
    expect(filterCommands(many, "change")[0].items).toHaveLength(8);
  });

  it("returns nothing for a query that matches nothing", () => {
    expect(filterCommands(commands, "zzzz")).toEqual([]);
  });
});

describe("mrPriority", () => {
  it("puts MRs waiting for approval first, then running reviews", () => {
    const run = (status: string) => ({ latest_run: { status } }) as never;
    expect(mrPriority(run("awaiting_approval"))).toBe(0);
    expect(mrPriority(run("reviewing"))).toBe(1);
    expect(mrPriority(run("posted"))).toBe(2);
    expect(mrPriority({ latest_run: null })).toBe(2);
  });
});
