// Pure helpers for the Projects pane. Unit tested in settings.test.ts.
import type { Project } from "@/bindings/Project";
import { modeLabel, splitGlobs } from "@/lib/format";

export function projectSummary(p: Pick<Project, "mode" | "target_branches" | "enabled">): string {
  const s = `${modeLabel(p.mode)} · ${p.target_branches.join(", ") || "no branches"}`;
  return p.enabled ? s : `${s} · paused`;
}

/** Adds globs to a branch list, skipping blanks and duplicates. */
export function addBranches(list: string[], input: string): string[] {
  const out = [...list];
  for (const b of splitGlobs(input)) if (!out.includes(b)) out.push(b);
  return out;
}
