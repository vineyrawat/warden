import { beforeEach, describe, expect, it } from "vitest";

import { activeSection, normalizeRoute, settingsRouteFromSearch, storeSettingsTarget, takeSettingsTarget } from "@/lib/nav";

describe("normalizeRoute", () => {
  it("maps legacy setup routes onto Settings", () => {
    expect(normalizeRoute({ screen: "guardrails", projectId: 3 })).toEqual({ screen: "settings", pane: "projects", projectId: 3, tab: "guardrails" });
    expect(normalizeRoute({ screen: "guardrails" })).toEqual({ screen: "settings", pane: "projects", projectId: undefined, tab: undefined });
    expect(normalizeRoute({ screen: "projects" })).toEqual({ screen: "settings", pane: "projects" });
    expect(normalizeRoute({ screen: "connections" })).toEqual({ screen: "settings", pane: "general" });
  });
  it("leaves work routes alone", () => {
    const r = { screen: "review" as const, runId: 4, tab: "diff" as const };
    expect(normalizeRoute(r)).toBe(r);
  });
});

describe("activeSection", () => {
  it("puts Review under Inbox and legacy routes under Settings", () => {
    expect(activeSection({ screen: "review", runId: 1 })).toBe("inbox");
    expect(activeSection({ screen: "guardrails" })).toBe("settings");
    expect(activeSection({ screen: "ci" })).toBe("ci");
  });
});

describe("settingsRouteFromSearch", () => {
  it("parses the Settings window query", () => {
    expect(settingsRouteFromSearch("?view=settings&pane=claude")).toEqual({ screen: "settings", pane: "claude", projectId: undefined, tab: undefined });
    expect(settingsRouteFromSearch("?view=settings&pane=projects&project=7&tab=guardrails")).toEqual({ screen: "settings", pane: "projects", projectId: 7, tab: "guardrails" });
  });
  it("falls back to General for an unknown pane and ignores other views", () => {
    expect(settingsRouteFromSearch("?view=settings&pane=nope")!.pane).toBe("general");
    expect(settingsRouteFromSearch("?pane=claude")).toBeNull();
  });
});

describe("settings target hand-off", () => {
  const store = new Map<string, string>();
  beforeEach(() => {
    store.clear();
    globalThis.localStorage = {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
      removeItem: (k: string) => void store.delete(k),
      clear: () => store.clear(),
      key: () => null,
      length: 0,
    } as Storage;
  });
  it("returns a fresh target once", () => {
    storeSettingsTarget({ screen: "settings", pane: "projects", projectId: 2, tab: "guardrails" });
    expect(takeSettingsTarget()).toEqual({ screen: "settings", pane: "projects", projectId: 2, tab: "guardrails" });
    expect(takeSettingsTarget()).toBeNull();
  });
  it("ignores a stale target", () => {
    storeSettingsTarget({ screen: "settings", pane: "tools" });
    expect(takeSettingsTarget(Date.now() + 60_000)).toBeNull();
  });
});
