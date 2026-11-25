// Guardrails (8A + 8B + 8C): per-project tabs Posting / Filters / Rules, a read-only "Always on"
// card, a live preview beside Posting and Filters, and a sticky save bar listing what changed.
// App-wide limits (max turns, reviews per hour, timeout) live in Settings → Claude.
import { useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Save } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Tip } from "@/components/ui/tooltip";
import { EmptyState, ErrorBox, Loading, ScreenHeader } from "@/components/layout";
import { AlwaysOnCard } from "@/components/guardrails/bits";
import { FiltersTab } from "@/components/guardrails/filters-tab";
import { LivePreview } from "@/components/guardrails/live-preview";
import { changeText, diffDraft, draftFrom, overridesFromDraft, patchOverrides, type Change, type Draft } from "@/components/guardrails/logic";
import { PostingTab } from "@/components/guardrails/posting-tab";
import { RulesTab, STATS_DAYS } from "@/components/guardrails/rules-tab";
import type { Project } from "@/bindings/Project";
import * as api from "@/lib/api";
import type { MergedConfig } from "@/lib/api";
import { errorMessage, qk, useProjectConfig, useProjects, useRuleStats, useSettings, useSimulateGuardrails } from "@/lib/queries";
import { cn } from "@/lib/utils";

export { overridesFromDraft } from "@/components/guardrails/logic";

type Tab = "posting" | "filters" | "rules";

const POSTING_KEYS = ["mode", "autoSeverities", "autoPostConfidence", "maxNotes", "maxNotesPerHour", "summaryNote", "mrStatusMode", "requestChangesOn"];
const FILTER_KEYS = ["minConfidence", "severityFloor", "nits", "focus", "ignorePaths", "extraInstructions"];

