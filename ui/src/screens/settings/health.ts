// Pure builder for the General pane's setup checklist (9A). Unit tested in health.test.ts.
import type { EngineStatus } from "@/bindings/EngineStatus";
import type { ForgeStatus } from "@/bindings/ForgeStatus";
import type { Project } from "@/bindings/Project";
import type { ToolStatus } from "@/bindings/ToolStatus";
import { installHint } from "@/lib/format";
import type { SettingsPane } from "@/lib/nav";

export type CheckLevel = "ok" | "warn" | "error";
/** What "Check again" re-runs. */
export type Recheck = "deep" | "engine" | "tools" | "forges" | "projects";

export type CheckItem = {
  id: string;
  level: CheckLevel;
  title: string;
  /** Muted one-liner: the value when ok, the consequence when not. */
  detail?: string;
  /** What to do, in words. */
  fix?: string;
  /** Exact command to copy. */
  command?: string;
  recheck?: Recheck;
  /** Pane that holds the related settings. */
  pane?: SettingsPane;
  /** Button label for the pane link (otherwise a chevron). */
  paneAction?: string;
};

export const CLAUDE_INSTALL = "npm install -g @anthropic-ai/claude-code";

/** What a missing optional tool costs. */
const TOOL_IMPACT: Record<string, string> = {
  semgrep: "Scan jobs are skipped.",
  gitleaks: "The built-in secret scanner is used instead.",
  "gitlab-ci-local": "Local GitLab CI jobs are skipped.",
  act: "Local GitHub Actions jobs are skipped.",
  docker: "CI jobs that need containers are skipped.",
  rg: "Code search falls back to slower tools.",
};

export function toolImpact(name: string): string {
  return TOOL_IMPACT[name] ?? "Jobs that need it are skipped.";
}

/** Splits an install hint into a copyable command and the words around it. */
export function toolFix(name: string): { fix: string; command?: string } {
  const hint = installHint(name);
  if (/^(brew|npm|xcode-select|pip|cargo) /.test(hint)) {
    const [command, rest] = hint.split(/, then /);
    const via = command.startsWith("brew ") ? "Install with Homebrew:" : command.startsWith("npm ") ? "Install with npm:" : "Install:";
    return { fix: rest ? `${via} (then ${rest})` : via, command };
  }
  return { fix: hint };
}

