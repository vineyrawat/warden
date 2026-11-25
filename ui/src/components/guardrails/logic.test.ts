import { describe, expect, it } from "vitest";

import type { Finding } from "@/bindings/Finding";
import type { RuleConfig } from "@/bindings/RuleConfig";
import type { SimulatedFinding } from "@/bindings/SimulatedFinding";
import { defaultConfig } from "@/lib/mock-data";
import type { MergedConfig } from "@/lib/api";
import {
  changeText,
  commonDir,
  diffDraft,
  dismissedRate,
  draftFrom,
  filterRules,
  mrRef,
  outcomeIndex,
  overridesFromDraft,
  reasonTag,
  ruleAdvice,
  ruleSource,
  sortSimRows,
  summarizeSimulation,
  unmatchedStats,
} from "./logic";

const cfg = (): MergedConfig => ({ ...defaultConfig(), disabled_rules: ["FR-004"] });

const rule = (p: Partial<RuleConfig>): RuleConfig => ({ id: "X-1", title: "t", applies_to: null, instruction: null, pattern: null, semgrep: null, severity: "major", ...p });

function sim(id: number, outcome: string, reason = "", p: Partial<Finding> = {}): SimulatedFinding {
  const finding = { id, run_id: 1, file: "a.py", line: 1, side: "new", severity: "major", category: "c", rule_id: null, title: `f${id}`, body: "", suggestion: null, evidence: null, confidence: 0.8, source: "claude", status: "draft", guardrail_reason: null, fingerprint: "", forge_note_id: null, position: null, ...p } as Finding;
  return { finding, mr_label: "!41 Add billing export", outcome, reason };
}

describe("draft and overrides", () => {
  it("seeds the draft from the merged config, project mode wins", () => {
    const d = draftFrom(cfg(), "assist");
    expect(d.mode).toBe("assist");
    expect(d.minConfidence).toBe(0.7);
    expect(d.autoSeverities).toEqual(["blocker", "major"]);
    expect(d.disabled).toEqual(["FR-004"]);
    expect(draftFrom(cfg()).mode).toBe("draft");
  });

  it("builds overrides with every edited field, rounded and sorted", () => {
    const c = cfg();
    const d = { ...draftFrom(c), minConfidence: 0.7500001, autoSeverities: ["minor", "blocker"] as const, disabled: ["SEC-001", "FR-007"] };
    const o = overridesFromDraft({ ...d, autoSeverities: [...d.autoSeverities] }, c);
    expect(o.review).toMatchObject({ min_confidence: 0.75, severity_floor: "minor", max_notes: 10, nits: false, ignore_paths: c.review.ignore_paths });
    expect(o.posting).toMatchObject({ mode: "draft", summary_note: true, max_notes_per_hour: 30, auto_post: { severities: ["blocker", "minor"], min_confidence: 0.85 } });
    expect(o.disabled_rules).toEqual(["FR-007", "SEC-001"]);
  });

  it("lists changes between saved and draft", () => {
    const saved = draftFrom(cfg());
    expect(diffDraft(saved, saved)).toEqual([]);
    const changes = diffDraft(saved, { ...saved, minConfidence: 0.8, nits: true, disabled: ["FR-007"], ignorePaths: [...saved.ignorePaths, "docs/**"] });
    expect(changes.map(changeText)).toEqual([
      "Minimum confidence 0.70 → 0.80",
      "Nits off → on",
      "Ignored paths yarn.lock, **/*.min.js, **/dist/** → yarn.lock, **/*.min.js, **/dist/**, docs/**",
      "FR-007 off",
      "FR-004 on",
    ]);
  });

  it("ignores float noise and severity order", () => {
    const saved = draftFrom(cfg());
    expect(diffDraft(saved, { ...saved, minConfidence: 0.70000001, autoSeverities: ["major", "blocker"] })).toEqual([]);
  });
});

