// Minimal state routing (SPEC: no router needed). The Route type is the navigation contract in
// docs/UX-BUILD.md; every screen relies on it.
import { createContext, useContext } from "react";

export type SettingsPane = "general" | "accounts" | "claude" | "projects" | "tools" | "advanced";
export type ProjectTab = "overview" | "guardrails";

export type Route =
  | { screen: "inbox"; mrId?: number }
  | { screen: "review"; runId: number; findingId?: number; tab?: "findings" | "diff" | "ci" | "summary" }
  | { screen: "queue" }
  | { screen: "ci"; runId?: number; jobId?: number }
  | { screen: "history"; runId?: number }
  | { screen: "settings"; pane?: SettingsPane; projectId?: number; tab?: ProjectTab }
  // legacy, kept so old links work; App maps them to settings:
  | { screen: "guardrails"; projectId?: number }
  | { screen: "projects" }
  | { screen: "connections" };

export type Screen = Route["screen"];

export type Nav = { route: Route; navigate: (r: Route) => void };

export const NavContext = createContext<Nav>({ route: { screen: "inbox" }, navigate: () => {} });

export function useNav(): Nav {
  return useContext(NavContext);
}

export const SETTINGS_PANES: { id: SettingsPane; label: string }[] = [
  { id: "general", label: "General" },
  { id: "accounts", label: "Accounts" },
  { id: "claude", label: "Claude" },
  { id: "projects", label: "Projects" },
  { id: "tools", label: "Tools" },
  { id: "advanced", label: "Advanced" },
];

export function isSettingsPane(v: unknown): v is SettingsPane {
  return typeof v === "string" && SETTINGS_PANES.some((p) => p.id === v);
}

export function paneLabel(p: SettingsPane): string {
  return SETTINGS_PANES.find((x) => x.id === p)?.label ?? "Settings";
}

export type SettingsRoute = Extract<Route, { screen: "settings" }>;

/** Maps the legacy setup routes onto Settings; every other route is returned unchanged. */
export function normalizeRoute(r: Route): Route {
  switch (r.screen) {
    case "guardrails":
      return { screen: "settings", pane: "projects", projectId: r.projectId, tab: r.projectId != null ? "guardrails" : undefined };
    case "projects":
      return { screen: "settings", pane: "projects" };
    case "connections":
      return { screen: "settings", pane: "general" };
    default:
      return r;
  }
}

/** The sidebar item a route belongs to (Review lives under Inbox). */
export function activeSection(r: Route): "inbox" | "queue" | "ci" | "history" | "settings" {
  const n = normalizeRoute(r);
  switch (n.screen) {
    case "review":
    case "inbox":
      return "inbox";
    case "queue":
    case "ci":
    case "history":
    case "settings":
      return n.screen;
    default:
      return "inbox";
  }
}

/** Parses `?view=settings&pane=…&project=…&tab=…` for the separate Settings window. */
export function settingsRouteFromSearch(search: string): SettingsRoute | null {
  const q = new URLSearchParams(search);
  if (q.get("view") !== "settings") return null;
  const pane = q.get("pane");
  const project = Number(q.get("project"));
  const tab = q.get("tab");
  return {
    screen: "settings",
    pane: isSettingsPane(pane) ? pane : "general",
    projectId: Number.isFinite(project) && project > 0 ? project : undefined,
    tab: tab === "guardrails" || tab === "overview" ? tab : undefined,
  };
}

// ---- hand-off to the Settings window ----------------------------------------------------------
// open_settings_window(pane) only carries the pane. A project deep link (projectId + tab) is left
// in localStorage for the Settings window, which reads it on load, on focus and on `storage`.

const TARGET_KEY = "warden.settings.target";

export function storeSettingsTarget(r: SettingsRoute) {
  try {
    localStorage.setItem(TARGET_KEY, JSON.stringify({ ...r, at: Date.now() }));
  } catch {
    /* storage unavailable: the window still opens on the pane */
  }
}

/** Returns and clears a target stored in the last 30 seconds. */
export function takeSettingsTarget(now: number = Date.now()): SettingsRoute | null {
  try {
    const raw = localStorage.getItem(TARGET_KEY);
    if (!raw) return null;
    localStorage.removeItem(TARGET_KEY);
    const t = JSON.parse(raw) as SettingsRoute & { at?: number };
    if (!t || t.screen !== "settings" || !t.at || now - t.at > 30_000) return null;
    return { screen: "settings", pane: isSettingsPane(t.pane) ? t.pane : "general", projectId: t.projectId, tab: t.tab };
  } catch {
    return null;
  }
}

export { TARGET_KEY as SETTINGS_TARGET_KEY };
