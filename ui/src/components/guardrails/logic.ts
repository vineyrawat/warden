// Pure helpers for the Guardrails panel: draft <-> overrides, change list, rule catalogue
// (source, filtering, dismissed-rate advice) and live-preview summaries. Unit tested in logic.test.ts.
import type { MrStatusMode } from "@/bindings/MrStatusMode";
import type { PostingMode } from "@/bindings/PostingMode";
import type { RuleConfig } from "@/bindings/RuleConfig";
import type { RuleKind } from "@/bindings/RuleKind";
import type { RuleStat } from "@/bindings/RuleStat";
import type { Severity } from "@/bindings/Severity";
import type { SimulatedFinding } from "@/bindings/SimulatedFinding";
import type { MergedConfig, ProjectOverrides } from "@/lib/api";
import { SEVERITIES, capitalize, ruleKind, severityRank } from "@/lib/format";

/** Everything the panel edits for one project. */
export type Draft = {
  mode: PostingMode;
  autoSeverities: Severity[];
  autoPostConfidence: number;
  maxNotes: number;
  maxNotesPerHour: number;
  summaryNote: boolean;
  minConfidence: number;
  severityFloor: Severity;
  nits: boolean;
  focus: string[];
  ignorePaths: string[];
  disabled: string[];
  /** Free text appended to every review prompt (`review.extra_instructions`). */
  extraInstructions: string;
  /** `posting.mr_status`: what happens to the MR's status after posting. */
  mrStatusMode: MrStatusMode;
  requestChangesOn: Severity[];
};

export function draftFrom(cfg: MergedConfig, projectMode?: PostingMode | null): Draft {
  return {
    mode: projectMode ?? cfg.posting.mode,
    autoSeverities: sortSeverities(cfg.posting.auto_post.severities),
    autoPostConfidence: cfg.posting.auto_post.min_confidence,
    maxNotes: cfg.review.max_notes,
    maxNotesPerHour: cfg.posting.max_notes_per_hour,
    summaryNote: cfg.posting.summary_note,
    minConfidence: cfg.review.min_confidence,
    severityFloor: cfg.review.severity_floor,
    nits: cfg.review.nits,
    focus: [...cfg.review.focus],
    ignorePaths: [...cfg.review.ignore_paths],
    disabled: [...(cfg.disabled_rules ?? [])].sort(),
    extraInstructions: cfg.review.extra_instructions ?? "",
    mrStatusMode: cfg.posting.mr_status?.mode ?? "off",
    requestChangesOn: sortSeverities(cfg.posting.mr_status?.request_changes_on ?? ["blocker", "major"]),
  };
}

export const round2 = (n: number) => Math.round(n * 100) / 100;

/** Most severe first, deduplicated. */
export function sortSeverities(list: Severity[]): Severity[] {
  return [...new Set(list)].sort((a, b) => severityRank(b) - severityRank(a));
}

/**
 * Overrides written for the project: every field this panel edits (merged over `.warden.yml` by the
 * backend). Saving replaces the stored overrides, so all edited fields are always included.
 */
export function overridesFromDraft(d: Draft, cfg: MergedConfig): ProjectOverrides {
  return {
    review: {
      min_confidence: round2(d.minConfidence),
      severity_floor: d.severityFloor,
      max_notes: d.maxNotes,
      nits: d.nits,
      focus: [...d.focus],
      ignore_paths: [...d.ignorePaths],
      extra_instructions: d.extraInstructions,
    },
    posting: {
      mode: d.mode,
      summary_note: d.summaryNote,
      max_notes_per_hour: d.maxNotesPerHour,
      auto_post: { ...cfg.posting.auto_post, severities: sortSeverities(d.autoSeverities), min_confidence: round2(d.autoPostConfidence) },
      mr_status: { mode: d.mrStatusMode, request_changes_on: sortSeverities(d.requestChangesOn) },
    },
    disabled_rules: [...d.disabled].sort(),
  };
}

export type Change = { key: string; label: string; from: string; to: string };

export const MR_STATUS_LABEL: Record<MrStatusMode, string> = { off: "Off", labels: "Labels", review: "Approve / request changes" };

/**
 * Stored overrides with ONLY the changed fields written on top. Fields you did not touch stay out of
 * the overrides, so later edits to them in the repo's `.warden.yml` still apply.
 */