export function hostOf(url: string): string {
  return url.replace(/^https?:\/\//, "").replace(/^api\./, "").replace(/\/+$/, "");
}

export type ChecklistInput = {
  engine: EngineStatus;
  forges: ForgeStatus[];
  projects: Project[];
  tools?: ToolStatus[];
  mcpPort?: number;
};

export function buildChecklist({ engine, forges, projects, tools, mcpPort }: ChecklistInput): CheckItem[] {
  const items: CheckItem[] = [];

  // Claude Code CLI
  if (engine.cli_path && engine.version)
    items.push({ id: "cli", level: "ok", title: "Claude Code CLI", detail: `${engine.version} · ${engine.cli_path}`, pane: "claude" });
  else if (engine.cli_path)
    items.push({
      id: "cli",
      level: "error",
      title: "Claude Code CLI does not run",
      detail: `Found ${engine.cli_path} but it did not report a version.`,
      fix: "Reinstall it, or set the right path in Claude settings:",
      command: CLAUDE_INSTALL,
      recheck: "engine",
      pane: "claude",
    });
  else
    items.push({
      id: "cli",
      level: "error",
      title: "Claude Code CLI not found",
      detail: "Reviews cannot run without it.",
      fix: "Install it, then run claude once to log in:",
      command: CLAUDE_INSTALL,
      recheck: "engine",
      pane: "claude",
    });

  // Subscription login
  if (engine.logged_in)
    items.push({
      id: "login",
      level: "ok",
      title: "Logged in with your subscription",
      detail: engine.api_key_in_env ? "API key in environment is stripped for reviews" : "no API key in environment",
      pane: "claude",
    });
  else
    items.push({
      id: "login",
      level: "error",
      title: "Subscription login not verified",
      detail: "Check again sends one tiny prompt with your subscription.",
      fix: "If it fails, run claude in a terminal and log in with your Pro or Max plan:",
      command: "claude",
      recheck: "deep",
      pane: "claude",
    });

  // Forges + keychain
  const okForge = forges.find((f) => f.ok);
  if (!forges.length)
    items.push({
      id: "forge",
      level: "error",
      title: "No GitLab or GitHub account",
      detail: "Warden needs a token to read merge requests and post notes.",
      fix: "Add a project access token for your bot user in Accounts.",
      pane: "accounts",
      paneAction: "Open Accounts",
    });
  for (const f of forges) {
    const host = hostOf(f.base_url);
    if (f.ok)
      items.push({
        id: `forge-${f.forge}`,
        level: "ok",
        title: host,
        detail: `${f.user ? `@${f.user.replace(/^@/, "")} · ` : ""}token in Keychain`,
        pane: "accounts",
        paneAction: "Replace token",
      });
    else
      items.push({
        id: `forge-${f.forge}`,
        level: "error",
        title: `${host} token rejected`,
        detail: "The stored token no longer works (expired, revoked or missing scope api).",
        fix: "Create a new token and replace it in Accounts.",
        recheck: "forges",
        pane: "accounts",
        paneAction: "Replace token",
      });
  }
  if (okForge) items.push({ id: "keychain", level: "ok", title: "Keychain", detail: "tokens are read from the macOS Keychain", pane: "accounts" });
  else if (forges.length)
    items.push({
      id: "keychain",
      level: "warn",
      title: "Keychain token not usable",
      detail: "Warden could not use the token stored in the Keychain.",
      fix: "If macOS asks for Keychain access, choose Always Allow. Otherwise replace the token in Accounts.",
      recheck: "forges",
      pane: "accounts",
    });
  else
    items.push({
      id: "keychain",
      level: "warn",
      title: "No token in the Keychain yet",
      detail: "Saving a token in Accounts stores it in the Keychain.",
      pane: "accounts",
      paneAction: "Open Accounts",
    });

  // Projects
  const watched = projects.filter((p) => p.enabled);
  if (watched.length)
    items.push({
      id: "projects",
      level: "ok",
      title: `${watched.length} project${watched.length === 1 ? "" : "s"} watched`,
      detail: watched.map((p) => p.name).join(", "),
      pane: "projects",
    });
  else
    items.push({
      id: "projects",
      level: "warn",
      title: projects.length ? "All projects are paused" : "No projects watched",
      detail: projects.length ? "No merge requests are polled." : "Nothing is reviewed until you add a project.",
      fix: projects.length ? "Turn watching back on in Projects." : "Add the GitLab projects Warden should review.",
      pane: "projects",
      paneAction: projects.length ? "Open Projects" : "Add projects",
    });

  // git
  const git = tools?.find((t) => t.name === "git");
  if (git)
    items.push(
      git.installed
        ? { id: "git", level: "ok", title: "git", detail: git.version ?? "installed", pane: "tools" }
        : { id: "git", level: "error", title: "git not found", detail: "Worktrees for reviews cannot be created.", ...toolFix("git"), recheck: "tools", pane: "tools" },
    );

  // MCP
  items.push({ id: "mcp", level: "ok", title: "Warden MCP server", detail: `127.0.0.1:${mcpPort ?? 7811}`, pane: "advanced" });

  // CI tools
  const optional = (tools ?? []).filter((t) => t.name !== "git" && t.name !== "claude");
  const installed = optional.filter((t) => t.installed);
  if (installed.length)
    items.push({
      id: "ci-tools",
      level: "ok",
      title: `${installed.length} CI tool${installed.length === 1 ? "" : "s"} installed`,
      detail: installed.map((t) => (t.version ? `${t.name} ${t.version}` : t.name)).join(", "),
      pane: "tools",
    });
  for (const t of optional.filter((x) => !x.installed))
    items.push({ id: `tool-${t.name}`, level: "warn", title: `${t.name} not installed`, detail: toolImpact(t.name), ...toolFix(t.name), recheck: "tools", pane: "tools" });

  return items;
}

export function readiness(items: CheckItem[]): { ready: number; total: number; errors: number } {
  return {
    ready: items.filter((i) => i.level === "ok").length,
    total: items.length,
    errors: items.filter((i) => i.level === "error").length,
  };
}
