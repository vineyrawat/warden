// Projects pane: project list on the left, the selected project's page on the right with tabs
// Overview (repo, branches, mode, poll, watching) and Guardrails (GuardrailsPanel from the
// guardrails agent). "Add projects" opens the shared AddProjectsDialog.
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { FolderOpen, Plus, X } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { EmptyState, Loading } from "@/components/layout";
import { Segmented } from "@/components/segmented";
import { AddProjectsDialog } from "@/components/add-projects";
import { GuardrailsPanel } from "@/screens/Guardrails";
import type { PostingMode } from "@/bindings/PostingMode";
import type { Project } from "@/bindings/Project";
import * as api from "@/lib/api";
import { POSTING_MODES, modeDescription, modeLabel, pollLabel } from "@/lib/format";
import { useNav, type ProjectTab } from "@/lib/nav";
import { errorMessage, qk, useProjects } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { hostOf } from "./health";
import { addBranches, projectSummary } from "./project-helpers";
import { Kv, Pane } from "./shared";

const POLL_OPTIONS = [60, 120, 300, 600, 900, 1800, 3600];

function useSaveProject() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (p: Project) => api.save_project(p),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.projects }),
    onError: (e) => toast.error(errorMessage(e)),
  });
}

export function ProjectsPane({ projectId, tab }: { projectId?: number; tab?: ProjectTab }) {
  const { navigate } = useNav();
  const projects = useProjects();
  const [adding, setAdding] = useState(false);
  const list = projects.data ?? [];
  const selected = list.find((p) => p.id === projectId) ?? list[0];
  const select = (id: number, t?: ProjectTab) => navigate({ screen: "settings", pane: "projects", projectId: id, tab: t });

  return (
    <div className="grid h-full min-h-0 grid-cols-[210px_minmax(0,1fr)]">
      <div className="flex min-h-0 flex-col border-r bg-card">
        <ul className="min-h-0 flex-1 overflow-y-auto" aria-label="Projects">
          {projects.isLoading && <Loading />}
          {list.map((p) => {
            const on = p.id === selected?.id;
            return (
              <li key={p.id}>
                <button
                  type="button"
                  aria-current={on ? "true" : undefined}
                  onClick={() => select(p.id, tab)}
                  className={cn("grid w-full gap-0.5 border-b px-3 py-2.5 text-left text-[12.5px]", on ? "bg-surface-2" : "hover:bg-surface-2/60")}
                >
                  <b className={cn("truncate", !p.enabled && "text-muted-foreground")}>{p.name}</b>
                  <span className="truncate font-mono text-2xs text-muted-foreground">{projectSummary(p)}</span>
                </button>
              </li>
            );
          })}
        </ul>
        <div className="border-t p-2.5">
          <Button size="sm" onClick={() => setAdding(true)}>
            <Plus /> Add projects
          </Button>
        </div>
      </div>
      <div className="min-h-0 overflow-y-auto">
        {selected ? (
          <ProjectPage key={selected.id} project={selected} tab={tab ?? "overview"} onTab={(t) => select(selected.id, t)} />
        ) : (
          !projects.isLoading && (
            <EmptyState title="No projects yet">
              Add the GitLab projects Warden should review. It finds your local clones for you.
              <div className="mt-3">
                <Button variant="default" onClick={() => setAdding(true)}>
                  <Plus /> Add projects
                </Button>
              </div>
            </EmptyState>
          )
        )}
      </div>
      <AddProjectsDialog open={adding} onOpenChange={setAdding} />
    </div>
  );
}

function ProjectPage({ project: p, tab, onTab }: { project: Project; tab: ProjectTab; onTab: (t: ProjectTab) => void }) {
  const save = useSaveProject();
  return (
    <div className="grid content-start gap-3 px-5 py-4">
      <div className="flex items-center gap-2.5">
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">{p.name}</h2>
        <Switch
          id={`watch-${p.id}`}
          checked={p.enabled}
          onCheckedChange={(v) => save.mutate({ ...p, enabled: v }, { onSuccess: () => toast.success(v ? `Watching ${p.name}` : `Paused ${p.name}`) })}
        />
        <label htmlFor={`watch-${p.id}`} className="text-xs text-muted-foreground">
          {p.enabled ? "watching" : "paused"}
        </label>
      </div>
      <Tabs value={tab} onValueChange={(v) => onTab(v as ProjectTab)}>
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="guardrails">Guardrails</TabsTrigger>
        </TabsList>
        <TabsContent value="overview" className="mt-3">
          <Overview project={p} onGuardrails={() => onTab("guardrails")} />
        </TabsContent>
        <TabsContent value="guardrails" className="mt-3">
          <GuardrailsPanel projectId={p.id} />
        </TabsContent>
      </Tabs>
    </div>
  );
}