export function patchOverrides(stored: ProjectOverrides | null | undefined, d: Draft, changes: Change[], cfg: MergedConfig): ProjectOverrides {
  const out = JSON.parse(JSON.stringify(stored ?? {})) as Record<string, unknown>;
  const section = (k: "review" | "posting") => ((out[k] ??= {}) as Record<string, unknown>);
  const keys = new Set(changes.map((c) => (c.key.startsWith("rule:") ? "rules" : c.key)));
  const full = overridesFromDraft(d, cfg);
  const fr = full.review as Record<string, unknown>;
  const fp = full.posting as Record<string, unknown>;
  const map: Record<string, () => void> = {
    mode: () => (section("posting").mode = fp.mode),
    summaryNote: () => (section("posting").summary_note = fp.summary_note),
    maxNotesPerHour: () => (section("posting").max_notes_per_hour = fp.max_notes_per_hour),
    autoSeverities: () => (section("posting").auto_post = { ...((section("posting").auto_post as object) ?? {}), severities: (fp.auto_post as Record<string, unknown>).severities }),
    autoPostConfidence: () => (section("posting").auto_post = { ...((section("posting").auto_post as object) ?? {}), min_confidence: (fp.auto_post as Record<string, unknown>).min_confidence }),
    maxNotes: () => (section("review").max_notes = fr.max_notes),
    minConfidence: () => (section("review").min_confidence = fr.min_confidence),
    severityFloor: () => (section("review").severity_floor = fr.severity_floor),
    nits: () => (section("review").nits = fr.nits),
    focus: () => (section("review").focus = fr.focus),
    ignorePaths: () => (section("review").ignore_paths = fr.ignore_paths),
    rules: () => (out.disabled_rules = full.disabled_rules),
    extraInstructions: () => (section("review").extra_instructions = fr.extra_instructions),
    mrStatusMode: () => (section("posting").mr_status = { ...((section("posting").mr_status as object) ?? {}), mode: (fp.mr_status as Record<string, unknown>).mode }),
    requestChangesOn: () =>
      (section("posting").mr_status = { ...((section("posting").mr_status as object) ?? {}), request_changes_on: (fp.mr_status as Record<string, unknown>).request_changes_on }),
  };
  keys.forEach((k) => map[k]?.());
  return out as ProjectOverrides;
}

const fmtConf = (n: number) => round2(n).toFixed(2);
const fmtList = (l: string[]) => (l.length ? l.join(", ") : "none");
const onOff = (b: boolean) => (b ? "on" : "off");
const sameList = (a: string[], b: string[]) => a.length === b.length && a.every((x, i) => x === b[i]);

/** Human-readable list of what changed between the saved state and the draft (empty = clean). */
export function diffDraft(saved: Draft, draft: Draft): Change[] {
  const out: Change[] = [];
  const add = (key: string, label: string, from: string, to: string) => {
    if (from !== to) out.push({ key, label, from, to });
  };
  add("mode", "Posting mode", capitalize(saved.mode), capitalize(draft.mode));
  add("autoSeverities", "Auto-post severities", fmtList(sortSeverities(saved.autoSeverities)), fmtList(sortSeverities(draft.autoSeverities)));
  add("autoPostConfidence", "Auto-post confidence", fmtConf(saved.autoPostConfidence), fmtConf(draft.autoPostConfidence));
  add("maxNotes", "Notes per MR", String(saved.maxNotes), String(draft.maxNotes));
  add("maxNotesPerHour", "Notes per hour", String(saved.maxNotesPerHour), String(draft.maxNotesPerHour));
  add("summaryNote", "Summary note", onOff(saved.summaryNote), onOff(draft.summaryNote));
  add("minConfidence", "Minimum confidence", fmtConf(saved.minConfidence), fmtConf(draft.minConfidence));
  add("severityFloor", "Lowest severity", saved.severityFloor, draft.severityFloor);
  add("nits", "Nits", onOff(saved.nits), onOff(draft.nits));
  if (!sameList(saved.focus, draft.focus)) out.push({ key: "focus", label: "Focus areas", from: fmtList(saved.focus), to: fmtList(draft.focus) });
  if (!sameList(saved.ignorePaths, draft.ignorePaths))
    out.push({ key: "ignorePaths", label: "Ignored paths", from: fmtList(saved.ignorePaths), to: fmtList(draft.ignorePaths) });
  add("mrStatusMode", "MR status after posting", MR_STATUS_LABEL[saved.mrStatusMode], MR_STATUS_LABEL[draft.mrStatusMode]);
  add("requestChangesOn", "Request changes on", fmtList(sortSeverities(saved.requestChangesOn)), fmtList(sortSeverities(draft.requestChangesOn)));
  if (saved.extraInstructions.trim() !== draft.extraInstructions.trim())
    out.push({ key: "extraInstructions", label: "Review instructions", from: saved.extraInstructions.trim() ? "set" : "none", to: draft.extraInstructions.trim() ? "edited" : "none" });
  const was = new Set(saved.disabled);
  const now = new Set(draft.disabled);
  for (const id of [...now].filter((x) => !was.has(x)).sort()) out.push({ key: `rule:${id}`, label: id, from: "on", to: "off" });
  for (const id of [...was].filter((x) => !now.has(x)).sort()) out.push({ key: `rule:${id}`, label: id, from: "off", to: "on" });
  return out;
}

