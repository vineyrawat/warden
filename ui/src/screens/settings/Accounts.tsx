// Accounts pane: forge connections and tokens (9B).
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ShieldCheck } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { ForgeKind } from "@/bindings/ForgeKind";
import type { ForgeStatus } from "@/bindings/ForgeStatus";
import * as api from "@/lib/api";
import { errorMessage, qk, useForges } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { hostOf } from "./health";
import { Kv, Pane, PaneColumn, PaneNote } from "./shared";

const FORGES: { forge: ForgeKind; name: string; short: string; defaultUrl: string; host: string; hint: string; scopes: string }[] = [
  {
    forge: "gitlab",
    name: "GitLab",
    short: "GL",
    defaultUrl: "https://gitlab.com",
    host: "gitlab.com",
    hint: "Project access token for a bot user (e.g. warden-bot), scope api.",
    scopes: "api, read_repository",
  },
  {
    forge: "github",
    name: "GitHub",
    short: "GH",
    defaultUrl: "https://api.github.com",
    host: "github.com",
    hint: "Fine-grained token with pull requests read/write.",
    scopes: "pull requests: read/write",
  },
];

export function AccountsPane() {
  const forges = useForges().data ?? [];
  const posting = forges.find((f) => f.ok);
  return (
    <PaneColumn>
      <ul className="overflow-hidden rounded-[10px] border bg-card" aria-label="Forge accounts">
        {FORGES.map((f) => (
          <ForgeRow key={f.forge} meta={f} status={forges.find((s) => s.forge === f.forge)} />
        ))}
      </ul>
      <Pane title="Bot identity for posting">
        <Kv
          label="Post as"
          hint={`Notes appear under this user on ${posting?.forge === "github" ? "GitHub" : "GitLab"}. To post as a bot, save a token from the bot account.`}
        >
          <span className="rounded-full border bg-surface-2 px-2.5 py-0.5 font-mono text-xs">{posting?.user ? `@${posting.user.replace(/^@/, "")}` : "no account"}</span>
        </Kv>
      </Pane>
      <PaneNote>
        <ShieldCheck className="mr-1 inline h-3 w-3 align-[-2px]" aria-hidden />
        Tokens are stored in the macOS Keychain and never shown again after saving. Claude never sees them.
      </PaneNote>
    </PaneColumn>
  );
}

function ForgeRow({ meta, status }: { meta: (typeof FORGES)[number]; status?: ForgeStatus }) {
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const [baseUrl, setBaseUrl] = useState(status?.base_url ?? meta.defaultUrl);
  const [token, setToken] = useState("");
  const save = useMutation({
    mutationFn: () => api.set_forge_token(meta.forge, baseUrl.trim(), token),
    onSuccess: (user) => {
      setToken("");
      setOpen(false);
      qc.invalidateQueries({ queryKey: qk.forges });
      toast.success(`${meta.name} connected as ${user}`);
    },
    onError: (e) => {
      setToken("");
      toast.error(errorMessage(e));
    },
  });
  const id = `token-${meta.forge}`;
  const host = status ? hostOf(status.base_url) : meta.host;

  return (
    <li className="border-t first:border-t-0">
      <div className="grid grid-cols-[28px_minmax(0,1fr)_auto] items-center gap-2.5 px-3 py-2.5 text-[12.5px]">
        <span
          aria-hidden
          className={cn(
            "inline-flex h-6 w-6 items-center justify-center rounded-md font-mono text-[10px] font-semibold",
            meta.forge === "gitlab" ? "bg-orange-soft text-orange" : "bg-surface-2 text-foreground",
          )}
        >
          {meta.short}
        </span>
        <span className="min-w-0 truncate">
          <b>{host}</b>{" "}
          <span className="font-mono text-2xs text-muted-foreground">
            {status ? `${status.user ? `@${status.user.replace(/^@/, "")}` : "unknown user"} · ${meta.scopes}` : "not connected"}
          </span>
        </span>
        <span className="flex items-center gap-1.5">
          {status && <Badge tone={status.ok ? "green" : "red"}>{status.ok ? "Connected" : "Token rejected"}</Badge>}
          <Button size="sm" variant={status ? "ghost" : "outline"} onClick={() => setOpen((o) => !o)} aria-expanded={open} aria-controls={`${id}-form`}>
            {open ? "Cancel" : status ? "Replace token" : "Connect…"}
          </Button>
        </span>
      </div>
      {open && (
        <form
          id={`${id}-form`}
          className="grid grid-cols-[1fr_1fr_auto] items-end gap-2 border-t border-dashed bg-surface-2/50 px-3 py-2.5"
          autoComplete="off"
          onSubmit={(e) => {
            e.preventDefault();
            if (token.trim()) save.mutate();
          }}
        >
          <div className="space-y-1">
            <Label htmlFor={`${id}-url`} className="text-2xs text-muted-foreground">
              Base URL
            </Label>
            <Input id={`${id}-url`} value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} required />
          </div>
          <div className="space-y-1">
            <Label htmlFor={id} className="text-2xs text-muted-foreground">
              Token
            </Label>
            <Input id={id} type="password" autoComplete="new-password" spellCheck={false} placeholder="paste token" value={token} onChange={(e) => setToken(e.target.value)} required autoFocus />
          </div>
          <Button type="submit" variant="default" disabled={!token.trim() || save.isPending}>
            {save.isPending ? "Checking…" : "Save"}
          </Button>
          <p className="col-span-3 text-2xs text-muted-foreground">{meta.hint}</p>
        </form>
      )}
    </li>
  );
}