function Overview({ project: p, onGuardrails }: { project: Project; onGuardrails: () => void }) {
  const qc = useQueryClient();
  const { navigate } = useNav();
  const save = useSaveProject();
  const remove = useMutation({
    mutationFn: () => api.delete_project(p.id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: qk.projects });
      qc.invalidateQueries({ queryKey: ["mrs"] });
      toast.success(`Stopped watching ${p.remote_id}`);
      navigate({ screen: "settings", pane: "projects" });
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  const pickFolder = async () => {
    try {
      const path = await api.pick_folder();
      if (path) save.mutate({ ...p, local_path: path }, { onSuccess: () => toast.success("Local clone updated") });
    } catch (e) {
      toast.error(errorMessage(e));
    }
  };
  const forgeName = p.forge === "github" ? "GitHub" : "GitLab";
  const polls = POLL_OPTIONS.includes(p.poll_interval_secs) ? POLL_OPTIONS : [...POLL_OPTIONS, p.poll_interval_secs].sort((a, b) => a - b);

  return (
    <div className="grid gap-3">
      <Pane>
        <Kv label={`${forgeName} project`} hint={hostOf(p.base_url)}>
          <span className="font-mono text-xs">{p.remote_id}</span>
        </Kv>
        <Kv label="Code on this Mac">
          <span className="max-w-[300px] truncate font-mono text-2xs text-muted-foreground" title={p.local_path}>
            {p.local_path}
          </span>
          <Button size="sm" variant="ghost" onClick={pickFolder}>
            <FolderOpen /> Change
          </Button>
        </Kv>
        <Kv label="Reviews MRs into" hint="Target branches, globs allowed (release/*)">
          <BranchChips branches={p.target_branches} onChange={(b) => save.mutate({ ...p, target_branches: b })} />
        </Kv>
        <Kv label="Posting mode" hint={modeDescription(p.mode)}>
          <Segmented<PostingMode>
            label="Posting mode"
            options={POSTING_MODES.map((m) => ({ value: m, label: modeLabel(m) }))}
            value={p.mode}
            onChange={(m) => save.mutate({ ...p, mode: m })}
          />
        </Kv>
        <Kv label="Poll every" htmlFor={`poll-${p.id}`}>
          <Select value={String(p.poll_interval_secs)} onValueChange={(v) => save.mutate({ ...p, poll_interval_secs: Number(v) })}>
            <SelectTrigger id={`poll-${p.id}`} className="w-[100px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {polls.map((s) => (
                <SelectItem key={s} value={String(s)}>
                  {pollLabel(s)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Kv>
        <Kv label="Guardrails" hint="Confidence, severity, rules and posting limits">
          <Button size="sm" variant="ghost" onClick={onGuardrails}>
            Edit →
          </Button>
        </Kv>
      </Pane>
      <div>
        <Button
          variant="ghost"
          className="text-red hover:bg-red-soft hover:text-red"
          disabled={remove.isPending}
          onClick={() => {
            if (window.confirm(`Stop watching ${p.remote_id}? Review history stays in the database.`)) remove.mutate();
          }}
        >
          Stop watching
        </Button>
      </div>
    </div>
  );
}

function BranchChips({ branches, onChange }: { branches: string[]; onChange: (b: string[]) => void }) {
  const [adding, setAdding] = useState(false);
  const [value, setValue] = useState("");
  const commit = () => {
    const next = addBranches(branches, value);
    if (next.length !== branches.length) onChange(next);
    setValue("");
    setAdding(false);
  };
  return (
    <span className="flex flex-wrap items-center justify-end gap-1">
      {branches.map((b) => (
        <span key={b} className="inline-flex items-center gap-0.5 rounded-full border bg-surface-2 py-px pl-2 pr-0.5 font-mono text-2xs">
          {b}
          <button
            type="button"
            aria-label={`Stop reviewing MRs into ${b}`}
            disabled={branches.length <= 1}
            title={branches.length <= 1 ? "Keep at least one branch" : undefined}
            className="rounded-full p-0.5 text-muted-foreground hover:bg-card hover:text-foreground disabled:opacity-40"
            onClick={() => onChange(branches.filter((x) => x !== b))}
          >
            <X className="h-2.5 w-2.5" />
          </button>
        </span>
      ))}
      {adding ? (
        <Input
          autoFocus
          aria-label="Add target branch"
          placeholder="main, release/*"
          className="h-6 w-[130px]"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commit();
            } else if (e.key === "Escape") {
              e.stopPropagation();
              setValue("");
              setAdding(false);
            }
          }}
        />
      ) : (
        <button
          type="button"
          aria-label="Add target branch"
          className="inline-flex h-[20px] w-[24px] items-center justify-center rounded-full border border-dashed text-muted-foreground hover:text-foreground"
          onClick={() => setAdding(true)}
        >
          <Plus className="h-3 w-3" />
        </button>
      )}
    </span>
  );
}
