// Inline editor for a pending finding's title, body and suggestion (used in the list and the
// preview pane). Esc cancels, ⌘Enter / Ctrl+Enter saves.
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import type { Finding } from "@/bindings/Finding";
import { qk, useUpdateFinding } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { Kbd, MOD, SevChip } from "./bits";

export function QueueEditor({ finding: f, onDone, className }: { finding: Finding; onDone: () => void; className?: string }) {
  const [title, setTitle] = useState(f.title);
  const [body, setBody] = useState(f.body);
  const [suggestion, setSuggestion] = useState(f.suggestion ?? "");
  const update = useUpdateFinding();
  const qc = useQueryClient();
  const titleRef = useRef<HTMLInputElement>(null);
  useEffect(() => titleRef.current?.focus({ preventScroll: true }), []);
  const valid = title.trim().length > 0 && body.trim().length > 0;
  const changed = title !== f.title || body !== f.body || suggestion !== (f.suggestion ?? "");

  const save = () => {
    if (!valid || update.isPending) return;
    if (!changed) return onDone();
    update.mutate(
      { id: f.id, runId: f.run_id, title: title.trim(), body: body.trim(), suggestion: suggestion.trim() ? suggestion : null },
      {
        onSuccess: () => {
          qc.invalidateQueries({ queryKey: qk.pendingFindings });
          qc.invalidateQueries({ queryKey: qk.previewNote(f.id) });
          onDone();
        },
      },
    );
  };

  return (
    <form
      className={cn("space-y-2", className)}
      aria-label={`Edit finding: ${f.title}`}
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
        <SevChip severity={f.severity} />
        <Input ref={titleRef} aria-label="Finding title" value={title} onChange={(e) => setTitle(e.target.value)} className="font-sans font-semibold" />
      </div>
      <Textarea aria-label="Finding body" rows={4} value={body} onChange={(e) => setBody(e.target.value)} className="font-sans" />
      <Textarea
        aria-label="Suggested code"
        rows={2}
        placeholder="Suggested code (optional, posted as a suggestion block)"
        value={suggestion}
        onChange={(e) => setSuggestion(e.target.value)}
        className="font-mono text-[11.5px]"
      />
      <div className="flex flex-wrap items-center gap-1.5">
        <Button size="sm" type="submit" variant="default" disabled={!valid || update.isPending}>
          {update.isPending ? "Saving…" : "Save"}
        </Button>
        <Button size="sm" onClick={onDone}>
          Cancel
        </Button>
        <span className="ml-auto text-2xs text-muted-foreground">
          <Kbd>{MOD}↵</Kbd> save · <Kbd>Esc</Kbd> cancel
        </span>
      </div>
    </form>
  );
}
