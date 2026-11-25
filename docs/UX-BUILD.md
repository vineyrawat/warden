# UX build contract (picks: 2A+C 3C+A 4B+A 5A+C 6A+C 7A+C 8A+B+C 9B+A)

Mockups: `docs/ux-screens.html`. Section anchors `#s2` … `#s9`. Section 1 (add projects) is already built in `ui/src/components/add-projects.tsx`.
Match the mockup layout, copy and density. Use the existing tokens/Tailwind classes, shadcn components in `ui/src/components/ui/`, IBM Plex fonts. Both themes must work (tokens only, no literal colours).

## Rules for every agent

- Edit ONLY the files you own (table below). Other agents edit other files at the same time.
- Shared files you may READ but not edit: `ui/src/lib/format.ts`, `ui/src/lib/utils.ts`, `ui/src/components/ui/*`, `ui/src/components/layout.tsx`, `ui/src/components/pills.tsx`, `ui/src/components/segmented.tsx`, `ui/src/bindings/*`. Need a helper? Put it in a file you own.
- `ui/src/lib/api.ts`, `ui/src/lib/mock.ts`, `ui/src/lib/mock-data.ts`, `ui/src/lib/queries.ts`, all Rust: owned by the backend agent only.
- When you run `npx tsc --noEmit -p .` (in `ui/`), fix errors in your files only; errors in other agents' files are theirs.
- Keep existing exported component names/props that App.tsx or other screens import (listed below) unless you own App.tsx.
- No commits. No real GitLab calls. Do not start dev servers; the main thread verifies in the browser.
- Add vitest tests for any pure logic you write (parsers, grouping, sorting) under `ui/src/lib/__tests__/` or next to your files as `*.test.ts`.
- Plain, specific UI copy. No filler.

## Ownership

| Agent | Owns |
|---|---|
| backend | `crates/**`, `src-tauri/**` (commands.rs, lib.rs, capabilities, tauri.conf.json), `ui/src/lib/api.ts`, `ui/src/lib/mock.ts`, `ui/src/lib/mock-data.ts`, `ui/src/lib/queries.ts`, `ui/src/bindings/**`, `ui/src/lib/__tests__/mock.test.ts` |
| shell (2 + 9) | `ui/src/App.tsx`, `ui/src/main.tsx`, `ui/src/lib/nav.tsx`, `ui/src/components/shell.tsx`, new `ui/src/components/command-palette.tsx`, new `ui/src/screens/Settings.tsx` (+ `ui/src/screens/settings/*`), `ui/src/screens/Connections.tsx` |
| inbox (3) | `ui/src/screens/Inbox.tsx`, new `ui/src/components/inbox/*` |
| review (4) | `ui/src/screens/Review.tsx`, `ui/src/components/finding-card.tsx`, `ui/src/components/diff-view.tsx`, new `ui/src/components/review/*` |
| ci (6) | `ui/src/screens/Ci.tsx`, new `ui/src/components/ci/*`, new `ui/src/lib/ci-parse.ts` |
| queue (5) | `ui/src/screens/ApprovalQueue.tsx`, new `ui/src/components/queue/*` |
| history (7) | `ui/src/screens/History.tsx`, new `ui/src/components/history/*` |
| guardrails (8) | `ui/src/screens/Guardrails.tsx`, new `ui/src/components/guardrails/*` |

## Navigation contract (shell owns nav.tsx; everyone relies on this)

```ts
type Route =
  | { screen: "inbox"; mrId?: number }               // mrId = preselect in the list+preview
  | { screen: "review"; runId: number; findingId?: number; tab?: "findings" | "diff" | "ci" | "summary" }
  | { screen: "queue" }
  | { screen: "ci"; runId?: number; jobId?: number }
  | { screen: "history"; runId?: number }
  | { screen: "settings"; pane?: "general" | "accounts" | "claude" | "projects" | "tools" | "advanced"; projectId?: number }
  // legacy, kept so old links work; App maps them to settings:
  | { screen: "guardrails"; projectId?: number }      // → settings/projects/projectId (Guardrails tab)
  | { screen: "projects" }                            // → settings/projects
  | { screen: "connections" };                        // → settings/general
```