/** "Minimum confidence 0.70 → 0.80", "FR-007 off". */
export function changeText(c: Change): string {
  if (c.key.startsWith("rule:")) return `${c.label} ${c.to}`;
  return `${c.label} ${c.from} → ${c.to}`;
}

// ---- rules catalogue ---------------------------------------------------------------------------

/** Built-in rule ids and how each is enforced (warden-core `config::default_rules`). */
export const BUILTIN_RULES: Record<string, RuleKind> = {
  "FR-001": "claude",
  "FR-002": "regex",
  "FR-004": "claude",
  "FR-007": "regex",
  "SEC-001": "claude",
};

export type RuleSource = "repo" | "builtin";

/**
 * Where a rule comes from. A rule with a built-in id that is still enforced the built-in way is
 * "builtin"; anything else (new ids, or built-in ids redefined e.g. as semgrep) came from the
 * repo's `.warden.yml` on the base branch.
 */
export function ruleSource(r: Pick<RuleConfig, "id" | "instruction" | "pattern" | "semgrep">): RuleSource {
  const k = BUILTIN_RULES[r.id];
  return k && k === ruleKind(r) ? "builtin" : "repo";
}

export function ruleSourceLabel(s: RuleSource): string {
  return s === "builtin" ? "built-in" : ".warden.yml";
}

/** One-line description of what the rule checks. */
export function ruleCatches(r: Pick<RuleConfig, "instruction" | "pattern" | "semgrep">): string {
  if (r.instruction?.trim()) return r.instruction.trim();
  if (r.semgrep) return `Semgrep rule ${r.semgrep}`;
  if (r.pattern) return `Added lines matching /${r.pattern}/`;
  return "";
}

export type RuleFilter = { kind: RuleKind | "all"; source: RuleSource | "all"; query: string };

export function filterRules<T extends RuleConfig>(rules: T[], f: RuleFilter): T[] {
  const q = f.query.trim().toLowerCase();
  return rules.filter((r) => {
    if (f.kind !== "all" && ruleKind(r) !== f.kind) return false;
    if (f.source !== "all" && ruleSource(r) !== f.source) return false;
    if (!q) return true;
    return [r.id, r.title, r.instruction ?? "", r.pattern ?? "", r.semgrep ?? "", r.applies_to ?? ""].some((s) => s.toLowerCase().includes(q));
  });
}

/** Dismissed share of a rule's hits, or null when there is too little data to judge. */
export function dismissedRate(s: Pick<RuleStat, "hits" | "dismissed"> | undefined, minHits = 3): number | null {
  if (!s || s.hits < minHits) return null;
  return s.dismissed / s.hits;
}

export const NOISY_RATE = 0.5;

/** Longest directory prefix shared by every path ("entries_crm/api/"), or "" when none. */
export function commonDir(paths: string[]): string {
  if (!paths.length) return "";
  const dirs = paths.map((p) => {
    const i = p.lastIndexOf("/");
    return i >= 0 ? p.slice(0, i + 1).split("/").filter(Boolean) : [];
  });
  const first = dirs[0];
  let n = 0;
  while (n < first.length && dirs.every((d) => d[n] === first[n])) n++;
  return n ? `${first.slice(0, n).join("/")}/` : "";
}

/**
 * Advice for a noisy rule (dismissed ≥ 50%), e.g. "FR-007 notes are dismissed 55% of the time.
 * Consider lowering it to minor or limiting it to api/." `files` are paths of the rule's recent
 * findings; when they share a directory narrower than the rule's scope it is suggested.
 */
