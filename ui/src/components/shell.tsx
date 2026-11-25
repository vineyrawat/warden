import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { CircleDot, History, Inbox, ListChecks, PauseCircle, Search, Settings as SettingsIcon, ShieldCheck } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { StatusDot } from "@/components/pills";
import { formatClock, isRunActive, runStatusLabel, truncate } from "@/lib/format";
import { activeSection, useNav, type Route } from "@/lib/nav";
import { useEngineStatus, useForges, useMrs, useProjects, useRunProgress, useSetPostingPaused, useSettings, useUsage } from "@/lib/queries";
import { isTauri } from "@/lib/api";
import { cn } from "@/lib/utils";
import type { MrRow } from "@/bindings/MrRow";

// ---- poll countdown --------------------------------------------------------------------------

let lastPollAt = Date.now();
const pollListeners = new Set<() => void>();
/** Called when a poll happens (Poll now, or the backend reports refreshed MRs). */
export function markPolled() {
  lastPollAt = Date.now();
  pollListeners.forEach((l) => l());
}

export function nextPollIn(intervalSecs: number, last: number, now: number): number {
  if (intervalSecs <= 0) return 0;
  const elapsed = Math.max(0, (now - last) / 1000);
  return intervalSecs - (elapsed % intervalSecs);
}

export function useNextPoll(): number | null {
  const projects = useProjects().data;
  const interval = useMemo(() => {
    const enabled = (projects ?? []).filter((p) => p.enabled);
    return enabled.length ? Math.min(...enabled.map((p) => p.poll_interval_secs)) : null;
  }, [projects]);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    const l = () => setNow(Date.now());
    pollListeners.add(l);
    return () => {
      clearInterval(t);
      pollListeners.delete(l);
    };
  }, []);
  return interval == null ? null : nextPollIn(interval, lastPollAt, now);
}

// ---- command palette open state (shared by the title bar, App shortcuts and the palette) -------

let paletteOpen = false;
const paletteListeners = new Set<() => void>();
export function setPaletteOpen(open: boolean) {
  if (paletteOpen === open) return;
  paletteOpen = open;
  paletteListeners.forEach((l) => l());
}
export const togglePalette = () => setPaletteOpen(!paletteOpen);
export function usePaletteOpen(): boolean {
  return useSyncExternalStore(
    (cb) => {
      paletteListeners.add(cb);
      return () => paletteListeners.delete(cb);
    },
    () => paletteOpen,
  );
}

export const IS_MAC = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
/** "⌘K" on macOS, "Ctrl K" elsewhere. */
export const modKey = (key: string) => (IS_MAC ? `⌘${key}` : `Ctrl ${key}`);

export function Kbd({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <kbd className={cn("rounded border border-b-2 bg-card px-[5px] font-mono text-[10.5px] font-medium leading-[1.5] text-muted-foreground", className)}>
      {children}
    </kbd>
  );
}

// ---- title bar -------------------------------------------------------------------------------

