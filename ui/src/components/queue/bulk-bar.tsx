// Selection bar, inline post confirm, "Post all approved" footer and the post results strip.
import { Check, Send, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { cn } from "@/lib/utils";
import { Kbd, MOD } from "./bits";
import { plural, postAllLabel, postConfirmText, reportLine, reportTotals, type LabeledReport, type PostSet } from "./queue-logic";

export function SelectionBar({
  checked,
  selectedCount,
  onToggleAll,
  onApprove,
  onDismiss,
  onPost,
  busy,
  postBlocked,
  children,
}: {
  checked: boolean | "indeterminate";
  selectedCount: number;
  onToggleAll: () => void;
  onApprove: () => void;
  onDismiss: () => void;
  onPost: () => void;
  busy: boolean;
  /** Why posting is unavailable (paused), or null. */
  postBlocked: string | null;
  /** Replaces the actions, e.g. with an inline confirm. */
  children?: React.ReactNode;
}) {
  return (
    <div className="flex min-h-[38px] flex-wrap items-center gap-2 border-b bg-surface px-4 py-1.5 text-xs">
      <Checkbox
        checked={checked}
        aria-label={checked === true ? "Clear selection" : "Select all visible"}
        onClick={(e) => {
          e.preventDefault();
          onToggleAll();
        }}
      />
      {children ??
        (selectedCount > 0 ? (
          <>
            <span className="font-medium">{selectedCount} selected</span>
            <span className="flex-1" />
            <Button size="sm" onClick={onApprove} disabled={busy}>
              <Check /> Approve <Kbd>A</Kbd>
            </Button>
            <Button size="sm" onClick={onDismiss} disabled={busy}>
              <X /> Dismiss <Kbd>D</Kbd>
            </Button>
            <Button size="sm" variant="default" onClick={onPost} disabled={busy || postBlocked != null} title={postBlocked ?? undefined}>
              <Send /> Post {selectedCount}
            </Button>
          </>
        ) : (
          <span className="text-muted-foreground">
            <Kbd>↑</Kbd> <Kbd>↓</Kbd> move · <Kbd>X</Kbd> select · <Kbd>{MOD}A</Kbd> all · <Kbd>A</Kbd> approve · <Kbd>D</Kbd> dismiss · <Kbd>E</Kbd> edit · <Kbd>P</Kbd> preview
          </span>
        ))}
    </div>
  );
}

/** "Post 7 notes to 3 merge requests on GitLab?  Cancel  Post 7" */
export function PostConfirm({ set, forge, onConfirm, onCancel, pending }: { set: PostSet; forge: string; onConfirm: () => void; onCancel: () => void; pending: boolean }) {
  return (
    <div role="alertdialog" aria-label="Confirm posting" className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
      <span className="font-medium">{postConfirmText(set.ids.length, set.mrs, forge)}</span>
      <span className="flex-1" />
      <Button size="sm" onClick={onCancel} disabled={pending}>
        Cancel
      </Button>
      <Button size="sm" variant="default" onClick={onConfirm} disabled={pending} autoFocus>
        <Send /> {pending ? "Posting…" : `Post ${set.ids.length}`}
      </Button>
    </div>
  );
}

export function PostFooter({
  approved,
  onPostAll,
  postBlocked,
  onOpenSettings,
  confirm,
  busy,
}: {
  approved: PostSet;
  onPostAll: () => void;
  postBlocked: string | null;
  onOpenSettings: () => void;
  confirm?: React.ReactNode;
  busy: boolean;
}) {
  return (
    <div className="flex min-h-[44px] flex-wrap items-center gap-2 border-t bg-surface px-4 py-2 text-xs">
      {confirm ?? (
        <>
          {postBlocked ? (
            <span className="text-muted-foreground">
              {postBlocked}{" "}
              <button type="button" onClick={onOpenSettings} className="text-primary hover:underline">
                Resume in Settings
              </button>
            </span>
          ) : approved.ids.length === 0 ? (
            <span className="text-muted-foreground">Approve findings to post them.</span>
          ) : null}
          <span className="flex-1" />
          <Button variant="default" onClick={onPostAll} disabled={busy || postBlocked != null || approved.ids.length === 0} title={postBlocked ?? undefined}>
            <Send /> {postAllLabel(approved)}
          </Button>
        </>
      )}
    </div>
  );
}

export function PostResults({ reports, onClose }: { reports: LabeledReport[]; onClose: () => void }) {
  const t = reportTotals(reports);
  const notes = t.posted;
  const problem = t.failed > 0;
  return (
    <div
      role="status"
      className={cn("mx-4 mt-3 rounded-md border px-3 py-2 text-xs", problem ? "border-orange/40 bg-orange-soft" : "border-green/40 bg-green-soft")}
    >
      <div className="flex items-center gap-2">
        <b className={cn("font-semibold", problem ? "text-orange" : "text-green")}>
          Posted {plural(notes, "note")} to {plural(reports.filter((r) => r.report.posted > 0).length, "merge request")}
          {t.failed ? ` · ${t.failed} failed` : ""}
          {t.skipped ? ` · ${t.skipped} skipped` : ""}
        </b>
        <span className="flex-1" />
        <Button size="icon" variant="ghost" className="h-5 w-5" onClick={onClose} aria-label="Hide post results">
          <X />
        </Button>
      </div>
      <ul className="mt-1 space-y-0.5">
        {reports.map((r) => (
          <li key={r.runId} className="flex min-w-0 gap-2">
            <span className="min-w-0 truncate">{r.label}</span>
            <span className="shrink-0 font-mono text-2xs text-muted-foreground">{reportLine(r.report)}</span>
          </li>
        ))}
      </ul>
      {t.skipped > 0 && <div className="mt-1 text-2xs text-muted-foreground">Skipped findings were already posted, dismissed, or over the hourly posting budget.</div>}
    </div>
  );
}
