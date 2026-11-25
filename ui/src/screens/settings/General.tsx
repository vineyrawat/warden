// General pane: the setup / health checklist (9A). Green rows collapse to one line; anything
// missing shows the exact fix, a copy button and "Check again".
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ChevronRight, RefreshCw } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Loading } from "@/components/layout";
import { AddProjectsDialog } from "@/components/add-projects";
import * as api from "@/lib/api";
import { paneLabel, useNav } from "@/lib/nav";
import { errorMessage, qk, useEngineStatus, useForges, useProjects, useSettings, useTools } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { buildChecklist, readiness, type CheckItem, type Recheck } from "./health";
import { CommandLine, PaneColumn } from "./shared";

export function GeneralPane() {
  const qc = useQueryClient();
  const { navigate } = useNav();
  const engine = useEngineStatus();
  const forges = useForges();
  const projects = useProjects();
  const tools = useTools();
  const settings = useSettings().data;
  const [adding, setAdding] = useState(false);

  const deep = useMutation({
    mutationFn: api.check_engine_deep,
    onSuccess: (st) => {
      qc.setQueryData(qk.engine, st);
      (st.ok ? toast.success : toast.error)(st.message);
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  const detect = useMutation({
    mutationFn: api.detect_tools,
    onSuccess: (t) => qc.setQueryData(qk.tools, t),
    onError: (e) => toast.error(errorMessage(e)),
  });

  const recheck = (r: Recheck) => {
    switch (r) {
      case "deep":
        return deep.mutate();
      case "engine":
        return void engine.refetch();
      case "tools":
        return detect.mutate();
      case "forges":
        return void forges.refetch();
      case "projects":
        return void projects.refetch();
    }
  };
  const busy = (r?: Recheck) =>
    r === "deep" ? deep.isPending : r === "tools" ? detect.isPending : r === "engine" ? engine.isFetching : r === "forges" ? forges.isFetching : r === "projects" ? projects.isFetching : false;

  const checkAll = () => {
    void engine.refetch();
    void forges.refetch();
    void projects.refetch();
    detect.mutate();
  };

  if (!engine.data || !forges.data || !projects.data)
    return (
      <PaneColumn>
        <Loading label="Checking setup…" />
      </PaneColumn>
    );

  const items = buildChecklist({ engine: engine.data, forges: forges.data, projects: projects.data, tools: tools.data, mcpPort: settings?.mcp_port });
  const { ready, total } = readiness(items);
  const allOk = ready === total;

  const onPane = (it: CheckItem) => {
    if (it.paneAction === "Add projects") setAdding(true);
    else if (it.pane) navigate({ screen: "settings", pane: it.pane });
  };

  return (
    <PaneColumn>
      <div className="flex items-center gap-2.5">
        <h2 className="flex-1 text-sm font-semibold">Setup</h2>
        <Badge tone={allOk ? "green" : "orange"}>{allOk ? `All ${total} ready` : `${ready} of ${total} ready`}</Badge>
        <Button size="sm" onClick={checkAll} disabled={detect.isPending || engine.isFetching}>
          <RefreshCw className={cn((detect.isPending || engine.isFetching) && "animate-spin")} /> Check all
        </Button>
      </div>
      <ul className="overflow-hidden rounded-[10px] border bg-card" aria-label="Setup checklist">
        {items.map((it) => (
          <CheckRow key={it.id} item={it} busy={busy(it.recheck)} onRecheck={recheck} onPane={onPane} />
        ))}
      </ul>
      <p className="text-2xs text-muted-foreground">
        Check all re-reads status without using your subscription. The login check sends one tiny prompt.
      </p>
      <AddProjectsDialog open={adding} onOpenChange={setAdding} />
    </PaneColumn>
  );
}

function CheckRow({ item, busy, onRecheck, onPane }: { item: CheckItem; busy: boolean; onRecheck: (r: Recheck) => void; onPane: (it: CheckItem) => void }) {
  const ok = item.level === "ok";
  const icon = (
    <span
      aria-hidden
      className={cn(
        "inline-flex h-5 w-5 items-center justify-center rounded-full font-mono text-[11px] font-bold",
        ok ? "bg-green-soft text-green" : item.level === "error" ? "bg-red-soft text-red" : "bg-orange-soft text-orange",
      )}
    >
      {ok ? "✓" : "!"}
    </span>
  );
  const status = <span className="sr-only">{ok ? "Ready: " : item.level === "error" ? "Problem: " : "Warning: "}</span>;

  if (ok)
    return (
      <li className="grid grid-cols-[22px_minmax(0,1fr)_auto] items-center gap-2.5 border-t px-3 py-2.5 text-[12.5px] first:border-t-0">
        {icon}
        <span className="min-w-0 truncate">
          {status}
          <b>{item.title}</b> {item.detail && <span className="font-mono text-2xs text-muted-foreground">{item.detail}</span>}
        </span>
        {item.pane &&
          (item.paneAction ? (
            <Button size="sm" variant="ghost" onClick={() => onPane(item)}>
              {item.paneAction}
            </Button>
          ) : (
            <Button size="icon" variant="ghost" className="h-6 w-6 text-muted-foreground" aria-label={`Open ${paneLabel(item.pane)} settings for ${item.title}`} onClick={() => onPane(item)}>
              <ChevronRight />
            </Button>
          ))}
      </li>
    );

  return (
    <li
      className={cn(
        "grid grid-cols-[22px_minmax(0,1fr)_auto] items-start gap-2.5 border-t px-3 py-2.5 text-[12.5px] first:border-t-0",
        item.level === "error" ? "bg-red-soft" : "bg-orange-soft",
      )}
    >
      <span className="pt-px">{icon}</span>
      <span className="grid min-w-0 gap-1">
        <b>
          {status}
          {item.title}
        </b>
        {item.detail && <span className="text-xs text-muted-foreground">{item.detail}</span>}
        {item.fix && <span className="text-xs">{item.fix}</span>}
        {item.command && <CommandLine command={item.command} />}
      </span>
      <span className="flex flex-col items-end gap-1.5">
        {item.recheck && (
          <Button size="sm" onClick={() => onRecheck(item.recheck!)} disabled={busy}>
            <RefreshCw className={cn(busy && "animate-spin")} /> {busy ? "Checking…" : "Check again"}
          </Button>
        )}
        {item.pane && (item.paneAction || !item.recheck) && (
          <Button size="sm" variant="ghost" onClick={() => onPane(item)}>
            {item.paneAction ?? `Open ${paneLabel(item.pane)}`}
          </Button>
        )}
      </span>
    </li>
  );
}
