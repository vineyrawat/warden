import { useEffect, useRef, useState } from "react";
import { Check, Pencil, RotateCcw, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Badge } from "@/components/ui/badge";
import { SeverityPill } from "@/components/pills";
import { RichText } from "@/components/layout";
import type { Finding } from "@/bindings/Finding";
import type { RuleConfig } from "@/bindings/RuleConfig";
import { useSetFindingStatus, useUpdateFinding } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { isFinal, sourceTag, suggestionDiff, type Decision } from "@/components/review/logic";

export const SEVERITY_BORDER: Record<Finding["severity"], string> = {
  blocker: "border-l-red",
  major: "border-l-orange",
  minor: "border-l-yellow",
  nit: "border-l-muted-foreground",
};

/** "FR-007 · regex · 0.90" (rule id or category, source, confidence), optionally led by file:line. */
export function findingMeta(f: Finding, opts: { rules?: RuleConfig[]; location?: boolean } = {}): string {
  return [opts.location ? `${f.file}:${f.line}` : null, f.rule_id ?? f.category, sourceTag(f, opts.rules), f.confidence.toFixed(2)]
    .filter(Boolean)
    .join(" · ");
}

/** Suggested code as a diff: the commented line removed, the suggestion added. */
export function SuggestionBlock({ suggestion, before, className }: { suggestion: string; before?: string | null; className?: string }) {
  const lines = suggestionDiff(before ?? null, suggestion);
  return (
    <div className={cn("overflow-hidden rounded-md border bg-codebg font-mono text-[11.5px] leading-[1.6]", className)}>
      <div className="border-b bg-surface-2 px-2.5 py-1 font-sans text-2xs font-medium text-muted-foreground">Suggestion</div>
      <div className="overflow-x-auto">
        <table className="min-w-full border-collapse">
          <tbody>
            {lines.map((l, i) => (
              <tr key={i} className={l.kind === "add" ? "bg-diff-add" : "bg-diff-del"}>
                <td className="w-5 select-none pl-2 text-muted-foreground">{l.kind === "add" ? "+" : "−"}</td>
                <td className="whitespace-pre pr-3">{l.text || " "}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

export function StatusTag({ finding: f }: { finding: Finding }) {
  switch (f.status) {
    case "approved":
      return <Badge tone="blue">Approved</Badge>;
    case "posted":
      return <Badge tone="green">Posted{f.forge_note_id ? ` · ${f.forge_note_id}` : ""}</Badge>;
    case "resolved":
      return <Badge tone="green">Resolved</Badge>;
    case "dismissed":
      return <Badge tone="grey">Dismissed</Badge>;
    case "rejected":
      return <Badge tone="grey">Dropped · {f.guardrail_reason ?? "guardrail"}</Badge>;
    default:
      return null;
  }
}

/**
 * Inline finding thread (mockup 4A `.thread`): severity, title, rule · source · confidence, body,
 * suggestion, then Dismiss / Edit / Approve. Pass `onDecide` to share decision state with a parent;
 * without it the card writes the status itself.
 */
export function FindingCard({
  finding: f,
  className,
  showLocation,
  id,
  onDecide,
  rules,
  before,
}: {
  finding: Finding;
  className?: string;
  showLocation?: boolean;
  id?: string;
  onDecide?: (f: Finding, to: Decision) => void;
  rules?: RuleConfig[];
  /** Text of the commented line, shown as the "−" side of the suggestion. */
  before?: string | null;
}) {
  const [editing, setEditing] = useState(false);
  const setStatus = useSetFindingStatus();
  const busy = !onDecide && setStatus.isPending;
  const muted = f.status === "rejected" || f.status === "dismissed";
  const final = isFinal(f.status);

  const act = (to: Decision) => (onDecide ? onDecide(f, to) : setStatus.mutate({ id: f.id, status: to, runId: f.run_id }));

  return (
    <article
      id={id}
      aria-label={`${f.severity} finding: ${f.title}`}
      className={cn("grid gap-1.5 rounded-md border border-l-[3px] bg-card px-2.5 py-2 text-xs leading-[1.45] shadow-sm", SEVERITY_BORDER[f.severity], className)}
    >
      {editing ? (
        <FindingEditor finding={f} onDone={() => setEditing(false)} />
      ) : (
        <>
          <div className={cn("flex flex-wrap items-center gap-2", muted && "opacity-60")}>
            <SeverityPill severity={f.severity} />
            <b className="min-w-0 flex-1 font-semibold">{f.title}</b>
            <span className="font-mono text-2xs text-muted-foreground">{findingMeta(f, { rules, location: showLocation })}</span>
          </div>
          <div className={cn(muted && "opacity-60")}>
            <RichText text={f.body} />
          </div>
          {f.suggestion && <SuggestionBlock suggestion={f.suggestion} before={before} className={cn(muted && "opacity-60")} />}
          {f.evidence && (
            <div className={cn("text-2xs text-muted-foreground", muted && "opacity-60")}>
              <span className="font-semibold">Evidence:</span> <RichText text={f.evidence} />
            </div>
          )}
          <div className="flex flex-wrap items-center gap-1.5">
            <StatusTag finding={f} />
            <span className="flex-1" />
            {!final && (
              <>
                {f.status === "dismissed" ? (
                  <Button size="sm" variant="ghost" onClick={() => act("draft")} disabled={busy}>
                    <RotateCcw /> Restore
                  </Button>
                ) : (
                  <Button size="sm" variant="ghost" onClick={() => act("dismissed")} disabled={busy}>
                    <X /> Dismiss
                  </Button>
                )}
                <Button size="sm" variant="ghost" onClick={() => setEditing(true)} disabled={busy}>
                  <Pencil /> Edit
                </Button>
                {f.status === "approved" ? (
                  <Button size="sm" onClick={() => act("draft")} disabled={busy} aria-label="Approved. Click to undo" title="Undo approval">
                    <Check /> Approved
                  </Button>
                ) : (
                  <Button size="sm" variant="default" onClick={() => act("approved")} disabled={busy}>
                    {f.status === "rejected" ? "Approve anyway" : "Approve"}
                  </Button>
                )}
              </>
            )}
          </div>
        </>
      )}
    </article>
  );
}

/** Inline editor for title, body and suggestion. Esc cancels, ⌘Enter (Ctrl+Enter) saves. */
export function FindingEditor({ finding: f, onDone, onSaved }: { finding: Finding; onDone: () => void; onSaved?: () => void }) {
  const [title, setTitle] = useState(f.title);
  const [body, setBody] = useState(f.body);
  const [suggestion, setSuggestion] = useState(f.suggestion ?? "");
  const update = useUpdateFinding();
  const titleRef = useRef<HTMLInputElement>(null);
  // Focus without letting the browser scroll a sideways-scrolling diff pane.
  useEffect(() => titleRef.current?.focus({ preventScroll: true }), []);
  const valid = title.trim().length > 0 && body.trim().length > 0;

  const save = () => {
    if (!valid || update.isPending) return;
    update.mutate(
      { id: f.id, runId: f.run_id, title: title.trim(), body: body.trim(), suggestion: suggestion.trim() ? suggestion : null },
      {
        onSuccess: () => {
          onSaved?.();
          onDone();
        },
      },
    );
  };

  return (
    <form
      className="space-y-2"
      onSubmit={(e) => {
        e.preventDefault();
        save();
      }}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          onDone();
        } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
          e.preventDefault();
          save();
        }
      }}
    >
      <div className="flex items-center gap-2">
        <SeverityPill severity={f.severity} />
        <Input aria-label="Finding title" value={title} onChange={(e) => setTitle(e.target.value)} className="font-sans font-semibold" ref={titleRef} />
      </div>
      <Textarea aria-label="Finding body" rows={4} value={body} onChange={(e) => setBody(e.target.value)} />
      <Textarea
        aria-label="Suggested code"
        rows={2}
        placeholder="Suggested code (optional, posted as a ```suggestion block)"
        value={suggestion}
        onChange={(e) => setSuggestion(e.target.value)}
        className="font-mono text-[11.5px]"
      />
      <div className="flex items-center gap-1.5">
        <Button size="sm" type="submit" variant="default" disabled={!valid || update.isPending}>
          {update.isPending ? "Saving…" : "Save"}
        </Button>
        <Button size="sm" onClick={onDone}>
          Cancel
        </Button>
        <span className="ml-auto text-2xs text-muted-foreground">
          <kbd className="font-mono">⌘↵</kbd> save · <kbd className="font-mono">Esc</kbd> cancel · edits are kept to tune the rules
        </span>
      </div>
    </form>
  );
}
