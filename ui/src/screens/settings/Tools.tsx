// Tools pane: CI / scan tool detection with install commands.
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Loading } from "@/components/layout";
import * as api from "@/lib/api";
import { errorMessage, qk, useTools } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { toolFix, toolImpact } from "./health";
import { CommandLine, PaneColumn, PaneNote } from "./shared";

export function ToolsPane() {
  const qc = useQueryClient();
  const tools = useTools();
  const refresh = useMutation({
    mutationFn: api.detect_tools,
    onSuccess: (t) => {
      qc.setQueryData(qk.tools, t);
      toast.success(`${t.filter((x) => x.installed).length} of ${t.length} tools found`);
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  return (
    <PaneColumn>
      <div className="flex items-center gap-2.5">
        <h2 className="flex-1 text-sm font-semibold">CI and scan tools</h2>
        <Button size="sm" onClick={() => refresh.mutate()} disabled={refresh.isPending}>
          <RefreshCw className={cn(refresh.isPending && "animate-spin")} /> Re-detect
        </Button>
      </div>
      {tools.isLoading ? (
        <Loading />
      ) : (
        <ul className="overflow-hidden rounded-[10px] border bg-card" aria-label="Tools">
          {(tools.data ?? []).map((t) => {
            const fix = toolFix(t.name);
            return (
              <li key={t.name} className="grid grid-cols-[130px_minmax(0,1fr)] items-start gap-3 border-t px-3 py-2.5 text-[12.5px] first:border-t-0">
                <span className="pt-0.5 font-mono text-xs font-semibold">{t.name}</span>
                <span className="grid min-w-0 gap-1">
                  <span>
                    {t.installed ? <Badge tone="green">installed{t.version ? ` · ${t.version}` : ""}</Badge> : <Badge tone="grey">not installed</Badge>}
                    {!t.installed && t.name !== "git" && t.name !== "claude" && <span className="ml-2 text-xs text-muted-foreground">{toolImpact(t.name)}</span>}
                  </span>
                  {!t.installed && (fix.command ? <CommandLine command={fix.command} /> : <span className="text-xs text-muted-foreground">{fix.fix}</span>)}
                </span>
              </li>
            );
          })}
        </ul>
      )}
      <PaneNote>Optional tools are skipped with a notice when missing; reviews still run.</PaneNote>
    </PaneColumn>
  );
}
