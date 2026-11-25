// Advanced pane: data folder, worktree root, posting pause and the MCP endpoint.
import { useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Loading } from "@/components/layout";
import { useSaveSettings, useSetPostingPaused, useSettings, useUsage } from "@/lib/queries";
import { CopyButton, Kv, Pane, PaneColumn, PaneNote } from "./shared";

export function AdvancedPane() {
  const settings = useSettings().data;
  const paused = useUsage().data?.paused ?? settings?.posting_paused ?? false;
  const save = useSaveSettings();
  const setPaused = useSetPostingPaused();
  const [dataDir, setDataDir] = useState<string | null>(null);
  const [worktree, setWorktree] = useState<string | null>(null);

  if (!settings)
    return (
      <PaneColumn>
        <Loading />
      </PaneColumn>
    );

  const dd = dataDir ?? settings.data_dir;
  const wt = worktree ?? settings.worktree_root;
  const dirty = dd.trim() !== settings.data_dir || wt.trim() !== settings.worktree_root;
  const mcpUrl = `http://127.0.0.1:${settings.mcp_port}/mcp`;

  return (
    <PaneColumn>
      <Pane title="Posting">
        <Kv label="Pause posting everywhere" hint="Reviews still run; nothing is posted to GitLab or GitHub until you resume." htmlFor="adv-paused">
          <Switch
            id="adv-paused"
            checked={paused}
            disabled={setPaused.isPending}
            onCheckedChange={(v) => setPaused.mutate(v, { onSuccess: () => toast.success(v ? "Posting paused everywhere" : "Posting resumed") })}
          />
        </Kv>
      </Pane>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (!dirty || !dd.trim() || !wt.trim()) return;
          save.mutate(
            { ...settings, data_dir: dd.trim(), worktree_root: wt.trim() },
            {
              onSuccess: () => {
                setDataDir(null);
                setWorktree(null);
                toast.success("Folders saved");
              },
            },
          );
        }}
      >
        <Pane
          title="Folders"
          actions={
            <>
              {dirty && (
                <Button size="sm" variant="ghost" onClick={() => { setDataDir(null); setWorktree(null); }}>
                  Revert
                </Button>
              )}
              <Button size="sm" type="submit" variant="default" disabled={!dirty || !dd.trim() || !wt.trim() || save.isPending}>
                Save
              </Button>
            </>
          }
        >
          <Kv label="Data folder" hint="Holds warden.db and logs/" htmlFor="adv-data">
            <Input id="adv-data" className="w-[280px]" value={dd} onChange={(e) => setDataDir(e.target.value)} />
          </Kv>
          <Kv label="Worktree root" hint="Each review checks out the MR here" htmlFor="adv-wt">
            <Input id="adv-wt" className="w-[280px]" value={wt} onChange={(e) => setWorktree(e.target.value)} />
          </Kv>
        </Pane>
      </form>
      <Pane title="MCP server">
        <Kv label="Endpoint" hint="Add it to Claude chat to ask about reviews. Port is set in Claude.">
          <span className="font-mono text-xs">{mcpUrl}</span>
          <CopyButton text={mcpUrl} />
        </Kv>
      </Pane>
      <PaneNote>~ is expanded to your home folder.</PaneNote>
    </PaneColumn>
  );
}