Sidebar work items: Inbox, Approval queue, CI runs, History. One "Settings" entry (⌘,). A live card at the sidebar bottom (review in progress: MR, turn n / max, current tool; reviews this hour x / y; next poll). ⌘K opens the command palette (jump to MR, start/re-run review, post approved notes, pause posting, open a project's guardrails, open settings panes).

Settings (9B + 9A): in the Tauri app it is a separate window (backend provides `open_settings_window(pane)`); the window loads `index.html?view=settings&pane=...` and main.tsx renders `<SettingsApp/>` for that query. In the browser mock (no Tauri) the shell renders Settings as an in-app route. Panes: General (setup/health checklist from 9A: Claude CLI, subscription login, forge, keychain, tools; green rows collapse; missing items show the exact fix + copy button + "Check again"), Accounts (forges + tokens), Claude (engine, model, max turns, reviews per hour, timeout, MCP port: the app-wide limits that used to appear on Guardrails), Projects (project list on the left; selecting one shows its page with tabs: Overview (repo path, branches, mode, poll, enabled) and Guardrails = `<GuardrailsPanel projectId={id} />` exported by `screens/Guardrails.tsx`; "Add projects" opens `AddProjectsDialog`), Tools (CI tool detection), Advanced (data folder, worktree root, posting paused, MCP).

## Component contract between agents

- `screens/Guardrails.tsx` (guardrails agent) exports `GuardrailsPanel({ projectId }: { projectId: number })` = the per-project guardrails UI (tabs Posting / Filters / Rules + "Always on" card + live preview), no screen header, no project picker. Keep `GuardrailsScreen` exported too (thin wrapper with a project picker) until shell removes the route.
- `screens/Review.tsx` keeps `ReviewScreen({ runId, focusFindingId, tab? })`.
- `screens/Ci.tsx` keeps `CiScreen({ initialRunId, initialJobId })`.
- `screens/History.tsx` keeps `HistoryScreen({ initialRunId })`.
- `screens/Inbox.tsx` keeps `InboxScreen({ initialMrId? })`.
- `screens/ApprovalQueue.tsx` keeps `ApprovalQueueScreen()`.

## New backend API (backend agent adds to Rust + api.ts + mock.ts + queries.ts hooks)

| Command (api.ts name) | Returns | For |
|---|---|---|
| `get_stats(days)` | `Stats { reviews, notes_posted, resolved, resolved_rate, median_run_ms, per_day: [{ date, reviews, notes }] }` | History KPI strip + 14-day chart (7C) |
| `rule_stats(projectId \| null, days)` | `RuleStat[] { rule_id, title, hits, posted, dismissed, rejected, last_hit_at }` | Guardrails rules catalogue (8B) |
| `simulate_guardrails(projectId, overrides, limitRuns)` | `SimulatedFinding[] { finding, mr_label, outcome: "post" \| "draft" \| "drop", reason }` | Guardrails live preview (8C) |
| `preview_note(findingId)` | `string` (markdown exactly as Warden would post it) | Queue note preview (5C) |
| `list_pending_findings()` | `PendingFinding[] { finding, run_id, mr_id, mr_iid, mr_title, project_name, web_url }` | Queue one list (5A) |
| `post_findings_bulk(findingIds)` | `PostReport[]` (grouped per run internally) | Queue bulk post (5A) |
| `open_settings_window(pane \| null)` | `void` (Tauri: create/focus window `settings`) | Settings window (9B) |

Hooks in queries.ts: `useStats(days)`, `useRuleStats(projectId, days)`, `useSimulateGuardrails(projectId, overrides, limitRuns)` (query, keyed on overrides JSON), `usePreviewNote(findingId)`, `usePendingFindings()`, `usePostFindingsBulk()`.

Mock data must make every new screen look realistic (several runs over 14 days, posted/resolved findings, a noisy FR-007 rule dismissed ~55% of the time).