export function TitleBar() {
  const { navigate } = useNav();
  const engine = useEngineStatus().data;
  const forges = useForges().data;
  const forge = forges?.find((f) => f.ok) ?? forges?.[0];
  const host = forge ? forge.base_url.replace(/^https?:\/\//, "").replace(/^api\./, "") : "";

  return (
    <header data-tauri-drag-region className="flex h-9 shrink-0 items-center gap-2.5 border-b bg-chrome px-3 text-xs" aria-label="Title bar">
      {isTauri() ? <div className="w-14" data-tauri-drag-region /> : <WindowDots />}
      <ShieldCheck className="h-3.5 w-3.5 text-primary" aria-hidden />
      <span className="font-semibold" data-tauri-drag-region>
        Warden
      </span>
      {!isTauri() && <Badge tone="grey" className="font-mono font-medium">mock backend</Badge>}
      <div className="ml-auto flex min-w-0 items-center gap-3 text-muted-foreground" data-tauri-drag-region>
        {engine && !engine.ok && (
          <button type="button" className="rounded px-1 hover:text-foreground" onClick={() => navigate({ screen: "settings", pane: "general" })}>
            <StatusDot warn>Claude not ready</StatusDot>
          </button>
        )}
        {forges &&
          (forge ? (
            <StatusDot ok={forge.ok} warn={!forge.ok}>
              {host}
              {forge.user ? ` · @${forge.user.replace(/^@/, "")}` : ""}
            </StatusDot>
          ) : (
            <button type="button" className="rounded px-1 hover:text-foreground" onClick={() => navigate({ screen: "settings", pane: "accounts" })}>
              <StatusDot warn>No forge connected</StatusDot>
            </button>
          ))}
        <button
          type="button"
          onClick={() => setPaletteOpen(true)}
          className="flex h-6 items-center gap-1.5 rounded-md border bg-card px-2 text-muted-foreground hover:text-foreground"
          aria-label="Open command palette"
          aria-keyshortcuts={IS_MAC ? "Meta+K" : "Control+K"}
        >
          <Search className="h-3 w-3" aria-hidden />
          <span>Search</span>
          <Kbd className="border-b">{modKey("K")}</Kbd>
        </button>
      </div>
    </header>
  );
}

function WindowDots() {
  return (
    <div className="flex gap-1.5" aria-hidden>
      <i className="block h-[11px] w-[11px] rounded-full bg-line" />
      <i className="block h-[11px] w-[11px] rounded-full bg-line" />
      <i className="block h-[11px] w-[11px] rounded-full bg-line" />
    </div>
  );
}

// ---- sidebar ---------------------------------------------------------------------------------

type Item = { id: "inbox" | "queue" | "ci" | "history"; route: Route; label: string; icon: React.ComponentType<{ className?: string }>; count?: number };

const itemClass = (on: boolean) =>
  cn(
    "flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[13px] text-foreground transition-colors",
    on ? "bg-card font-semibold shadow-[inset_0_0_0_1px_hsl(var(--line))]" : "hover:bg-card/60",
  );

export function Sidebar() {
  const { route, navigate } = useNav();
  const mrs = useMrs(null).data;
  const pending = (mrs ?? []).reduce((n, m) => n + m.finding_counts.draft, 0);
  const failing = (mrs ?? []).filter((m) => m.ci_summary.failed > 0).length;
  const active = activeSection(route);

  const main: Item[] = [
    { id: "inbox", route: { screen: "inbox" }, label: "Inbox", icon: Inbox, count: mrs?.length },
    { id: "queue", route: { screen: "queue" }, label: "Approval queue", icon: ListChecks, count: pending || undefined },
    { id: "ci", route: { screen: "ci" }, label: "CI runs", icon: CircleDot, count: failing || undefined },
    { id: "history", route: { screen: "history" }, label: "History", icon: History },
  ];

  return (
    <nav aria-label="Main" className="flex w-[184px] shrink-0 flex-col gap-0.5 border-r bg-surface-2 px-2 py-3">
      <ul className="flex flex-col gap-0.5">
        {main.map((it) => {
          const on = active === it.id;
          const Icon = it.icon;
          return (
            <li key={it.id}>
              <button type="button" aria-current={on ? "page" : undefined} onClick={() => navigate(it.route)} className={itemClass(on)}>
                <Icon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                <span className="flex-1 truncate">{it.label}</span>
                {it.count != null && <small className="font-mono text-2xs text-muted-foreground">{it.count}</small>}
              </button>
            </li>
          );
        })}
      </ul>
      <div className="h-4" aria-hidden />
      <button
        type="button"
        aria-current={active === "settings" ? "page" : undefined}
        aria-keyshortcuts={IS_MAC ? "Meta+Comma" : "Control+Comma"}
        onClick={() => navigate({ screen: "settings" })}
        className={itemClass(active === "settings")}
      >
        <SettingsIcon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <span className="flex-1 truncate">Settings</span>
        <small className="font-mono text-2xs text-muted-foreground">{modKey(",")}</small>
      </button>
      <div className="mt-auto flex flex-col gap-2 pt-3">
        <RunCard mrs={mrs} />
        <UsageCard />
      </div>
    </nav>
  );
}

const cardClass = "grid gap-[5px] rounded-lg border bg-background p-2 text-[11.5px]";

function Meter({ value, tone = "brand" }: { value: number; tone?: "brand" | "green" | "orange" | "red" }) {
  const pct = Math.max(0, Math.min(1, value)) * 100;
  return (
    <div className="h-1 overflow-hidden rounded-sm bg-surface-2" aria-hidden>
      <i
        className={cn("block h-full transition-[width]", { brand: "bg-primary", green: "bg-green", orange: "bg-orange", red: "bg-red" }[tone])}
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}

/** Active runs, the one currently talking to Claude first. */
export function activeRuns(mrs: MrRow[] | undefined): MrRow[] {
  const list = (mrs ?? []).filter((m) => m.latest_run && isRunActive(m.latest_run.status));
  const rank = (m: MrRow) => (m.latest_run!.status === "reviewing" ? 0 : m.latest_run!.status === "queued" ? 2 : 1);
  return list.sort((a, b) => rank(a) - rank(b) || a.latest_run!.id - b.latest_run!.id);
}

function RunCard({ mrs }: { mrs: MrRow[] | undefined }) {
  const { navigate } = useNav();
  const maxTurns = useSettings().data?.max_turns ?? 30;
  const runs = activeRuns(mrs);
  const current = runs[0];
  const run = current?.latest_run ?? null;
  const progress = useRunProgress(run?.id);

  if (!current || !run)
    return (
      <div className={cardClass} aria-live="polite">
        <div className="flex items-center gap-1.5">
          <span aria-hidden className="h-[7px] w-[7px] rounded-full bg-muted-foreground/50" />
          <b>Idle</b>
        </div>
        <span className="text-muted-foreground">No review running</span>
      </div>
    );

  const reviewing = run.status === "reviewing";
  const turn = progress?.turn ?? run.turns;
  const detail = reviewing ? `turn ${turn} / ${maxTurns}${progress?.message ? ` · ${progress.message}` : ""}` : progress?.message ?? runStatusLabel(run.status);
  return (
    <button
      type="button"
      className={cn(cardClass, "text-left hover:border-primary/60")}
      onClick={() => navigate({ screen: "review", runId: run.id })}
      aria-label={`${reviewing ? "Reviewing" : runStatusLabel(run.status)} !${current.iid} ${current.title}. Open review.`}
    >
      <div className="flex items-center gap-1.5" aria-live="polite">
        <span aria-hidden className="h-[7px] w-[7px] shrink-0 animate-pulse rounded-full bg-primary" />
        <b className="truncate">
          {reviewing ? "Reviewing" : runStatusLabel(run.status)} !{current.iid}
        </b>
      </div>
      <span className="truncate text-muted-foreground" title={current.title}>
        {current.title}
      </span>
      {reviewing && <Meter value={turn / Math.max(1, maxTurns)} />}
      <span className="truncate font-mono text-2xs text-muted-foreground" title={detail}>
        {truncate(detail, 80)}
      </span>
      {runs.length > 1 && <span className="text-2xs text-muted-foreground">+{runs.length - 1} more queued</span>}
    </button>
  );
}

function UsageCard() {
  const usage = useUsage().data;
  const next = useNextPoll();
  const resume = useSetPostingPaused();
  const used = usage?.reviews_last_hour ?? 0;
  const max = usage?.max_reviews_per_hour ?? 0;
  const ratio = max > 0 ? used / max : 0;
  const tone = ratio >= 1 ? "red" : ratio >= 0.8 ? "orange" : "green";
  return (
    <div className={cardClass}>
      <div className="flex items-center gap-2">
        <span className="flex-1">Reviews this hour</span>
        <b className="font-mono">{usage ? `${used} / ${max}` : "…"}</b>
      </div>
      <Meter value={ratio} tone={tone} />
      <span className="font-mono text-2xs tabular-nums text-muted-foreground">
        {next != null ? `next poll ${formatClock(next)}` : "no projects watched"}
      </span>
      {usage?.paused && (
        <div className="-mx-2 -mb-2 mt-1 flex items-center gap-1.5 rounded-b-lg border-t bg-orange-soft px-2 py-1.5 text-orange">
          <PauseCircle className="h-3 w-3 shrink-0" aria-hidden />
          <span className="flex-1 font-semibold">Posting paused</span>
          <Button size="sm" variant="outline" className="h-5 px-1.5" disabled={resume.isPending} onClick={() => resume.mutate(false)}>
            Resume
          </Button>
        </div>
      )}
    </div>
  );
}
