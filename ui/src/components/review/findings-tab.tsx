import { useEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronDown, ChevronRight, RotateCcw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { SeverityPill } from "@/components/pills";
import { RichText } from "@/components/layout";
import { FindingEditor, StatusTag, SuggestionBlock, findingMeta } from "@/components/finding-card";
import type { FileDiff } from "@/bindings/FileDiff";
import type { Finding } from "@/bindings/Finding";
import type { RuleConfig } from "@/bindings/RuleConfig";
import { cn } from "@/lib/utils";
import { CodeContextBlock } from "./code-context";
import {
  afterDecision,
  anchorText,
  droppedSummary,
  findFile,
  isEditableTarget,
  isFinal,
  progressCounts,
  progressLine,
  reviewKeyAction,
  stepId,
  undoTarget,
  type Decision,
} from "./logic";

function Kbd({ children, onPrimary }: { children: React.ReactNode; onPrimary?: boolean }) {
  return (
    <kbd
      className={cn(
        "rounded border border-b-2 px-1 font-mono text-[10.5px] font-medium leading-[1.35]",
        onPrimary ? "border-primary-foreground/40 bg-transparent text-inherit" : "bg-card text-muted-foreground",
      )}
    >
      {children}
    </kbd>
  );
}

/**
 * Variant 4B: one finding at a time. A approve, E edit, D dismiss, ←/→ prev/next, U undo.
 * A decision auto-advances to the next undecided finding.
 */
export function FindingsTab({
  list,
  dropped,
  files,
  currentId,
  setCurrentId,
  decide,
  history,
  rules,
  active,
  onOpenInDiff,
  post,
  report,
}: {
  /** Walkthrough findings (decision state applied), in walkthrough order. */
  list: Finding[];
  dropped: Finding[];
  files: FileDiff[];
  currentId: number | null;
  setCurrentId: (id: number | null) => void;
  decide: (f: Finding, to: Decision) => void;
  history: number[];
  rules?: RuleConfig[];
  active: boolean;
  onOpenInDiff: (f: Finding) => void;
  post: React.ReactNode;
  report?: React.ReactNode;
}) {
  const [editing, setEditing] = useState(false);
  const index = Math.max(0, list.findIndex((f) => f.id === currentId));
  const cur = list.length ? list[index] : undefined;
  const p = useMemo(() => progressCounts(list), [list]);

  // Stop editing when the card changes underneath (navigation, a new finding arriving first).
  useEffect(() => setEditing(false), [cur?.id]);

  const decideAndAdvance = (f: Finding, to: Decision) => {
    if (isFinal(f.status)) return;
    decide(f, to);
    if (to === "draft") return;
    const after = list.map((x) => (x.id === f.id ? { ...x, status: to } : x));
    setCurrentId(afterDecision(after, f.id));
  };

  const undo = () => {
    const t = undoTarget(list, cur?.id ?? null, history);
    const f = t == null ? undefined : list.find((x) => x.id === t);
    if (!f) return;
    decide(f, "draft");
    setCurrentId(f.id);
  };

  // Keyboard handler reads the latest state through a ref so the listener is attached once.
  const handler = useRef<(e: KeyboardEvent) => void>(() => {});
  handler.current = (e: KeyboardEvent) => {
    if (editing || e.defaultPrevented) return;
    const action = reviewKeyAction({ key: e.key, metaKey: e.metaKey, ctrlKey: e.ctrlKey, altKey: e.altKey, editable: isEditableTarget(e.target) });
    if (!action) return;
    // Leave keys alone while a dialog/menu is open elsewhere.
    if (document.querySelector("[role=dialog][data-state=open], [role=menu]")) return;
    e.preventDefault();
    switch (action) {
      case "approve":
        if (cur) decideAndAdvance(cur, "approved");
        break;
      case "dismiss":
        if (cur) decideAndAdvance(cur, "dismissed");
        break;
      case "edit":
        if (cur && !isFinal(cur.status)) setEditing(true);
        break;
      case "prev":
        setCurrentId(stepId(list, cur?.id ?? null, -1));
        break;
      case "next":
        setCurrentId(stepId(list, cur?.id ?? null, 1));
        break;
      case "undo":
        undo();
        break;
    }
  };
  useEffect(() => {
    const on = (e: KeyboardEvent) => handler.current(e);
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, []);

  const file = cur ? findFile(files, cur.file) : undefined;
  const decided = p.approved + p.dismissed + p.posted;
  const pct = p.total ? Math.round((decided / p.total) * 100) : 0;

  return (
    <div className="min-h-0 flex-1 overflow-y-auto">
      <div className="mx-auto grid w-full max-w-[760px] gap-3 px-4 py-4">
        {list.length > 0 && cur ? (
          <>
            <div className="flex items-center gap-3">
              <span className="min-w-0 flex-1 truncate font-mono text-2xs text-muted-foreground" aria-live="polite">
                {progressLine(index, p)}
              </span>
              <div className="h-1 w-40 overflow-hidden rounded-sm bg-surface-2" role="progressbar" aria-valuemin={0} aria-valuemax={p.total} aria-valuenow={decided} aria-label="Findings decided">
                <i className="block h-full bg-primary transition-[width]" style={{ width: `${pct}%` }} />
              </div>
            </div>

            <article aria-label={`${cur.severity} finding: ${cur.title}`} className="grid gap-2.5 rounded-xl border bg-card px-[18px] py-4 shadow-sm">
              {editing ? (
                <FindingEditor finding={cur} onDone={() => setEditing(false)} />
              ) : (
                <>
                  <div className="flex flex-wrap items-center gap-2">
                    <SeverityPill severity={cur.severity} />
                    <b className="min-w-0 flex-1 text-sm font-semibold">
                      <RichText text={cur.title} />
                    </b>
                    <StatusTag finding={cur} />
                    <span className="font-mono text-2xs text-muted-foreground">{findingMeta(cur, { rules })}</span>
                  </div>
                  <p className="text-[13px] leading-[1.55]">
                    <RichText text={cur.body} />
                  </p>
                  <CodeContextBlock file={file} finding={cur} onOpen={() => onOpenInDiff(cur)} />
                  {cur.suggestion && <SuggestionBlock suggestion={cur.suggestion} before={anchorText(file, cur)} />}
                  {cur.evidence && (
                    <div className="text-2xs text-muted-foreground">
                      <span className="font-semibold">Evidence:</span> <RichText text={cur.evidence} />
                    </div>
                  )}
                  <div className="flex flex-wrap items-center gap-2">
                    {isFinal(cur.status) ? (
                      <span className="text-xs text-muted-foreground">Already on the merge request. Nothing to decide.</span>
                    ) : (
                      <>
                        {cur.status === "approved" ? (
                          <Button onClick={() => decide(cur, "draft")} title="Undo approval (U)">
                            <Check /> Approved
                          </Button>
                        ) : (
                          <Button variant="default" onClick={() => decideAndAdvance(cur, "approved")}>
                            Approve <Kbd onPrimary>A</Kbd>
                          </Button>
                        )}
                        <Button onClick={() => setEditing(true)}>
                          Edit <Kbd>E</Kbd>
                        </Button>
                        {cur.status === "dismissed" ? (
                          <Button onClick={() => decide(cur, "draft")}>
                            <RotateCcw /> Restore <Kbd>U</Kbd>
                          </Button>
                        ) : (
                          <Button onClick={() => decideAndAdvance(cur, "dismissed")}>
                            Dismiss <Kbd>D</Kbd>
                          </Button>
                        )}
                        {cur.status === "approved" && (
                          <Button variant="ghost" onClick={() => decide(cur, "draft")}>
                            Undo <Kbd>U</Kbd>
                          </Button>
                        )}
                      </>
                    )}
                    <span className="flex-1" />
                    <Button variant="ghost" onClick={() => setCurrentId(stepId(list, cur.id, -1))} disabled={index === 0}>
                      ← Prev
                    </Button>
                    <Button variant="ghost" onClick={() => setCurrentId(stepId(list, cur.id, 1))} disabled={index >= list.length - 1}>
                      Skip →
                    </Button>
                  </div>
                </>
              )}
            </article>
            {p.undecided === 0 && !isFinal(cur.status) && (
              <div className="text-xs text-muted-foreground">
                Nothing left to decide.{p.approved > 0 ? ` ${p.approved} approved ${p.approved === 1 ? "note is" : "notes are"} ready to post.` : ""}
              </div>
            )}
          </>
        ) : (
          <div className="rounded-xl border bg-card px-4 py-8 text-center">
            <div className="text-sm font-semibold">{active ? "Claude is still reviewing" : "No findings to decide"}</div>
            <div className="mt-1 text-xs text-muted-foreground">
              {active ? "Findings appear here as they arrive." : dropped.length ? "Everything Claude raised was dropped by the guardrails. See the list below." : "This run raised nothing worth a note."}
            </div>
          </div>
        )}

        <DroppedFooter dropped={dropped} decide={decide} post={post} />
        {report}
        <div className="text-center text-2xs text-muted-foreground">
          <Kbd>A</Kbd> approve · <Kbd>E</Kbd> edit · <Kbd>D</Kbd> dismiss · <Kbd>←</Kbd> <Kbd>→</Kbd> move · <Kbd>U</Kbd> undo
        </div>
      </div>
    </div>
  );
}

function DroppedFooter({ dropped, decide, post }: { dropped: Finding[]; decide: (f: Finding, to: Decision) => void; post: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="grid gap-2">
      <div className="flex items-center gap-3">
        {dropped.length > 0 ? (
          <button
            type="button"
            className="flex min-w-0 flex-1 items-center gap-1 rounded text-left text-xs text-muted-foreground hover:text-foreground"
            aria-expanded={open}
            onClick={() => setOpen((v) => !v)}
          >
            {open ? <ChevronDown className="h-3.5 w-3.5 shrink-0" /> : <ChevronRight className="h-3.5 w-3.5 shrink-0" />}
            <span className="truncate">{droppedSummary(dropped)}</span>
          </button>
        ) : (
          <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">{droppedSummary(dropped)}</span>
        )}
        {post}
      </div>
      {open && dropped.length > 0 && (
        <ul className="grid gap-1.5" aria-label="Findings dropped by guardrails">
          {dropped.map((f) => (
            <li key={f.id} className="flex items-center gap-2 rounded-md border bg-card px-2.5 py-1.5 text-xs">
              <SeverityPill severity={f.severity} />
              <div className="min-w-0 flex-1">
                <div className="truncate">{f.title}</div>
                <div className="truncate font-mono text-2xs text-muted-foreground">
                  {f.file.split("/").pop()}:{f.line} · {f.guardrail_reason ?? "guardrail"}
                </div>
              </div>
              <Button size="sm" onClick={() => decide(f, "approved")}>
                Approve anyway
              </Button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
