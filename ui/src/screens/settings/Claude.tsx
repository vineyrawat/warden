// Claude pane: the engine (CLI, login, deep check) and the app-wide limits that apply to every
// project (model, max turns, reviews per hour, timeout, MCP port).
import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Stethoscope } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Loading } from "@/components/layout";
import { StatusDot } from "@/components/pills";
import * as api from "@/lib/api";
import { errorMessage, qk, useEngineStatus, useSaveSettings, useSettings } from "@/lib/queries";
import { applyLimits, draftFromSettings, limitsDirty, MODELS, validateLimits, type LimitsDraft } from "./limits";
import { Kv, Pane, PaneColumn, PaneNote } from "./shared";

export function ClaudePane() {
  return (
    <PaneColumn>
      <EngineSection />
      <LimitsSection />
    </PaneColumn>
  );
}

function EngineSection() {
  const qc = useQueryClient();
  const engine = useEngineStatus().data;
  const settings = useSettings().data;
  const save = useSaveSettings();
  const [cliPath, setCliPath] = useState<string | null>(null);
  const deep = useMutation({
    mutationFn: api.check_engine_deep,
    onSuccess: (st) => {
      qc.setQueryData(qk.engine, st);
      (st.ok ? toast.success : toast.error)(st.message);
    },
    onError: (e) => toast.error(errorMessage(e)),
  });
  const pathValue = cliPath ?? settings?.claude_cli_path ?? "";
  const pathDirty = cliPath !== null && cliPath !== (settings?.claude_cli_path ?? "");

  return (
    <Pane
      title="Claude engine"
      actions={
        <Button size="sm" onClick={() => deep.mutate()} disabled={deep.isPending}>
          <Stethoscope /> {deep.isPending ? "Checking…" : "Run deep check"}
        </Button>
      }
    >
      <div className="mb-1 flex items-start gap-2 rounded-md border border-primary bg-brand-soft p-2 text-[12.5px]">
        <span aria-hidden className="mt-[3px] h-[13px] w-[13px] shrink-0 rounded-full border-4 border-primary" />
        <div>
          <b>Claude Code subscription</b>
          <small className="block text-2xs text-muted-foreground">
            <code className="font-mono">claude -p</code> headless with your logged-in Pro or Max plan. No API key, no per-token billing.
          </small>
        </div>
      </div>
      {!engine ? (
        <Loading />
      ) : (
        <>
          <Kv label="CLI path" hint="Empty = find claude on PATH" htmlFor="cli-path">
            <form
              className="flex items-center gap-1.5"
              onSubmit={(ev) => {
                ev.preventDefault();
                if (!settings) return;
                save.mutate({ ...settings, claude_cli_path: pathValue.trim() || null }, { onSuccess: () => { setCliPath(null); toast.success("CLI path saved"); } });
              }}
            >
              <Input id="cli-path" className="w-[240px]" placeholder={engine.cli_path ?? "auto-detect on PATH"} value={pathValue} onChange={(ev) => setCliPath(ev.target.value)} />
              {pathDirty && (
                <Button size="sm" type="submit" variant="default" disabled={save.isPending}>
                  Save
                </Button>
              )}
            </form>
          </Kv>
          <Kv label="Version">
            <span className="font-mono text-xs">{engine.version ?? "not found"}</span>
          </Kv>
          <Kv label="Login">
            <span className="text-xs">{engine.logged_in ? `${engine.subscription ?? "Claude"} · subscription` : "not verified · run deep check"}</span>
          </Kv>
          <Kv label="API key in environment">
            {engine.api_key_in_env ? <Badge tone="orange">set · stripped for reviews</Badge> : <Badge tone="green">none</Badge>}
          </Kv>
          <Kv label="Status">
            <StatusDot ok={engine.ok} warn={!engine.ok} className="max-w-[320px] truncate text-xs">
              {engine.message}
            </StatusDot>
          </Kv>
        </>
      )}
      <PaneNote>The deep check sends one tiny prompt with your subscription to confirm login.</PaneNote>
    </Pane>
  );
}

function LimitsSection() {
  const settings = useSettings().data;
  const save = useSaveSettings();
  const [draft, setDraft] = useState<LimitsDraft | null>(null);
  useEffect(() => {
    if (settings && !draft) setDraft(draftFromSettings(settings));
  }, [settings, draft]);

  if (!settings || !draft)
    return (
      <Pane title="Limits">
        <Loading />
      </Pane>
    );

  const errors = validateLimits(draft);
  const dirty = limitsDirty(settings, draft);
  const valid = Object.keys(errors).length === 0;
  const set = (k: keyof LimitsDraft) => (e: React.ChangeEvent<HTMLInputElement>) => setDraft({ ...draft, [k]: e.target.value });
  const models = MODELS.includes(settings.model) ? MODELS : [settings.model, ...MODELS];

  const num = (k: Exclude<keyof LimitsDraft, "model">, label: string, hint: string, unit?: string) => (
    <Kv label={label} hint={errors[k] ? <span className="text-red">{errors[k]}</span> : hint} htmlFor={`lim-${k}`}>
      <Input
        id={`lim-${k}`}
        inputMode="numeric"
        className="w-[90px] text-right"
        value={draft[k]}
        onChange={set(k)}
        aria-invalid={!!errors[k]}
      />
      <span className="w-[52px] text-xs text-muted-foreground">{unit}</span>
    </Kv>
  );

  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (!valid || !dirty) return;
        save.mutate(applyLimits(settings, draft), {
          onSuccess: () => {
            setDraft(null);
            toast.success("Limits saved");
          },
        });
      }}
    >
      <Pane
        title="Limits for every project"
        actions={
          <>
            {dirty && (
              <Button size="sm" variant="ghost" onClick={() => setDraft(draftFromSettings(settings))}>
                Revert
              </Button>
            )}
            <Button size="sm" type="submit" variant="default" disabled={!dirty || !valid || save.isPending}>
              {save.isPending ? "Saving…" : "Save"}
            </Button>
          </>
        }
      >
        <Kv label="Model" hint="Used for every review" htmlFor="lim-model">
          <Select value={draft.model} onValueChange={(v) => setDraft({ ...draft, model: v })}>
            <SelectTrigger id="lim-model" className="w-[90px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {models.map((m) => (
                <SelectItem key={m} value={m}>
                  {m}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <span className="w-[52px]" />
        </Kv>
        {num("max_turns", "Max turns per review", "Upper limit for one review", "turns")}
        {num("max_reviews_per_hour", "Reviews per hour", "New reviews wait when the limit is reached", "per hour")}
        {num("timeout_min", "Review timeout", "A run longer than this is cancelled", "minutes")}
        {num("mcp_port", "MCP port", "Warden's local MCP server for Claude", "")}
      </Pane>
      <p className="mt-2 text-2xs text-muted-foreground">Per-project rules, confidence and posting live in Projects → Guardrails.</p>
    </form>
  );
}
