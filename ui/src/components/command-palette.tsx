// ⌘K command palette: jump to an MR, start or re-run a review, post approved notes, pause posting,
// poll, open Settings panes and a project's guardrails. Searches data already loaded by the hooks.
import { useEffect, useMemo, useState } from "react";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { useQueryClient } from "@tanstack/react-query";
import { Search } from "lucide-react";
import { toast } from "sonner";

import { Kbd, markPolled, modKey, setPaletteOpen, usePaletteOpen, IS_MAC } from "@/components/shell";
import * as api from "@/lib/api";
import { isRunActive } from "@/lib/format";
import { SETTINGS_PANES, useNav, type Route, type SettingsPane } from "@/lib/nav";
import { qk, useMrs, usePollNow, usePostFindings, useProjects, useSetPostingPaused, useStartReview, useUsage } from "@/lib/queries";
import { cn } from "@/lib/utils";
import type { MrRow } from "@/bindings/MrRow";
import type { Project } from "@/bindings/Project";

// ---- pure search -----------------------------------------------------------------------------

export type GroupId = "mrs" | "actions" | "settings" | "goto";
export const GROUPS: { id: GroupId; label: string }[] = [
  { id: "mrs", label: "Merge requests" },
  { id: "actions", label: "Actions" },
  { id: "settings", label: "Settings" },
  { id: "goto", label: "Go to" },
];

export type Command = {
  id: string;
  group: GroupId;
  label: string;
  /** Mono prefix shown before the label, e.g. "!41". Searched like the label. */
  prefix?: string;
  /** Muted text on the right, e.g. the project name. */
  hint?: string;
  /** Extra search text that is not shown (project, author, branches). Weighted lower. */
  keywords?: string;
  shortcut?: string;
  /** Hidden until the user types something (per-MR actions would flood the empty list). */
  needsQuery?: boolean;
  /** Added to the match score; per-MR actions use a small negative value so "open" ranks first. */
  boost?: number;
  run: () => void;
};

const isBoundary = (c: string | undefined) => c === undefined || !/[a-z0-9]/i.test(c);

/** Score one query token against text (both lowercase). null = no match. */
function tokenScore(tok: string, text: string): number | null {
  const idx = text.indexOf(tok);
  if (idx >= 0) {
    let s = 100;
    if (isBoundary(text[idx - 1])) s += 50;
    if (isBoundary(text[idx + tok.length])) s += 20;
    return s - Math.min(idx, 40) * 0.5;
  }
  // Subsequence match ("rrv" → "Re-run review"): reward consecutive runs, penalise gaps.
  let ti = 0;
  let gaps = 0;
  let run = 0;
  let bonus = 0;
  let last = -2;
  for (let i = 0; i < text.length && ti < tok.length; i++) {
    if (text[i] !== tok[ti]) continue;
    if (i === last + 1) {
      run++;
      bonus += run * 2;
    } else {
      run = 0;
      if (last >= 0) gaps++;
      if (isBoundary(text[i - 1])) bonus += 4;
    }
    last = i;
    ti++;
  }
  if (ti < tok.length) return null;
  return Math.max(1, 30 + bonus - gaps * 3);
}

/**
 * Fuzzy score of a query against a command: every whitespace-separated token must match the
 * visible text (prefix + label) or, at a lower weight, the keywords. Higher is better; null = hidden.
 */
export function fuzzyScore(query: string, primary: string, keywords = ""): number | null {
  const tokens = query.toLowerCase().trim().split(/\s+/).filter(Boolean);
  if (!tokens.length) return 0;
  const p = primary.toLowerCase();
  const k = keywords.toLowerCase();
  let total = 0;
  for (const tok of tokens) {
    const a = tokenScore(tok, p);
    const b = k ? tokenScore(tok, k) : null;
    const best = Math.max(a ?? -1, b == null ? -1 : b * 0.6);
    if (best < 0) return null;
    total += best;
  }
  return total - p.length * 0.05;
}

export type CommandGroup<T> = { id: GroupId; label: string; items: T[] };

/** Filter + rank commands, grouped in the fixed group order. Empty groups are dropped. */
export function filterCommands<T extends Pick<Command, "group" | "label" | "prefix" | "keywords" | "needsQuery" | "boost">>(
  commands: T[],
  query: string,
  { emptyLimit = 6, limit = 8 }: { emptyLimit?: number; limit?: number } = {},
): CommandGroup<T>[] {
  const q = query.trim();
  const scored: { c: T; s: number; i: number }[] = [];
  commands.forEach((c, i) => {
    if (!q) {
      if (!c.needsQuery) scored.push({ c, s: 0, i });
      return;
    }
    const s = fuzzyScore(q, c.prefix ? `${c.prefix} ${c.label}` : c.label, c.keywords);
    if (s != null) scored.push({ c, s: s + (c.boost ?? 0), i });
  });
  return GROUPS.map((g) => {
    const items = scored
      .filter((x) => x.c.group === g.id)
      .sort((a, b) => b.s - a.s || a.i - b.i)
      .slice(0, q ? limit : emptyLimit)
      .map((x) => x.c);
    return { id: g.id, label: g.label, items };
  }).filter((g) => g.items.length > 0);
}

