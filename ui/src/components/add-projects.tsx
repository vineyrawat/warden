// Add-projects dialog (UX section 1, variant A + C): the projects you can access load at once,
// matched to clones found on this Mac; tick several and watch them. A folder scan finds more
// clones, and a pasted MR link adds its project and starts a review.
import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { FolderSearch, Link2, Loader2, Search } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { DiscoveredProject } from "@/bindings/DiscoveredProject";
import type { ForgeStatus } from "@/bindings/ForgeStatus";
import type { PostingMode } from "@/bindings/PostingMode";
import * as api from "@/lib/api";
import { POSTING_MODES, formatRelative, modeLabel, pollLabel } from "@/lib/format";
import { useNav } from "@/lib/nav";
import { errorMessage, qk, useForges } from "@/lib/queries";
import { cn } from "@/lib/utils";

const POLL_OPTIONS = [60, 120, 300, 600, 900];
/** Value of the per-row source select meaning "Warden keeps its own copy". */
const MANAGED = "__managed__";
const ROOTS_KEY = "warden.scanRoots";

function loadRoots(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(ROOTS_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
  } catch {
    return [];
  }
}
function saveRoots(roots: string[]) {
  try {
    localStorage.setItem(ROOTS_KEY, JSON.stringify(roots));
  } catch {
    /* storage unavailable: the scan just uses default folders next time */
  }
}