describe("rules catalogue", () => {
  it("tells built-in rules from .warden.yml ones", () => {
    expect(ruleSource(rule({ id: "FR-007", pattern: "x" }))).toBe("builtin");
    expect(ruleSource(rule({ id: "FR-007", semgrep: "rules/fr-007.yml" }))).toBe("repo");
    expect(ruleSource(rule({ id: "UI-002", instruction: "x" }))).toBe("repo");
  });

  it("filters by kind, source and search", () => {
    const rules = [rule({ id: "FR-001", title: "Permissions", instruction: "check" }), rule({ id: "FR-007", title: "Commit", pattern: "commit" }), rule({ id: "UI-002", title: "Use @entries/ui", instruction: "ui", applies_to: "frontend/**" })];
    expect(filterRules(rules, { kind: "regex", source: "all", query: "" }).map((r) => r.id)).toEqual(["FR-007"]);
    expect(filterRules(rules, { kind: "all", source: "repo", query: "" }).map((r) => r.id)).toEqual(["UI-002"]);
    expect(filterRules(rules, { kind: "all", source: "all", query: "FRONTEND" }).map((r) => r.id)).toEqual(["UI-002"]);
    expect(filterRules(rules, { kind: "claude", source: "builtin", query: "perm" }).map((r) => r.id)).toEqual(["FR-001"]);
  });

  it("computes dismissed rate only with enough hits", () => {
    expect(dismissedRate({ hits: 11, dismissed: 6 })).toBeCloseTo(0.545);
    expect(dismissedRate({ hits: 2, dismissed: 2 })).toBeNull();
    expect(dismissedRate(undefined)).toBeNull();
  });

  it("finds the common directory", () => {
    expect(commonDir(["entries_crm/api/leads.py", "entries_crm/api/deals.py"])).toBe("entries_crm/api/");
    expect(commonDir(["entries_crm/api/leads.py", "entries_desk/x.py"])).toBe("");
    expect(commonDir(["setup.py"])).toBe("");
    expect(commonDir([])).toBe("");
  });

  it("advises on noisy rules", () => {
    const r = rule({ id: "FR-007", severity: "major", applies_to: "**/*.py" });
    expect(ruleAdvice(r, { hits: 11, dismissed: 6 }, ["entries_crm/api/leads.py", "entries_crm/api/deals.py"])).toBe(
      "FR-007 notes are dismissed 55% of the time. Consider lowering it to minor or limiting it to api/.",
    );
    expect(ruleAdvice(r, { hits: 11, dismissed: 6 })).toBe("FR-007 notes are dismissed 55% of the time. Consider lowering it to minor.");
    expect(ruleAdvice({ ...r, severity: "nit" }, { hits: 4, dismissed: 4 })).toBe("FR-007 notes are dismissed 100% of the time. Consider turning it off.");
    expect(ruleAdvice(r, { hits: 10, dismissed: 4 })).toBeNull();
    expect(ruleAdvice({ ...r, applies_to: "entries_crm/api/**" }, { hits: 4, dismissed: 2 }, ["entries_crm/api/a.py"])).toBe(
      "FR-007 notes are dismissed 50% of the time. Consider lowering it to minor.",
    );
  });

  it("returns stats rows that are not configured rules", () => {
    const stats = [{ rule_id: "FR-007", title: "", hits: 1, posted: 0, dismissed: 0, rejected: 0, last_hit_at: null }, { rule_id: "correctness", title: "", hits: 2, posted: 1, dismissed: 0, rejected: 0, last_hit_at: null }];
    expect(unmatchedStats(stats, [{ id: "FR-007" }]).map((s) => s.rule_id)).toEqual(["correctness"]);
  });
});

describe("live preview", () => {
  it("summarises outcomes, runs and drop reasons", () => {
    const rows = [
      sim(1, "post"),
      sim(2, "draft"),
      sim(3, "drop", "confidence 0.62 is below 0.75", { run_id: 2 }),
      sim(4, "drop", "minor is below the floor (major)"),
      sim(5, "drop", "confidence 0.5 is below 0.75"),
    ];
    const s = summarizeSimulation(rows);
    expect(s).toMatchObject({ post: 1, draft: 1, drop: 3, runs: 2 });
    expect(s.dropReasons).toEqual([
      { tag: "confidence", count: 2 },
      { tag: "severity", count: 1 },
    ]);
  });

  it("tags drop reasons", () => {
    expect(reasonTag("nits are disabled")).toBe("nits");
    expect(reasonTag("over the note budget (5)")).toBe("budget");
    expect(reasonTag("rule FR-007 is disabled")).toBe("rule off");
    expect(reasonTag("duplicate of an open thread")).toBe("duplicate");
    expect(reasonTag("something else")).toBe("guardrail");
    expect(reasonTag("severity minor is below the floor (major)")).toBe("severity");
    expect(reasonTag("confidence 0.62 is below the threshold 0.70")).toBe("confidence");
    expect(reasonTag("over note budget (max 5 per run)")).toBe("budget");
    expect(reasonTag("nits are disabled for this project")).toBe("nits");
    expect(reasonTag("entries_crm/unit_confidence.py is in ignore_paths")).toBe("ignored path");
  });

  it("indexes outcomes, extracts MR refs, sorts rows", () => {
    expect(outcomeIndex([sim(7, "post")]).get(7)).toBe("post");
    expect(mrRef("!41 Add billing export")).toBe("!41");
    const sorted = sortSimRows([sim(1, "drop"), sim(2, "post", "", { severity: "minor" }), sim(3, "post", "", { severity: "blocker" }), sim(4, "draft")]);
    expect(sorted.map((r) => r.finding.id)).toEqual([3, 2, 4, 1]);
  });
});

describe("patchOverrides", () => {
  it("writes only changed fields on top of stored overrides", async () => {
    const { patchOverrides, draftFrom, diffDraft } = await import("./logic");
    const cfg = {
      review: { min_confidence: 0.7, severity_floor: "minor", max_notes: 10, nits: false, focus: ["security"], ignore_paths: [] },
      posting: { mode: "draft", summary_note: true, max_notes_per_hour: 30, auto_post: { severities: ["blocker"], min_confidence: 0.9 } },
      rules: [],
      disabled_rules: [],
    } as never;
    const saved = draftFrom(cfg);
    const draft = { ...saved, minConfidence: 0.8 };
    const out = patchOverrides({ posting: { summary_note: false } }, draft, diffDraft(saved, draft), cfg) as Record<string, any>;
    expect(out).toEqual({ posting: { summary_note: false }, review: { min_confidence: 0.8 } });
  });
});
