import { describe, expect, it } from "vitest";

import type { AppSettings } from "@/bindings/AppSettings";
import type { EngineStatus } from "@/bindings/EngineStatus";
import type { Project } from "@/bindings/Project";
import { buildChecklist, readiness, toolFix, CLAUDE_INSTALL } from "./health";
import { applyLimits, draftFromSettings, limitsDirty, validateLimits } from "./limits";
import { addBranches, projectSummary } from "./project-helpers";

const engineOk: EngineStatus = { cli_path: "/opt/homebrew/bin/claude", version: "2.1.321", logged_in: true, subscription: "Max", api_key_in_env: false, ok: true, message: "ready" };
const project = (over: Partial<Project> = {}): Project => ({
  id: 1,
  forge: "gitlab",
  base_url: "https://gitlab.com",
  remote_id: "entries/entries_crm",
  name: "entries_crm",
  local_path: "/code/entries_crm",
  target_branches: ["develop", "main"],
  mode: "draft",
  poll_interval_secs: 300,
  enabled: true,
  ...over,
});
const forge = { forge: "gitlab" as const, base_url: "https://gitlab.com", user: "vinay", ok: true };

describe("buildChecklist", () => {
  it("is all green for a complete setup", () => {
    const items = buildChecklist({
      engine: engineOk,
      forges: [forge],
      projects: [project()],
      tools: [
        { name: "git", installed: true, version: "2.45" },
        { name: "semgrep", installed: true, version: "1.80" },
      ],
      mcpPort: 7811,
    });
    expect(items.every((i) => i.level === "ok")).toBe(true);
    expect(items.find((i) => i.id === "cli")!.detail).toBe("2.1.321 · /opt/homebrew/bin/claude");
    expect(items.find((i) => i.id === "forge-gitlab")!.detail).toBe("@vinay · token in Keychain");
    expect(items.find((i) => i.id === "mcp")!.detail).toBe("127.0.0.1:7811");
    expect(readiness(items)).toEqual({ ready: items.length, total: items.length, errors: 0 });
  });

  it("gives the exact fix when the CLI is missing", () => {
    const items = buildChecklist({ engine: { ...engineOk, cli_path: null, version: null, logged_in: false, ok: false }, forges: [forge], projects: [project()] });
    const cli = items.find((i) => i.id === "cli")!;
    expect(cli.level).toBe("error");
    expect(cli.command).toBe(CLAUDE_INSTALL);
    expect(cli.recheck).toBe("engine");
    const login = items.find((i) => i.id === "login")!;
    expect(login.recheck).toBe("deep");
    expect(login.command).toBe("claude");
  });

  it("flags a missing forge and keychain and points to Accounts", () => {
    const items = buildChecklist({ engine: engineOk, forges: [], projects: [] });
    expect(items.find((i) => i.id === "forge")).toMatchObject({ level: "error", pane: "accounts" });
    expect(items.find((i) => i.id === "keychain")!.level).toBe("warn");
    expect(items.find((i) => i.id === "projects")).toMatchObject({ level: "warn", paneAction: "Add projects" });
  });

  it("reports a rejected token", () => {
    const items = buildChecklist({ engine: engineOk, forges: [{ ...forge, ok: false }], projects: [project()] });
    expect(items.find((i) => i.id === "forge-gitlab")).toMatchObject({ level: "error", title: "gitlab.com token rejected", recheck: "forges" });
  });

  it("lists each missing CI tool with its Homebrew command", () => {
    const items = buildChecklist({
      engine: engineOk,
      forges: [forge],
      projects: [project()],
      tools: [
        { name: "semgrep", installed: false, version: null },
        { name: "docker", installed: false, version: null },
      ],
    });
    expect(items.find((i) => i.id === "tool-semgrep")).toMatchObject({ level: "warn", command: "brew install semgrep", fix: "Install with Homebrew:", detail: "Scan jobs are skipped." });
    const docker = items.find((i) => i.id === "tool-docker")!;
    expect(docker.command).toBeUndefined();
    expect(docker.fix).toMatch(/Docker Desktop/);
    expect(readiness(items).ready).toBe(items.length - 2);
  });

  it("treats all-paused projects as a warning", () => {
    const items = buildChecklist({ engine: engineOk, forges: [forge], projects: [project({ enabled: false })] });
    expect(items.find((i) => i.id === "projects")!.title).toBe("All projects are paused");
  });
});

describe("toolFix", () => {
  it("splits a command from trailing words", () => {
    expect(toolFix("claude")).toEqual({ fix: "Install with npm: (then run claude to log in)", command: "npm install -g @anthropic-ai/claude-code" });
    expect(toolFix("git")).toEqual({ fix: "Install:", command: "xcode-select --install" });
  });
});

const settings: AppSettings = {
  claude_cli_path: null,
  model: "opus",
  max_turns: 30,
  review_timeout_secs: 900,
  max_reviews_per_hour: 6,
  posting_paused: false,
  mcp_port: 7811,
  worktree_root: "~/.warden/worktrees",
  data_dir: "~/.warden",
};

describe("limits", () => {
  it("round-trips settings through the draft", () => {
    const d = draftFromSettings(settings);
    expect(d.timeout_min).toBe("15");
    expect(limitsDirty(settings, d)).toBe(false);
    expect(applyLimits(settings, d)).toEqual(settings);
  });

  it("validates ranges and whole numbers", () => {
    const d = { ...draftFromSettings(settings), max_turns: "2.5", max_reviews_per_hour: "0", mcp_port: "80", timeout_min: "abc" };
    const e = validateLimits(d);
    expect(e.max_turns).toMatch(/whole number/);
    expect(e.max_reviews_per_hour).toMatch(/between 1 and 120/);
    expect(e.mcp_port).toMatch(/between 1024/);
    expect(e.timeout_min).toMatch(/a number/);
    expect(validateLimits(draftFromSettings(settings))).toEqual({});
  });

  it("converts minutes to seconds and detects changes", () => {
    const d = { ...draftFromSettings(settings), timeout_min: "2.5", model: "sonnet" };
    expect(limitsDirty(settings, d)).toBe(true);
    expect(applyLimits(settings, d)).toMatchObject({ review_timeout_secs: 150, model: "sonnet" });
  });
});

describe("project helpers", () => {
  it("adds branches without blanks or duplicates", () => {
    expect(addBranches(["main"], "develop, main,  release/* ,")).toEqual(["main", "develop", "release/*"]);
  });
  it("summarises a project", () => {
    expect(projectSummary(project())).toBe("Draft · develop, main");
    expect(projectSummary(project({ enabled: false, mode: "assist", target_branches: ["main"] }))).toBe("Assist · main · paused");
  });
});