/** Order MRs for the empty palette: waiting for you first, then running, then the rest. */
export function mrPriority(m: Pick<MrRow, "latest_run">): number {
  const st = m.latest_run?.status;
  if (st === "awaiting_approval") return 0;
  if (st && isRunActive(st)) return 1;
  return 2;
}

const PANE_KEYWORDS: Record<SettingsPane, string> = {
  general: "setup health checklist status connections",
  accounts: "gitlab github forge token keychain",
  claude: "engine model turns limits timeout reviews per hour cli",
  projects: "repositories watch branches mode poll",
  tools: "semgrep gitleaks ci docker act install",
  advanced: "data folder worktree mcp posting paused",
};

// ---- component -------------------------------------------------------------------------------

type BuildDeps = {
  mrs: MrRow[];
  projects: Project[];
  paused: boolean;
  navigate: (r: Route) => void;
  startReview: (mrId: number) => void;
  postApproved: (mr: MrRow) => void;
  setPaused: (p: boolean) => void;
  pollNow: () => void;
};

function buildCommands(d: BuildDeps): Command[] {
  const out: Command[] = [];
  const mrs = [...d.mrs].sort((a, b) => mrPriority(a) - mrPriority(b));
  for (const m of mrs) {
    const run = m.latest_run;
    const keywords = `${m.project_name} ${m.author} ${m.source_branch} ${m.target_branch}`;
    out.push({
      id: `mr-${m.id}`,
      group: "mrs",
      prefix: `!${m.iid}`,
      label: m.title,
      hint: m.project_name,
      keywords,
      run: () => d.navigate(run ? { screen: "review", runId: run.id } : { screen: "inbox", mrId: m.id }),
    });
    if (run?.status === "awaiting_approval")
      out.push({
        id: `post-${m.id}`,
        group: "mrs",
        prefix: `!${m.iid}`,
        label: `Post approved notes to !${m.iid}`,
        keywords: `${m.title} ${m.project_name} send publish`,
        needsQuery: true,
        boost: -10,
        run: () => d.postApproved(m),
      });
    if (!run || !isRunActive(run.status))
      out.push({
        id: `review-${m.id}`,
        group: "actions",
        label: `${run ? "Re-run review on" : "Start review on"} !${m.iid}`,
        hint: m.project_name,
        keywords: `${m.title} ${m.project_name} rerun restart`,
        needsQuery: true,
        run: () => d.startReview(m.id),
      });
  }
  out.push(
    d.paused
      ? { id: "resume", group: "actions", label: "Resume posting", keywords: "unpause posting", shortcut: `${IS_MAC ? "⌘⇧" : "Ctrl Shift "}P`, run: () => d.setPaused(false) }
      : { id: "pause", group: "actions", label: "Pause posting everywhere", keywords: "stop posting", shortcut: `${IS_MAC ? "⌘⇧" : "Ctrl Shift "}P`, run: () => d.setPaused(true) },
    { id: "poll", group: "actions", label: "Poll now", keywords: "refresh fetch merge requests", run: d.pollNow },
  );
  for (const p of SETTINGS_PANES)
    out.push({
      id: `pane-${p.id}`,
      group: "settings",
      label: `Settings: ${p.label}`,
      keywords: PANE_KEYWORDS[p.id],
      shortcut: p.id === "general" ? modKey(",") : undefined,
      run: () => d.navigate({ screen: "settings", pane: p.id }),
    });
  for (const p of d.projects)
    out.push({
      id: `guardrails-${p.id}`,
      group: "settings",
      label: `Open ${p.name} guardrails`,
      keywords: `${p.remote_id} rules filters posting confidence`,
      needsQuery: true,
      run: () => d.navigate({ screen: "settings", pane: "projects", projectId: p.id, tab: "guardrails" }),
    });
  const go: [string, Route][] = [
    ["Inbox", { screen: "inbox" }],
    ["Approval queue", { screen: "queue" }],
    ["CI runs", { screen: "ci" }],
    ["History", { screen: "history" }],
  ];
  for (const [label, route] of go) out.push({ id: `go-${route.screen}`, group: "goto", label, needsQuery: true, run: () => d.navigate(route) });
  return out;
}