/** Thin wrapper kept for the legacy `guardrails` route: header + project picker + panel. */
export function GuardrailsScreen({ initialProjectId }: { initialProjectId?: number }) {
  const projects = useProjects();
  const [projectId, setProjectId] = useState<number | null>(initialProjectId ?? null);
  const pid = projectId ?? projects.data?.[0]?.id ?? null;
  const project = projects.data?.find((p) => p.id === pid);

  if (projects.isLoading) return <Loading />;
  if (projects.error) return <ErrorBox error={projects.error} />;
  if (!project)
    return (
      <div className="flex h-full flex-col">
        <ScreenHeader title="Guardrails" />
        <EmptyState title="No watched projects">Add a project under Settings → Projects to configure its guardrails.</EmptyState>
      </div>
    );

  return (
    <div className="flex h-full min-h-0 flex-col">
      <ScreenHeader
        title="Guardrails"
        actions={
          <Select value={String(project.id)} onValueChange={(v) => setProjectId(Number(v))}>
            <SelectTrigger className="h-7 w-44 bg-card font-sans" aria-label="Project">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {projects.data!.map((p) => (
                <SelectItem key={p.id} value={String(p.id)}>
                  {p.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        }
      />
      <div className="min-h-0 flex-1 overflow-y-auto px-[18px] pt-1">
        <GuardrailsPanel projectId={project.id} />
      </div>
    </div>
  );
}

/** Per-project guardrails UI (no header, no project picker); used by Settings → Projects. */
export function GuardrailsPanel({ projectId }: { projectId: number }) {
  const projects = useProjects();
  const config = useProjectConfig(projectId);
  const project = projects.data?.find((p) => p.id === projectId);

  if (projects.error) return <ErrorBox error={projects.error} />;
  if (config.error) return <ErrorBox error={config.error} />;
  if (projects.isLoading || config.isLoading) return <Loading />;
  if (!project) return <EmptyState title="Project not found">This project is no longer watched.</EmptyState>;
  if (!config.data) return <Loading />;
  return <PanelBody key={project.id} project={project} cfg={config.data} />;
}

function PanelBody({ project, cfg }: { project: Project; cfg: MergedConfig }) {
  const qc = useQueryClient();
  const settings = useSettings();
  const ruleStats = useRuleStats(project.id, STATS_DAYS);
  const [tab, setTab] = useState<Tab>("posting");
  // null = no local edits; the panel then follows the saved config (e.g. after a mode change elsewhere).
  const [edits, setEdits] = useState<Draft | null>(null);
  const [saving, setSaving] = useState(false);

  const saved = useMemo(() => draftFrom(cfg, project.mode), [cfg, project.mode]);
  const draft = edits ?? saved;
  const changes = useMemo(() => diffDraft(saved, draft), [saved, draft]);
  const dirty = changes.length > 0;
  const overrides = useMemo(() => overridesFromDraft(draft, cfg), [draft, cfg]);
  const savedOverrides = useMemo(() => overridesFromDraft(saved, cfg), [saved, cfg]);
  // Shares its cache with the preview's "saved" query; the Rules tab uses it for path advice.
  const recent = useSimulateGuardrails(project.id, savedOverrides, 10);

  const set = <K extends keyof Draft>(k: K, v: Draft[K]) => setEdits((prev) => ({ ...(prev ?? saved), [k]: v }));
  const toggleRule = (id: string, on: boolean) => {
    const cur = draft.disabled;
    set("disabled", (on ? cur.filter((x) => x !== id) : [...cur, id]).sort());
  };

  const save = async () => {
    setSaving(true);
    try {
      // Write only what changed on top of the stored overrides (see patchOverrides).
      const stored = await api.get_project_overrides(project.id);
      await api.save_project_overrides(project.id, patchOverrides(stored, draft, changes, cfg));
      if (draft.mode !== project.mode) await api.save_project({ ...project, mode: draft.mode });
      await Promise.all([qc.invalidateQueries({ queryKey: qk.projectConfig(project.id) }), qc.invalidateQueries({ queryKey: qk.projects })]);
      setEdits(null);
      toast.success(`Guardrails saved for ${project.name}`);
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const branch = project.target_branches[0] ?? "the target branch";
  const activeRules = cfg.rules.filter((r) => !draft.disabled.includes(r.id)).length;
  const hasChanges = (keys: string[] | "rules") => changes.some((c) => (keys === "rules" ? c.key.startsWith("rule:") : keys.includes(c.key)));
  const preview = <LivePreview projectId={project.id} overrides={overrides} savedOverrides={savedOverrides} dirty={dirty} />;

  const trigger = (value: Tab, label: string, keys: string[] | "rules", extra?: React.ReactNode) => (
    <TabsTrigger
      value={value}
      className="-mb-px gap-1.5 rounded-none border-b-2 border-transparent px-3 py-2 text-[12.5px] font-normal text-muted-foreground hover:text-foreground data-[state=active]:border-primary data-[state=active]:bg-transparent data-[state=active]:font-semibold data-[state=active]:shadow-none"
    >
      {label}
      {extra}
      {hasChanges(keys) && <span title="Unsaved changes" className="inline-block h-1.5 w-1.5 rounded-full bg-orange" />}
    </TabsTrigger>
  );

  const withPreview = (controls: React.ReactNode) => (
    // Container queries: the panel lives in the main window and in the 900px Settings window, so
    // its own width (not the window's) decides whether the preview sits beside the controls.
    <div className="grid items-start gap-3.5 @4xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
      <div className="@container grid min-w-0 gap-3.5">
        {controls}
        <AlwaysOnCard />
      </div>
      <div className="min-w-0 @4xl:sticky @4xl:top-2">{preview}</div>
    </div>
  );

  return (
    <Tabs value={tab} onValueChange={(v) => setTab(v as Tab)} className="@container flex min-w-0 flex-col">
      <div className="flex flex-wrap items-end gap-x-3 border-b">
        <TabsList className="h-auto justify-start gap-0.5 rounded-none bg-transparent p-0" aria-label="Guardrails sections">
          {trigger("posting", "Posting", POSTING_KEYS)}
          {trigger("filters", "Filters", FILTER_KEYS)}
          {trigger("rules", "Rules", "rules", <small className="rounded-full bg-surface-2 px-1.5 font-mono text-[10.5px] font-medium text-muted-foreground">{activeRules}</small>)}
        </TabsList>
        <span className="ml-auto pb-2 text-2xs text-muted-foreground">Source: .warden.yml on {branch} + local overrides</span>
      </div>

      {/* Only the active tab is mounted, so one preview query runs at a time. */}
      <TabsContent value="posting" className="mt-3.5">
        {withPreview(<PostingTab draft={draft} set={set} paused={!!settings.data?.posting_paused} />)}
      </TabsContent>
      <TabsContent value="filters" className="mt-3.5">
        {withPreview(<FiltersTab draft={draft} set={set} />)}
      </TabsContent>
      <TabsContent value="rules" className="mt-3.5">
        <div className="grid gap-3.5">
          <RulesTab projectId={project.id} rules={cfg.rules} disabled={draft.disabled} onToggle={toggleRule} stats={ruleStats.data} statsLoading={ruleStats.isLoading} recent={recent.data} branch={branch} />
          <AlwaysOnCard />
        </div>
      </TabsContent>

      <SaveBar changes={changes} saving={saving} onDiscard={() => setEdits(null)} onSave={save} />
    </Tabs>
  );
}

function SaveBar({ changes, saving, onDiscard, onSave }: { changes: Change[]; saving: boolean; onDiscard: () => void; onSave: () => void }) {
  const dirty = changes.length > 0;
  const head = changes.slice(0, 2).map(changeText);
  const rest = changes.length - head.length;
  return (
    <div role="region" aria-label="Save guardrails" className={cn("sticky bottom-0 z-10 mt-3.5 flex items-center gap-2.5 border-t bg-surface py-2.5 text-xs", dirty && "border-t-orange/50")}>
      {dirty ? (
        <span className="flex min-w-0 flex-1 items-center gap-2">
          <span aria-hidden className="inline-block h-[7px] w-[7px] shrink-0 rounded-full bg-orange" />
          <span className="shrink-0 font-medium">
            {changes.length} unsaved change{changes.length === 1 ? "" : "s"}
          </span>
          <span className="min-w-0 truncate text-muted-foreground" title={changes.map(changeText).join("\n")}>
            {head.join(" · ")}
          </span>
          {rest > 0 && (
            <Tip
              content={
                <div className="grid gap-0.5">
                  {changes.slice(2).map((c) => (
                    <span key={c.key}>{changeText(c)}</span>
                  ))}
                </div>
              }
            >
              <span tabIndex={0} className="shrink-0 rounded text-muted-foreground underline decoration-dotted underline-offset-2">
                +{rest} more
              </span>
            </Tip>
          )}
        </span>
      ) : (
        <span className="flex-1 text-muted-foreground">All changes saved</span>
      )}
      <Button disabled={!dirty || saving} onClick={onDiscard}>
        Discard
      </Button>
      <Button variant="default" disabled={!dirty || saving} onClick={onSave}>
        <Save /> {saving ? "Saving…" : "Save changes"}
      </Button>
    </div>
  );
}