export function ruleAdvice(rule: Pick<RuleConfig, "id" | "severity" | "applies_to">, stat: Pick<RuleStat, "hits" | "dismissed"> | undefined, files: string[] = []): string | null {
  const rate = dismissedRate(stat);
  if (rate == null || rate < NOISY_RATE) return null;
  const pct = Math.round(rate * 100);
  const idx = SEVERITIES.indexOf(rule.severity);
  const lower = idx >= 0 && idx < SEVERITIES.length - 1 ? SEVERITIES[idx + 1] : null;
  const dir = shortDir(commonDir(files));
  const scope = rule.applies_to ?? "";
  const narrower = dir && !scope.startsWith(dir) && !scope.includes(`/${dir}`) ? dir : "";
  const options = [lower ? `lowering it to ${lower}` : "turning it off", narrower ? `limiting it to ${narrower}` : ""].filter(Boolean);
  return `${rule.id} notes are dismissed ${pct}% of the time. Consider ${options.join(" or ")}.`;
}

/** Last directory segment of a prefix: "entries_crm/api/" -> "api/". */
function shortDir(dir: string): string {
  const parts = dir.split("/").filter(Boolean);
  return parts.length ? `${parts[parts.length - 1]}/` : "";
}

/** RuleStat rows that are not configured rules (Claude categories, scanner ids). */
export function unmatchedStats(stats: RuleStat[], rules: Pick<RuleConfig, "id">[]): RuleStat[] {
  const ids = new Set(rules.map((r) => r.id));
  return stats.filter((s) => !ids.has(s.rule_id));
}

// ---- live preview ------------------------------------------------------------------------------

export type Outcome = "post" | "draft" | "drop";

export function asOutcome(s: string): Outcome {
  return s === "post" || s === "draft" ? s : "drop";
}

export type SimSummary = { post: number; draft: number; drop: number; runs: number; dropReasons: { tag: string; count: number }[] };

export function summarizeSimulation(rows: SimulatedFinding[]): SimSummary {
  const out: SimSummary = { post: 0, draft: 0, drop: 0, runs: new Set(rows.map((r) => r.finding.run_id)).size, dropReasons: [] };
  const tags = new Map<string, number>();
  for (const r of rows) {
    const o = asOutcome(r.outcome);
    out[o]++;
    if (o === "drop") {
      const t = reasonTag(r.reason);
      tags.set(t, (tags.get(t) ?? 0) + 1);
    }
  }
  out.dropReasons = [...tags.entries()].map(([tag, count]) => ({ tag, count })).sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag));
  return out;
}

/** Short pill text for a drop reason: "confidence", "severity", "nits", "budget", "rule off", ... */
export function reasonTag(reason: string): string {
  const r = reason.trim().toLowerCase();
  // Reasons that embed a file path or rule id are matched by their fixed wording first.
  if (/^rule \S+ is disabled/.test(r)) return "rule off";
  if (/is in ignore_paths$/.test(r)) return "ignored path";
  if (/^nits are disabled/.test(r)) return "nits";
  if (/^(severity )?\w+ is below the floor/.test(r)) return "severity";
  if (/^confidence [\d.]+ is below/.test(r)) return "confidence";
  if (/note budget/.test(r)) return "budget";
  if (/near-duplicate|duplicate/.test(r)) return "duplicate";
  if (/anchor|changed line|not in the diff|added line|deleted line/.test(r)) return "anchor";
  if (/tone|hedg/.test(r)) return "tone";
  if (/evidence|rule_id|rule id|grounding/.test(r)) return "no evidence";
  return "guardrail";
}

/** Outcome per finding id under the saved settings, to mark rows the draft would change. */
export function outcomeIndex(rows: SimulatedFinding[] | undefined): Map<number, Outcome> {
  return new Map((rows ?? []).map((r) => [r.finding.id, asOutcome(r.outcome)]));
}