/** Mounted once by App. Opens with ⌘K / Ctrl+K (App owns the shortcut) or the title-bar button. */
export function CommandPalette() {
  const open = usePaletteOpen();
  return (
    <DialogPrimitive.Root open={open} onOpenChange={setPaletteOpen}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-50 bg-black/30 data-[state=open]:animate-in data-[state=open]:fade-in-0" />
        <DialogPrimitive.Content
          aria-describedby={undefined}
          className="fixed left-1/2 top-[12vh] z-50 w-[min(560px,calc(100vw-32px))] -translate-x-1/2 overflow-hidden rounded-xl border bg-card text-foreground shadow-lg focus:outline-none data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95"
        >
          <DialogPrimitive.Title className="sr-only">Command palette</DialogPrimitive.Title>
          {open && <PaletteBody />}
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}

function PaletteBody() {
  const { navigate } = useNav();
  const qc = useQueryClient();
  const mrs = useMrs(null).data ?? [];
  const projects = useProjects().data ?? [];
  const paused = useUsage().data?.paused ?? false;
  const start = useStartReview((runId) => navigate({ screen: "review", runId }));
  const post = usePostFindings();
  const setPausedM = useSetPostingPaused();
  const poll = usePollNow();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);

  const commands = useMemo(
    () =>
      buildCommands({
        mrs,
        projects,
        paused,
        navigate,
        startReview: (id) => start.mutate(id),
        postApproved: async (m) => {
          const runId = m.latest_run!.id;
          try {
            const detail = await qc.fetchQuery({ queryKey: qk.run(runId), queryFn: () => api.get_run(runId) });
            const ids = detail.findings.filter((f) => f.status === "approved").map((f) => f.id);
            if (!ids.length) {
              toast.info(`No approved notes on !${m.iid} yet. Approve findings first.`);
              navigate({ screen: "review", runId });
              return;
            }
            post.mutate({ runId, ids });
          } catch (e) {
            toast.error(e instanceof Error ? e.message : String(e));
          }
        },
        setPaused: (p) => setPausedM.mutate(p, { onSuccess: () => toast.success(p ? "Posting paused everywhere" : "Posting resumed") }),
        pollNow: () =>
          poll.mutate(undefined, {
            onSuccess: () => {
              markPolled();
              toast.success("Polled for merge requests");
            },
          }),
      }),
    // Mutation objects change identity every render; their mutate functions are stable enough.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [mrs, projects, paused, navigate, qc],
  );

  const groups = useMemo(() => filterCommands(commands, query), [commands, query]);
  const flat = useMemo(() => groups.flatMap((g) => g.items), [groups]);
  const idx = flat.length ? Math.min(active, flat.length - 1) : -1;
  const activeId = idx >= 0 ? `cmd-${flat[idx].id}` : undefined;

  useEffect(() => setActive(0), [query]);
  useEffect(() => {
    if (activeId) document.getElementById(activeId)?.scrollIntoView({ block: "nearest" });
  }, [activeId]);

  const runAt = (i: number) => {
    const c = flat[i];
    if (!c) return;
    setPaletteOpen(false);
    c.run();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (flat.length) setActive((idx + 1) % flat.length);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (flat.length) setActive((idx - 1 + flat.length) % flat.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      runAt(idx);
    }
  };

  return (
    <>
      <div className="flex items-center gap-2 border-b px-3.5 py-3 text-sm">
        <Search className="h-4 w-4 shrink-0 text-muted-foreground" aria-hidden />
        <input
          autoFocus
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-activedescendant={activeId}
          aria-autocomplete="list"
          aria-label="Search merge requests, actions and settings"
          placeholder="Jump to an MR, run an action, open settings…"
          className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          spellCheck={false}
        />
        <Kbd>esc</Kbd>
      </div>
      <div id="palette-list" role="listbox" aria-label="Commands" className="max-h-[min(420px,60vh)] overflow-y-auto pb-1.5">
        {groups.length === 0 && <div className="px-3.5 py-6 text-center text-xs text-muted-foreground">Nothing matches “{query}”.</div>}
        {groups.map((g) => (
          <div key={g.id} role="group" aria-labelledby={`grp-${g.id}`}>
            <div id={`grp-${g.id}`} className="px-3.5 pb-0.5 pt-2 font-mono text-[10.5px] font-medium uppercase tracking-[0.07em] text-muted-foreground">
              {g.label}
            </div>
            {g.items.map((c) => {
              const i = flat.indexOf(c);
              const on = i === idx;
              return (
                <div
                  key={c.id}
                  id={`cmd-${c.id}`}
                  role="option"
                  aria-selected={on}
                  onMouseMove={() => on || setActive(i)}
                  onClick={() => runAt(i)}
                  className={cn("flex cursor-default items-center gap-2.5 px-3.5 py-[7px] text-[12.5px]", on && "bg-brand-soft")}
                >
                  {c.prefix && <span className="shrink-0 font-mono text-muted-foreground">{c.prefix}</span>}
                  <span className="min-w-0 flex-1 truncate">{c.label}</span>
                  {c.hint && <span className="max-w-[140px] shrink-0 truncate text-2xs text-muted-foreground">{c.hint}</span>}
                  {c.shortcut && <Kbd>{c.shortcut}</Kbd>}
                  {on && c.group === "mrs" && !c.id.startsWith("post-") && <Kbd>↵ open</Kbd>}
                </div>
              );
            })}
          </div>
        ))}
      </div>
      <div className="flex items-center gap-3 border-t bg-surface-2 px-3.5 py-1.5 text-2xs text-muted-foreground">
        <span>
          <Kbd>↑</Kbd> <Kbd>↓</Kbd> move
        </span>
        <span>
          <Kbd>↵</Kbd> run
        </span>
        <span className="ml-auto">{modKey("K")} toggles</span>
      </div>
    </>
  );
}
