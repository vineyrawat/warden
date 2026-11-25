// "Review MR…" from the Inbox header: review a local branch against a base branch, outside the
// poll schedule. Repository defaults to the selected project's clone; branches default to the
// project's target branch and HEAD, with suggestions from the project's open MRs.
import { useEffect, useMemo, useState } from "react";
import { FolderOpen, Loader2, Play } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import type { MrRow } from "@/bindings/MrRow";
import type { Project } from "@/bindings/Project";
import * as api from "@/lib/api";
import { errorMessage } from "@/lib/queries";
import { defaultBaseBranch } from "./inbox-logic";

export function ReviewLocalDialog({
  open,
  onOpenChange,
  projects,
  mrs,
  preferProjectId,
  onStarted,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  projects: Project[];
  mrs: MrRow[];
  /** Project of the selected MR (or the project filter), used for the defaults. */
  preferProjectId: number | null;
  onStarted: (runId: number) => void;
}) {
  const withClone = projects.filter((p) => p.local_path);
  const [repoPath, setRepoPath] = useState("");
  const [extraPath, setExtraPath] = useState<string | null>(null);
  const [baseRef, setBaseRef] = useState("main");
  const [headRef, setHeadRef] = useState("HEAD");
  const [busy, setBusy] = useState(false);

  const project = withClone.find((p) => p.local_path === repoPath) ?? null;

  // Fresh defaults each time the dialog opens (deliberately keyed on `open` only).
  useEffect(() => {
    if (!open) return;
    const p = withClone.find((x) => x.id === preferProjectId) ?? withClone[0] ?? null;
    setRepoPath(p?.local_path ?? "");
    setExtraPath(null);
    setBaseRef(defaultBaseBranch(p?.target_branches));
    setHeadRef("HEAD");
  }, [open]);

  const baseOptions = useMemo(() => (project?.target_branches ?? []).filter((b) => !/[*?[]/.test(b)), [project]);
  const headOptions = useMemo(
    () => ["HEAD", ...new Set(mrs.filter((m) => project && m.project_id === project.id).map((m) => m.source_branch))],
    [mrs, project],
  );

  const choosePath = (v: string) => {
    setRepoPath(v);
    const p = withClone.find((x) => x.local_path === v);
    if (p) setBaseRef(defaultBaseBranch(p.target_branches));
  };

  const pickFolder = async () => {
    try {
      const path = await api.pick_folder();
      if (path) {
        setExtraPath(path);
        setRepoPath(path);
      }
    } catch (err) {
      toast.error(errorMessage(err));
    }
  };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      const runId = await api.review_local(repoPath.trim(), baseRef.trim(), headRef.trim());
      onOpenChange(false);
      onStarted(runId);
    } catch (err) {
      toast.error(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Review a local branch</DialogTitle>
          <DialogDescription>
            Runs checks, local CI and a Claude review now. Counts against the hourly review limit. To review an open MR, select it in the list and press R.
          </DialogDescription>
        </DialogHeader>
        <form className="space-y-3" onSubmit={submit}>
          <div className="space-y-1.5">
            <Label htmlFor="local-repo">Repository</Label>
            <div className="flex gap-2">
              {withClone.length > 0 || extraPath ? (
                <Select value={repoPath} onValueChange={choosePath}>
                  <SelectTrigger id="local-repo" className="min-w-0 flex-1 font-sans">
                    <SelectValue placeholder="Pick a repository" />
                  </SelectTrigger>
                  <SelectContent>
                    {withClone.map((p) => (
                      <SelectItem key={p.id} value={p.local_path}>
                        {p.name} · <span className="font-mono text-muted-foreground">{p.local_path}</span>
                      </SelectItem>
                    ))}
                    {extraPath && !withClone.some((p) => p.local_path === extraPath) && (
                      <SelectItem value={extraPath}>
                        <span className="font-mono">{extraPath}</span>
                      </SelectItem>
                    )}
                  </SelectContent>
                </Select>
              ) : (
                <Input id="local-repo" placeholder="~/code/entries_crm" value={repoPath} onChange={(e) => setRepoPath(e.target.value)} required className="flex-1" />
              )}
              <Button onClick={pickFolder}>
                <FolderOpen /> Folder…
              </Button>
            </div>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="local-base">Base branch</Label>
              <Input id="local-base" list="local-base-options" value={baseRef} onChange={(e) => setBaseRef(e.target.value)} required />
              <datalist id="local-base-options">
                {baseOptions.map((b) => (
                  <option key={b} value={b} />
                ))}
              </datalist>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="local-head">Branch to review</Label>
              <Input id="local-head" list="local-head-options" value={headRef} onChange={(e) => setHeadRef(e.target.value)} required />
              <datalist id="local-head-options">
                {headOptions.map((b) => (
                  <option key={b} value={b} />
                ))}
              </datalist>
            </div>
          </div>
          <p className="text-2xs text-muted-foreground">Local reviews never post anywhere; findings stay in Warden.</p>
          <DialogFooter>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button type="submit" variant="default" disabled={busy || !repoPath.trim()}>
              {busy ? <Loader2 className="animate-spin" /> : <Play />} Review branch
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
