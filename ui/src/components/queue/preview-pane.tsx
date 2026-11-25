// Note preview side pane (mockup 5C): the cursor finding rendered as the forge will show it, with
// the MR / file context, edit in place, and Dismiss / Approve.
import { ExternalLink, Pencil, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { ErrorBox } from "@/components/layout";
import type { PendingFinding } from "@/bindings/PendingFinding";
import { anchorText, findFile } from "@/components/review/logic";
import { usePreviewNote, useRun } from "@/lib/queries";
import { Kbd } from "./bits";
import { NoteMarkdown } from "./note-markdown";
import { QueueEditor } from "./queue-editor";
import { forgeName } from "./queue-logic";

function hostOf(url: string): string | null {
  try {
    return url ? new URL(url).host : null;
  } catch {
    return null;
  }
}

export function PreviewPane({
  row,
  editing,
  onEdit,
  onEditDone,
  onApprove,
  onDismiss,
  onClose,
  busy,
}: {
  row: PendingFinding | null;
  editing: boolean;
  onEdit: () => void;
  onEditDone: () => void;
  onApprove: () => void;
  onDismiss: () => void;
  onClose: () => void;
  busy: boolean;
}) {
  const f = row?.finding ?? null;
  const note = usePreviewNote(f?.id ?? null);
  const run = useRun(row?.run_id ?? null);
  const before = f && run.data ? anchorText(findFile(run.data.files, f.file), f) : null;
  const host = row ? hostOf(row.web_url) : null;
  const forge = row ? forgeName([row]) : "GitLab";

  return (
    <aside aria-label="Note preview" className="flex min-h-0 w-[44%] min-w-[340px] max-w-[560px] shrink-0 flex-col border-l bg-surface">
      <div className="flex min-h-[37px] items-center gap-2 border-b px-3.5 py-1.5">
        <span className="text-2xs font-semibold uppercase tracking-[.04em] text-muted-foreground">Note preview</span>
        <span className="flex-1" />
        <Button size="icon" variant="ghost" className="h-6 w-6" onClick={onClose} aria-label="Close preview (P)" title="Close preview (P)">
          <X />
        </Button>
      </div>
      {!row || !f ? (
        <div className="px-4 py-10 text-center text-xs text-muted-foreground">Move to a finding to see its note.</div>
      ) : (
        <div className="min-h-0 flex-1 space-y-3 overflow-y-auto px-3.5 py-3">
          <div className="flex min-w-0 items-center gap-1.5 text-xs">
            <span className="shrink-0 font-mono text-muted-foreground">!{row.mr_iid}</span>
            <b className="min-w-0 truncate font-semibold" title={row.mr_title}>
              {row.mr_title}
            </b>
            <span className="shrink-0 text-muted-foreground">· {row.project_name}</span>
            <span className="flex-1" />
            {row.web_url && (
              <a href={row.web_url} target="_blank" rel="noopener noreferrer" className="inline-flex shrink-0 items-center gap-1 text-2xs text-primary hover:underline">
                Open on {forge} <ExternalLink className="size-3" aria-hidden />
              </a>
            )}
          </div>

          <div className="text-2xs font-medium text-muted-foreground">Will be posted on {host ?? forge} as</div>

          <article className="overflow-hidden rounded-lg border bg-card shadow-sm" aria-label="Rendered note">
            <header className="flex min-w-0 items-center gap-2 border-b bg-surface-2 px-3 py-2 text-xs">
              <span aria-hidden className="grid size-5 shrink-0 place-items-center rounded-full bg-primary text-[10px] font-bold text-primary-foreground">
                W
              </span>
              <b className="shrink-0 font-semibold">warden-bot</b>
              <span className="min-w-0 truncate text-muted-foreground">
                {f.position ? (
                  <>
                    commented on <span className="font-mono">{f.file}</span> line {f.line}
                  </>
                ) : (
                  "commented on the merge request"
                )}
              </span>
            </header>
            <div className="px-3 py-2.5">
              {editing ? (
                <QueueEditor finding={f} onDone={onEditDone} />
              ) : note.isLoading ? (
                <div className="py-6 text-center text-xs text-muted-foreground">Rendering note…</div>
              ) : note.error ? (
                <ErrorBox error={note.error} />
              ) : (
                <NoteMarkdown source={note.data ?? ""} before={before} line={f.line > 0 ? f.line : undefined} />
              )}
            </div>
          </article>

          {!editing && (
            <div className="flex flex-wrap items-center gap-1.5">
              <Button size="sm" onClick={onEdit} disabled={busy}>
                <Pencil /> Edit text <Kbd className="ml-0.5">E</Kbd>
              </Button>
              <span className="flex-1" />
              <Button size="sm" onClick={onDismiss} disabled={busy}>
                Dismiss <Kbd className="ml-0.5">D</Kbd>
              </Button>
              <Button size="sm" variant="default" onClick={onApprove} disabled={busy || f.status === "approved"}>
                {f.status === "approved" ? "Approved" : "Approve"} {f.status !== "approved" && <Kbd className="ml-0.5 border-primary-foreground/30 bg-transparent text-primary-foreground/80">A</Kbd>}
              </Button>
            </div>
          )}
        </div>
      )}
    </aside>
  );
}