/** "!41 Add billing export" -> "!41". */
export function mrRef(label: string): string {
  const m = /^(!\d+|#\d+)/.exec(label.trim());
  return m ? m[1] : label;
}

/** Severity counts by outcome order for the preview list: post, draft, then drop; severe first. */
export function sortSimRows(rows: SimulatedFinding[]): SimulatedFinding[] {
  const order: Record<Outcome, number> = { post: 0, draft: 1, drop: 2 };
  return [...rows].sort(
    (a, b) =>
      order[asOutcome(a.outcome)] - order[asOutcome(b.outcome)] ||
      severityRank(b.finding.severity) - severityRank(a.finding.severity) ||
      b.finding.confidence - a.finding.confidence ||
      a.finding.id - b.finding.id,
  );
}

// ---- custom rules (rule editor) ---------------------------------------------------------------

/** Stored rule overrides keyed by id (`overrides.rules`), or {} when none. */
export function storedRules(stored: ProjectOverrides | null | undefined): Record<string, Partial<RuleConfig> | null> {
  const r = (stored as { rules?: unknown } | null | undefined)?.rules;
  return r && typeof r === "object" && !Array.isArray(r) ? (r as Record<string, Partial<RuleConfig> | null>) : {};
}

export type RuleOrigin = RuleSource | "custom" | "edited";

/** "custom" = added in Warden; "edited" = a repo/built-in rule with a local patch; else its source. */
export function ruleOrigin(r: RuleConfig, stored: ProjectOverrides | null | undefined): RuleOrigin {
  const own = storedRules(stored)[r.id];
  if (own && typeof own.title === "string" && (own.instruction != null || own.pattern != null || own.semgrep != null)) return "custom";
  if (own) return "edited";
  return ruleSource(r);
}

export function originLabel(o: RuleOrigin): string {
  return o === "custom" ? "custom" : o === "edited" ? "edited" : ruleSourceLabel(o);
}

/** Next free id like CR-003. */
export function nextCustomId(existing: string[]): string {
  const used = new Set(existing.map((x) => x.toUpperCase()));
  for (let n = 1; n < 1000; n++) {
    const id = `CR-${String(n).padStart(3, "0")}`;
    if (!used.has(id)) return id;
  }
  return `CR-${Date.now() % 100000}`;
}

/** Overrides with one rule written (custom: full rule; repo/built-in: only the edited fields). */
export function withRule(stored: ProjectOverrides | null | undefined, rule: RuleConfig, base: RuleConfig | null): ProjectOverrides {
  const out = JSON.parse(JSON.stringify(stored ?? {})) as Record<string, unknown>;
  const rules = { ...storedRules(stored) } as Record<string, unknown>;
  if (!base) {
    rules[rule.id] = { title: rule.title, applies_to: rule.applies_to, instruction: rule.instruction, pattern: rule.pattern, semgrep: rule.semgrep, severity: rule.severity };
  } else {
    const patch: Record<string, unknown> = {};
    if (rule.severity !== base.severity) patch.severity = rule.severity;
    if ((rule.applies_to ?? "") !== (base.applies_to ?? "")) patch.applies_to = rule.applies_to || null;
    if (Object.keys(patch).length) rules[rule.id] = patch;
    else delete rules[rule.id];
  }
  out.rules = rules;
  return out as ProjectOverrides;
}

/** Overrides without the local entry for `id` (deletes a custom rule, resets an edited one). */
export function withoutRule(stored: ProjectOverrides | null | undefined, id: string): ProjectOverrides {
  const out = JSON.parse(JSON.stringify(stored ?? {})) as Record<string, unknown>;
  const rules = { ...storedRules(stored) } as Record<string, unknown>;
  delete rules[id];
  out.rules = rules;
  return out as ProjectOverrides;
}

const yamlStr = (s: string) => (/^[\w./*-]+$/.test(s) && !/^(true|false|null|yes|no)$/i.test(s) ? s : JSON.stringify(s));

/** The given rules as a `.warden.yml` `rules:` block, ready to paste and commit. */
export function rulesToYaml(rules: RuleConfig[]): string {
  const lines = ["rules:"];
  for (const r of rules) {
    lines.push(`  - id: ${yamlStr(r.id)}`, `    title: ${yamlStr(r.title)}`);
    if (r.applies_to) lines.push(`    applies_to: ${yamlStr(r.applies_to)}`);
    if (r.instruction) lines.push(`    instruction: ${JSON.stringify(r.instruction)}`);
    if (r.pattern) lines.push(`    pattern: ${JSON.stringify(r.pattern)}`);
    if (r.semgrep) lines.push(`    semgrep: ${yamlStr(r.semgrep)}`);
    lines.push(`    severity: ${r.severity}`);
  }
  return lines.join("\n") + "\n";
}