const hostOf = (url: string) => url.replace(/^https?:\/\//, "").replace(/\/+$/, "");

export function AddProjectsDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (o: boolean) => void }) {
  const forges = (useForges().data ?? []).filter((f) => f.ok);
  const [forgeKey, setForgeKey] = useState<string | null>(null);
  const forge: ForgeStatus | undefined = forges.find((f) => `${f.forge}|${f.base_url}` === forgeKey) ?? forges[0];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-3xl gap-3">
        <DialogHeader>
          <DialogTitle>Add projects</DialogTitle>
          <DialogDescription>
            Tick the projects Warden should review. It uses your existing clone when it finds one, and never touches your checkout.
          </DialogDescription>
        </DialogHeader>
        {!forge ? (
          <NoForge onClose={() => onOpenChange(false)} />
        ) : (
          <>
            {forges.length > 1 && (
              <Select value={`${forge.forge}|${forge.base_url}`} onValueChange={setForgeKey}>
                <SelectTrigger aria-label="Forge" className="h-8 w-fit min-w-[220px] font-sans text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {forges.map((f) => (
                    <SelectItem key={`${f.forge}|${f.base_url}`} value={`${f.forge}|${f.base_url}`}>
                      {hostOf(f.base_url)} · @{f.user}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
            {open && <Picker key={`${forge.forge}|${forge.base_url}`} forge={forge} onDone={() => onOpenChange(false)} />}
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

function NoForge({ onClose }: { onClose: () => void }) {
  return (
    <div className="rounded-md bg-orange-soft px-3 py-2.5 text-xs text-orange">
      Connect GitLab or GitHub first: paste a token under Forges on this page, then come back here.
      <div className="mt-2">
        <Button size="sm" onClick={onClose}>
          Close
        </Button>
      </div>
    </div>
  );
}

function Picker({ forge, onDone }: { forge: ForgeStatus; onDone: () => void }) {
  const qc = useQueryClient();
  const { navigate } = useNav();
  const [roots, setRoots] = useState<string[]>(loadRoots);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<Record<string, boolean>>({});
  /** remote_id → chosen local path, or MANAGED. Unset = first local path (or MANAGED). */
  const [source, setSource] = useState<Record<string, string>>({});
  const [mode, setMode] = useState<PostingMode>("draft");
  const [poll, setPoll] = useState(300);
  const [link, setLink] = useState("");

  const discovery = useQuery({
    queryKey: ["discover", forge.forge, forge.base_url, roots],
    queryFn: () => api.discover_projects(forge.forge, forge.base_url, roots),
    staleTime: 60_000,
  });

  // A folder pick re-scans; keep earlier ticks.
  useEffect(() => saveRoots(roots), [roots]);

  const projects = useMemo(() => {
    const q = query.trim().toLowerCase();
    // Already-watched projects sink to the bottom; the backend's order (clones first) stays otherwise.
    const list = [...(discovery.data?.projects ?? [])].sort((a, b) => Number(a.existing_project_id != null) - Number(b.existing_project_id != null));
    return q ? list.filter((p) => p.remote.remote_id.toLowerCase().includes(q) || p.remote.name.toLowerCase().includes(q)) : list;
  }, [discovery.data, query]);

  const local = discovery.data?.local_repos ?? [];
  const otherRepos = local.filter((r) => !r.remote_id || r.duplicate_of);
  const chosen = (discovery.data?.projects ?? []).filter((p) => selected[p.remote.remote_id] && p.existing_project_id == null);
  const sourceOf = (p: DiscoveredProject) => source[p.remote.remote_id] ?? p.local_paths[0] ?? MANAGED;
  const clones = chosen.filter((p) => sourceOf(p) === MANAGED).length;

  const scan = async () => {
    try {
      const folder = await api.pick_folder();
      if (folder) setRoots((r) => (r.includes(folder) ? r : [folder, ...r].slice(0, 5)));
    } catch (e) {
      toast.error(errorMessage(e));
    }
  };

  const add = useMutation({
    mutationFn: () =>
      api.add_projects(
        forge.forge,
        forge.base_url,
        chosen.map((p) => {
          const src = sourceOf(p);
          return { remote_id: p.remote.remote_id, name: p.remote.name, local_path: src === MANAGED ? null : src, target_branches: null };
        }),
        mode,
        poll,
      ),
    onSuccess: (results) => {
      qc.invalidateQueries({ queryKey: qk.projects });
      qc.invalidateQueries({ queryKey: ["mrs"] });
      qc.invalidateQueries({ queryKey: ["discover"] });
      const ok = results.filter((r) => r.project_id != null);
      const failed = results.filter((r) => r.error);
      if (ok.length) toast.success(ok.length === 1 ? `Watching ${ok[0].remote_id}` : `Watching ${ok.length} projects`);
      failed.forEach((f) => toast.error(`${f.remote_id}: ${f.error}`));
      if (!failed.length) onDone();
      else setSelected(Object.fromEntries(failed.map((f) => [f.remote_id, true])));
    },
    onError: (e) => toast.error(errorMessage(e)),
  });

  const openLink = useMutation({
    mutationFn: () => api.open_mr_link(link.trim()),
    onSuccess: (r) => {
      qc.invalidateQueries({ queryKey: qk.projects });
      qc.invalidateQueries({ queryKey: ["mrs"] });
      toast.success(r.added_project ? "Project added. Review started." : "Review started.");
      onDone();
      navigate({ screen: "review", runId: r.run_id });
    },
    onError: (e) => toast.error(errorMessage(e)),
  });

  const toggle = (id: string, on: boolean) => setSelected((s) => ({ ...s, [id]: on }));
  const selectable = projects.filter((p) => p.existing_project_id == null);
  const allLocalOn = selectable.filter((p) => p.local_paths.length).every((p) => selected[p.remote.remote_id]);

  return (
    <div className="grid gap-3">
      {/* scan + paste */}
      <div className="grid grid-cols-[1fr_auto] items-center gap-3 rounded-md border border-dashed px-3 py-2.5">
        <div className="min-w-0 text-xs">
          <div className="truncate font-medium">
            {discovery.isFetching ? "Looking for clones on this Mac…" : `Code found in ${(discovery.data?.scanned_roots ?? []).map((r) => r.replace(/^\/Users\/[^/]+/, "~")).join(", ") || "default folders"}`}
          </div>
          <div className="text-2xs text-muted-foreground">
            {discovery.data
              ? `${discovery.data.scanned_dirs.toLocaleString()} folders scanned · ${local.length} git repos · ${local.filter((r) => r.remote_id && !r.duplicate_of).length} on ${hostOf(forge.base_url)}`
              : "Matching git remotes to your projects"}
          </div>
        </div>
        <Button size="sm" onClick={scan}>
          <FolderSearch /> Scan a folder…
        </Button>
      </div>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (link.trim()) openLink.mutate();
        }}
      >
        <div className="relative flex-1">
          <Link2 className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            aria-label="Merge request link"
            className="pl-8 font-sans"
            placeholder={`Or paste an MR link, e.g. ${hostOf(forge.base_url)}/group/project/-/merge_requests/41`}
            value={link}
            onChange={(e) => setLink(e.target.value)}
          />
        </div>
        {link.trim() && (
          <Button type="submit" variant="default" disabled={openLink.isPending}>
            {openLink.isPending ? <Loader2 className="animate-spin" /> : null} Review this MR
          </Button>
        )}
      </form>

      {/* search + list */}
      <div className="relative">
        <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          autoFocus
          aria-label="Filter projects"
          className="pl-8 font-sans"
          placeholder={discovery.data ? `Filter ${discovery.data.projects.length} projects on ${hostOf(forge.base_url)}` : "Loading projects…"}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>

      <div className="overflow-hidden rounded-md border">
        <div className="grid grid-cols-[20px_minmax(0,1.1fr)_minmax(0,1.7fr)_minmax(0,0.6fr)] items-center gap-3 border-b bg-secondary px-3 py-1.5 text-2xs font-medium text-muted-foreground">
          <Checkbox
            aria-label="Select all projects with a local clone"
            checked={selectable.some((p) => p.local_paths.length) && allLocalOn}
            onCheckedChange={(v) => selectable.filter((p) => p.local_paths.length).forEach((p) => toggle(p.remote.remote_id, !!v))}
          />
          <span>Project</span>
          <span>Code on this Mac</span>
          <span>Default branch</span>
        </div>
        <div className="max-h-[300px] overflow-y-auto" role="list" aria-label="Projects">
          {discovery.isLoading && <div className="px-3 py-6 text-center text-xs text-muted-foreground">Loading your projects and scanning folders…</div>}
          {discovery.isError && <div className="px-3 py-4 text-xs text-red">{errorMessage(discovery.error)}</div>}
          {discovery.data && projects.length === 0 && <div className="px-3 py-4 text-xs text-muted-foreground">No project matches “{query}”.</div>}
          {projects.map((p) => (
            <ProjectRow
              key={p.remote.remote_id}
              p={p}
              checked={!!selected[p.remote.remote_id]}
              onCheck={(v) => toggle(p.remote.remote_id, v)}
              source={sourceOf(p)}
              onSource={(v) => setSource((s) => ({ ...s, [p.remote.remote_id]: v }))}
            />
          ))}
        </div>
      </div>
      {otherRepos.length > 0 && (
        <details className="text-2xs text-muted-foreground">
          <summary className="cursor-pointer select-none">
            {otherRepos.length} other repo{otherRepos.length === 1 ? "" : "s"} found and skipped (duplicates or not on {hostOf(forge.base_url)})
          </summary>
          <ul className="mt-1 grid gap-0.5 pl-4">
            {otherRepos.map((r) => (
              <li key={r.path} className="font-mono">
                {r.path} <span className="font-sans">· {r.duplicate_of ? `same project as ${r.duplicate_of}` : r.remote_url || "no remote"}</span>
              </li>
            ))}
          </ul>
        </details>
      )}

      {/* footer */}
      <div className="flex flex-wrap items-center gap-2 border-t pt-3 text-xs">
        <span className="text-muted-foreground">Start in</span>
        <Select value={mode} onValueChange={(v) => setMode(v as PostingMode)}>
          <SelectTrigger aria-label="Posting mode" className="h-7 w-[92px] font-sans text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {POSTING_MODES.map((m) => (
              <SelectItem key={m} value={m}>
                {modeLabel(m)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <span className="text-muted-foreground">mode, checking</span>
        <Select value={String(poll)} onValueChange={(v) => setPoll(Number(v))}>
          <SelectTrigger aria-label="Poll interval" className="h-7 w-[120px] font-sans text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {POLL_OPTIONS.map((s) => (
              <SelectItem key={s} value={String(s)}>
                {pollLabel(s)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <span className="flex-1 text-2xs text-muted-foreground">
          {mode === "draft" ? "Warden suggests notes; you approve each one." : mode === "assist" ? "Confident blockers post; the rest wait for you." : "Notes that pass the guardrails post on their own."}
        </span>
        <Button onClick={onDone}>Cancel</Button>
        <Button variant="default" disabled={!chosen.length || add.isPending} onClick={() => add.mutate()}>
          {add.isPending ? <Loader2 className="animate-spin" /> : null}
          {add.isPending
            ? clones
              ? `Cloning ${clones}…`
              : "Adding…"
            : chosen.length
              ? `Watch ${chosen.length} project${chosen.length === 1 ? "" : "s"}`
              : "Watch projects"}
        </Button>
      </div>
    </div>
  );
}

function ProjectRow({
  p,
  checked,
  onCheck,
  source,
  onSource,
}: {
  p: DiscoveredProject;
  checked: boolean;
  onCheck: (v: boolean) => void;
  source: string;
  onSource: (v: string) => void;
}) {
  const watching = p.existing_project_id != null;
  const id = p.remote.remote_id;
  return (
    <div
      role="listitem"
      className={cn(
        "grid grid-cols-[20px_minmax(0,1.1fr)_minmax(0,1.7fr)_minmax(0,0.6fr)] items-center gap-3 border-b px-3 py-2 last:border-b-0",
        checked && !watching && "bg-brand-soft/60",
        watching && "opacity-60",
      )}
    >
      <Checkbox aria-label={`Watch ${id}`} checked={watching || checked} disabled={watching} onCheckedChange={(v) => onCheck(!!v)} />
      <label className="min-w-0 cursor-pointer" onClick={() => !watching && onCheck(!checked)}>
        <div className="truncate text-xs font-semibold">{p.remote.name}</div>
        <div className="truncate font-mono text-2xs text-muted-foreground">
          {id}
          {p.remote.last_activity_at ? ` · ${formatRelative(p.remote.last_activity_at)}` : ""}
        </div>
      </label>
      <div className="flex min-w-0 items-center gap-2">
        {watching ? (
          <span className="rounded-full bg-green-soft px-2 py-px text-2xs font-semibold text-green">Already watching</span>
        ) : p.local_paths.length > 0 ? (
          <>
            <span className={cn("shrink-0 rounded-full px-2 py-px text-2xs font-semibold", source === MANAGED ? "bg-brand-soft text-primary" : "bg-green-soft text-green")}>
              {source === MANAGED ? "Warden clones" : p.local_paths.length > 1 ? `${p.local_paths.length} clones` : "Found"}
            </span>
            <SourceSelect id={id} paths={p.local_paths} value={source} onChange={onSource} />
          </>
        ) : (
          <>
            <span className="shrink-0 rounded-full bg-brand-soft px-2 py-px text-2xs font-semibold text-primary">Warden clones</span>
            <span className="truncate text-2xs text-muted-foreground">No clone found. Warden keeps its own copy.</span>
          </>
        )}
      </div>
      <div className="truncate">
        {p.remote.default_branch && <span className="rounded-full border bg-secondary px-2 py-px font-mono text-2xs">{p.remote.default_branch}</span>}
      </div>
    </div>
  );
}

function SourceSelect({ id, paths, value, onChange }: { id: string; paths: string[]; value: string; onChange: (v: string) => void }) {
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger aria-label={`Code source for ${id}`} className="h-6 min-w-0 flex-1 border-transparent bg-transparent px-1 font-mono text-2xs text-muted-foreground hover:border-input">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {paths.map((path) => (
          <SelectItem key={path} value={path} className="font-mono text-2xs">
            {path}
          </SelectItem>
        ))}
        <SelectItem value={MANAGED} className="text-xs">
          Let Warden keep its own copy
        </SelectItem>
      </SelectContent>
    </Select>
  );
}
