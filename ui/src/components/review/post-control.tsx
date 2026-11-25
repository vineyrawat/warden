import { useEffect, useState } from "react";
import { Send, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Tip } from "@/components/ui/tooltip";
import type { PostReport } from "@/bindings/PostReport";
import { cn } from "@/lib/utils";
import { plural } from "./logic";

/**
 * Post button with an inline confirm step (posting writes to the forge). When disabled, the
 * reason shows in a tooltip and, with `showReason`, as text next to the button.
 */
export function PostControl({
  count,
  label,
  target,
  disabledReason,
  busy,
  onPost,
  inverse,
  showReason,
}: {
  count: number;
  /** Button text, e.g. "Post 2 approved". */
  label: string;
  /** Where it goes, e.g. "GitLab !41". */
  target: string;
  disabledReason: string | null;
  busy: boolean;
  onPost: () => void;
  /** Light-on-dark styling for the Diff tab's floating bar. */
  inverse?: boolean;
  showReason?: boolean;
}) {
  const [confirming, setConfirming] = useState(false);
  useEffect(() => {
    if (disabledReason) setConfirming(false);
  }, [disabledReason]);

  if (confirming && !disabledReason) {
    return (
      <div
        className="flex items-center gap-2"
        role="group"
        aria-label="Confirm posting"
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            setConfirming(false);
          }
        }}
      >
        <span className={cn("text-xs", inverse ? "text-background" : "text-foreground")}>
          Post {plural(count, "note")} to {target}?
        </span>
        <Button size="default" variant="ghost" className={cn(inverse && "text-background hover:bg-background/15 hover:text-background")} onClick={() => setConfirming(false)}>
          Cancel
        </Button>
        <Button
          variant="default"
          autoFocus
          disabled={busy}
          onClick={() => {
            onPost();
            setConfirming(false);
          }}
        >
          <Send /> Post {count}
        </Button>
      </div>
    );
  }

  const btn = (
    <Button variant={disabledReason ? "outline" : "default"} onClick={() => setConfirming(true)} disabled={!!disabledReason || busy} aria-disabled={!!disabledReason}>
      <Send /> {busy ? "Posting…" : label}
    </Button>
  );
  return (
    <div className="flex min-w-0 items-center gap-2">
      {disabledReason && showReason && <span className={cn("truncate text-2xs", inverse ? "text-background/70" : "text-muted-foreground")}>{disabledReason}</span>}
      {disabledReason ? (
        <Tip content={disabledReason}>
          <span tabIndex={0} className="shrink-0 rounded-md">
            {btn}
          </span>
        </Tip>
      ) : (
        btn
      )}
    </div>
  );
}

/** What happened on the last post: posted / file-note fallbacks / failed / skipped, with reasons. */
export function PostReportView({ report, forgeName, onClose, onOpenAudit, className }: { report: PostReport; forgeName: string; onClose: () => void; onOpenAudit: () => void; className?: string }) {
  const ok = report.failed === 0;
  return (
    <div role="status" className={cn("grid gap-1 rounded-md border px-3 py-2 text-xs", ok ? "border-green/30 bg-green-soft" : "border-orange/30 bg-orange-soft", className)}>
      <div className="flex items-center gap-2">
        <b className={ok ? "text-green" : "text-orange"}>
          {report.posted > 0 ? `Posted ${plural(report.posted, "note")} to ${forgeName}` : `Nothing was posted to ${forgeName}`}
        </b>
        <span className="flex-1" />
        <Button size="icon" variant="ghost" className="h-5 w-5" aria-label="Dismiss post report" onClick={onClose}>
          <X />
        </Button>
      </div>
      {report.fallback > 0 && <div>{plural(report.fallback, "note")} went in as a file note: {forgeName} rejected the line position.</div>}
      {report.failed > 0 && (
        <div>
          {report.failed} failed.{" "}
          <button type="button" className="rounded text-primary hover:underline" onClick={onOpenAudit}>
            See the error in the History audit
          </button>
        </div>
      )}
      {report.skipped > 0 && <div>{report.skipped} skipped: already posted on this MR, posting paused, or over the hourly note budget.</div>}
    </div>
  );
}
