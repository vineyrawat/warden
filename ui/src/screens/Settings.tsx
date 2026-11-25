// Settings (9B + 9A): a Mac-preferences toolbar of panes. In Tauri it lives in its own window
// (index.html?view=settings&pane=…, rendered by SettingsApp); in the browser mock it is an in-app
// route rendered full-height in the main area.
import { useCallback, useEffect, useRef, useState } from "react";
import { AtSign, Bot, Ellipsis, FolderGit2, Settings as Gear, Wrench } from "lucide-react";

import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { isTauri } from "@/lib/api";
import { NavContext, SETTINGS_PANES, normalizeRoute, useNav, paneLabel, takeSettingsTarget, type Route, type SettingsPane, type SettingsRoute } from "@/lib/nav";
import { useQueryClient } from "@tanstack/react-query";
import { qk, useEventBridge } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { AccountsPane } from "./settings/Accounts";
import { AdvancedPane } from "./settings/Advanced";
import { ClaudePane } from "./settings/Claude";
import { GeneralPane } from "./settings/General";
import { ProjectsPane } from "./settings/Projects";
import { ToolsPane } from "./settings/Tools";

const ICONS: Record<SettingsPane, React.ComponentType<{ className?: string }>> = {
  general: Gear,
  accounts: AtSign,
  claude: Bot,
  projects: FolderGit2,
  tools: Wrench,
  advanced: Ellipsis,
};

export function Settings({ route, standalone }: { route: SettingsRoute; standalone?: boolean }) {
  const { navigate } = useNav();
  const pane = route.pane ?? "general";
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);
  const go = (p: SettingsPane) => navigate({ screen: "settings", pane: p });

  useEffect(() => {
    if (standalone) document.title = `${paneLabel(pane)} · Warden`;
  }, [pane, standalone]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    const i = SETTINGS_PANES.findIndex((p) => p.id === pane);
    let next = -1;
    if (e.key === "ArrowRight") next = (i + 1) % SETTINGS_PANES.length;
    else if (e.key === "ArrowLeft") next = (i - 1 + SETTINGS_PANES.length) % SETTINGS_PANES.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = SETTINGS_PANES.length - 1;
    if (next < 0) return;
    e.preventDefault();
    go(SETTINGS_PANES[next].id);
    tabs.current[next]?.focus();
  };

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      {standalone && (
        <header data-tauri-drag-region className="relative flex h-9 shrink-0 items-center justify-center bg-chrome text-xs font-semibold">
          <span data-tauri-drag-region>{paneLabel(pane)}</span>
        </header>
      )}
      <div role="tablist" aria-label="Settings" onKeyDown={onKeyDown} data-tauri-drag-region className="flex shrink-0 justify-center gap-1 border-b bg-chrome p-2">
        {SETTINGS_PANES.map((p, i) => {
          const on = p.id === pane;
          const Icon = ICONS[p.id];
          return (
            <button
              key={p.id}
              ref={(el) => {
                tabs.current[i] = el;
              }}
              type="button"
              role="tab"
              id={`settings-tab-${p.id}`}
              aria-selected={on}
              aria-controls="settings-panel"
              tabIndex={on ? 0 : -1}
              onClick={() => go(p.id)}
              className={cn(
                "grid min-w-[64px] justify-items-center gap-0.5 rounded-[7px] px-3 py-[5px] text-2xs transition-colors",
                on ? "bg-card text-foreground shadow-[0_0_0_1px_hsl(var(--line))]" : "text-muted-foreground hover:text-foreground",
              )}
            >
              <Icon className="h-4 w-4" />
              {p.label}
            </button>
          );
        })}
      </div>
      <div id="settings-panel" role="tabpanel" aria-labelledby={`settings-tab-${pane}`} className={cn("min-h-0 flex-1", pane === "projects" ? "overflow-hidden" : "overflow-y-auto")}>
        <PaneBody route={route} pane={pane} />
      </div>
    </div>
  );
}

function PaneBody({ route, pane }: { route: SettingsRoute; pane: SettingsPane }) {
  switch (pane) {
    case "general":
      return <GeneralPane />;
    case "accounts":
      return <AccountsPane />;
    case "claude":
      return <ClaudePane />;
    case "projects":
      return <ProjectsPane projectId={route.projectId} tab={route.tab} />;
    case "tools":
      return <ToolsPane />;
    case "advanced":
      return <AdvancedPane />;
  }
}

/**
 * The Settings window and the main window have separate query caches. Refetch the shared
 * settings-like data when a window regains focus so changes made in the other one show up.
 */
export function useRefreshOnFocus() {
  const qc = useQueryClient();
  useEffect(() => {
    const onFocus = () => {
      for (const key of [qk.settings, qk.projects, qk.forges, qk.usage, qk.engine]) qc.invalidateQueries({ queryKey: key });
      qc.invalidateQueries({ queryKey: ["mrs"] });
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [qc]);
}

/**
 * Root of the separate Settings window (main.tsx renders it for `?view=settings`). It keeps its
 * own route; links to work screens (e.g. "open review" after adding a project) are ignored here.
 */
export function SettingsApp({ initial }: { initial: SettingsRoute }) {
  const [route, setRoute] = useState<SettingsRoute>(() => takeSettingsTarget() ?? initial);
  useEventBridge();
  useRefreshOnFocus();

  const navigate = useCallback((r: Route) => {
    const n = normalizeRoute(r);
    if (n.screen === "settings") setRoute({ ...n, pane: n.pane ?? "general" });
  }, []);

  // The main window leaves project deep links in localStorage before focusing this window.
  useEffect(() => {
    const pick = () => {
      const t = takeSettingsTarget();
      if (t) setRoute(t);
    };
    window.addEventListener("focus", pick);
    window.addEventListener("storage", pick);
    return () => {
      window.removeEventListener("focus", pick);
      window.removeEventListener("storage", pick);
    };
  }, []);

  // ⌘1…⌘6 switch panes, like most Mac preference windows. ⌘W is handled by the native window.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return;
      const n = Number(e.key);
      if (n >= 1 && n <= SETTINGS_PANES.length) {
        e.preventDefault();
        setRoute({ screen: "settings", pane: SETTINGS_PANES[n - 1].id });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <NavContext.Provider value={{ route, navigate }}>
      <TooltipProvider delayDuration={300}>
        <Settings route={route} standalone={isTauri()} />
        <Toaster position="bottom-right" />
      </TooltipProvider>
    </NavContext.Provider>
  );
}
