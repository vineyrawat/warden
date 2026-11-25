import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import { TooltipProvider } from "@/components/ui/tooltip";
import { Toaster } from "@/components/ui/sonner";
import { Sidebar, TitleBar, setPaletteOpen, togglePalette } from "@/components/shell";
import { CommandPalette } from "@/components/command-palette";
import * as api from "@/lib/api";
import { NavContext, normalizeRoute, storeSettingsTarget, type Route, type SettingsRoute } from "@/lib/nav";
import { useEventBridge, useSetPostingPaused, useUsage } from "@/lib/queries";
import { InboxScreen } from "@/screens/Inbox";
import { ReviewScreen } from "@/screens/Review";
import { ApprovalQueueScreen } from "@/screens/ApprovalQueue";
import { CiScreen } from "@/screens/Ci";
import { HistoryScreen } from "@/screens/History";
import { Settings, useRefreshOnFocus } from "@/screens/Settings";

/** Tauri: open (or focus) the Settings window. Resolves false when the window can't be opened. */
async function openSettingsWindow(r: SettingsRoute): Promise<boolean> {
  if (!api.isTauri()) return false;
  if (r.projectId != null) storeSettingsTarget(r);
  try {
    await api.open_settings_window(r.pane ?? null);
    return true;
  } catch (e) {
    console.warn("warden: settings window unavailable, using the in-app page", e);
    return false;
  }
}

export default function App() {
  const [route, setRoute] = useState<Route>({ screen: "inbox" });
  const navigate = useCallback((r: Route) => {
    const n = normalizeRoute(r);
    if (n.screen === "settings" && api.isTauri()) {
      void openSettingsWindow(n).then((ok) => ok || setRoute(n));
      return;
    }
    setRoute(n);
  }, []);
  useEventBridge();
  useRefreshOnFocus();
  useShortcuts(navigate);

  return (
    <NavContext.Provider value={{ route, navigate }}>
      <TooltipProvider delayDuration={300}>
        <div className="flex h-full flex-col">
          <TitleBar />
          <div className="flex min-h-0 flex-1">
            <Sidebar />
            <main className="flex min-w-0 flex-1 flex-col overflow-hidden bg-surface">
              <Screen route={route} />
            </main>
          </div>
        </div>
        <CommandPalette />
        <Toaster position="bottom-right" offset={{ bottom: 60, right: 16 }} />
      </TooltipProvider>
    </NavContext.Provider>
  );
}

/** ⌘K / Ctrl+K palette, ⌘, Settings, ⌘⇧P pause or resume posting. */
function useShortcuts(navigate: (r: Route) => void) {
  const paused = useUsage().data?.paused ?? false;
  const setPaused = useSetPostingPaused();
  const state = useRef({ paused, setPaused });
  state.current = { paused, setPaused };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (!mod || e.altKey) return;
      const key = e.key.toLowerCase();
      if (key === "k" && !e.shiftKey) {
        e.preventDefault();
        togglePalette();
      } else if (key === "," && !e.shiftKey) {
        e.preventDefault();
        setPaletteOpen(false);
        navigate({ screen: "settings" });
      } else if (key === "p" && e.shiftKey) {
        e.preventDefault();
        const { paused: p, setPaused: m } = state.current;
        m.mutate(!p, { onSuccess: () => toast.success(p ? "Posting resumed" : "Posting paused everywhere") });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);
}

function Screen({ route }: { route: Route }) {
  const r = normalizeRoute(route);
  switch (r.screen) {
    case "inbox":
      return <InboxScreen key={r.mrId ?? "inbox"} initialMrId={r.mrId} />;
    case "review":
      return <ReviewScreen key={r.runId} runId={r.runId} focusFindingId={r.findingId} tab={r.tab} />;
    case "queue":
      return <ApprovalQueueScreen />;
    case "ci":
      return <CiScreen initialRunId={r.runId} initialJobId={r.jobId} />;
    case "history":
      return <HistoryScreen initialRunId={r.runId} />;
    case "settings":
      return <Settings route={r} />;
    default:
      return null;
  }
}
