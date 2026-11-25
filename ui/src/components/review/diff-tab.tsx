import { useEffect, useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger } from "@/components/ui/select";
import { SeverityPill } from "@/components/pills";
import { DiffView } from "@/components/diff-view";
import { EmptyState } from "@/components/layout";
import type { FileDiff } from "@/bindings/FileDiff";
import type { Finding } from "@/bindings/Finding";
import type { RuleConfig } from "@/bindings/RuleConfig";
import { filePath, fileStats, maxSeverity } from "@/lib/format";
import { diffOrder, nextUndecidedId, progressCounts, type Decision } from "./logic";

/**
 * Variant 4A: full-width unified diff with findings as inline threads, a file picker, a severity
 * strip on the right edge and a floating bar ("1 approved · 3 to decide  Next ↓  Post 1 note").
 */
export function DiffTab({
  files,
  findings,
  focusId,
  setFocusId,
  decide,
  rules,
  active,
  post,
  report,
}: {
  /** Diff files plus placeholders for findings outside the diff (see `reviewFiles`). */
  files: FileDiff[];
  /** Kept findings (decision state applied). */
  findings: Finding[];
  focusId: number | null;
  setFocusId: (id: number | null) => void;
  decide: (f: Finding, to: Decision) => void;
  rules?: RuleConfig[];
  active: boolean;
  post: React.ReactNode;
  report?: React.ReactNode;
}) {
  const ordered = useMemo(() => diffOrder(files, findings), [files, findings]);
  const [path, setPath] = useState<string | null>(() => {
    const f = findings.find((x) => x.id === focusId);
    return f?.file ?? ordered[0]?.file ?? (files[0] ? filePath(files[0]) : null);
  });

  // Follow the focused finding into its file; pick a file once the diff arrives.
  useEffect(() => {
    const f = focusId == null ? undefined : findings.find((x) => x.id === focusId);
    if (f && f.file !== path) setPath(f.file);
    // Only when the focus moves; switching files by hand clears the focus.
  }, [focusId]);
  useEffect(() => {
    if (path == null && files[0]) setPath(ordered[0]?.file ?? filePath(files[0]));
  }, [files, ordered, path]);

  const fileIndex = files.findIndex((f) => filePath(f) === path);
  const file = fileIndex >= 0 ? files[fileIndex] : undefined;
  const fileFindings = useMemo(() => findings.filter((f) => f.file === path), [findings, path]);
  const p = progressCounts(findings);
  const nextId = nextUndecidedId(ordered, focusId);

  if (!file) {
    return (
      <EmptyState title={active ? "Review in progress" : "Nothing to show"} className="flex-1">
        {active ? "The diff appears here as soon as the worktree is ready." : "This run has no diff files."}
      </EmptyState>
    );
  }

  const stats = fileStats(file);
  const header = (
    <div className="flex shrink-0 flex-wrap items-center gap-2.5 border-b bg-surface px-3 py-1.5">
      <Select
        value={path ?? undefined}
        onValueChange={(v) => {
          setPath(v);
          setFocusId(null);
        }}
      >
        <SelectTrigger className="h-7 w-auto max-w-[360px] gap-2 rounded-full bg-surface-2 px-3" aria-label="Changed file">
          <span className="truncate">{(path ?? "").split("/").pop()}</span>
          <span className="shrink-0 font-sans text-2xs text-muted-foreground">
            {fileIndex + 1} of {files.length} files
          </span>
        </SelectTrigger>
        <SelectContent className="max-w-[560px]">
          {files.map((f) => {
            const fp = filePath(f);
            const mine = findings.filter((x) => x.file === fp && x.status !== "dismissed");
            const sev = maxSeverity(mine);
            return (
              <SelectItem key={fp} value={fp} className="font-mono">
                <span className="flex items-center gap-2">
                  <span className="truncate">{fp}</span>
                  {sev && <SeverityPill severity={sev} count={mine.length} />}
                </span>
              </SelectItem>
            );
          })}
        </SelectContent>
      </Select>
      <span className="min-w-0 flex-1 truncate font-mono text-xs font-semibold">{filePath(file)}</span>
      {file.old_path !== file.new_path && !file.is_new && !file.is_deleted && <span className="truncate font-mono text-2xs text-muted-foreground">from {file.old_path}</span>}
      <span className="font-mono text-xs text-green">+{stats.additions}</span>
      <span className="font-mono text-xs text-red">−{stats.deletions}</span>
      {file.is_new && <Badge tone="green">new file</Badge>}
      {file.is_deleted && <Badge tone="red">deleted</Badge>}
    </div>
  );

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      <DiffView
        file={file}
        findings={fileFindings}
        focusFindingId={focusId}
        onDecide={decide}
        onFocusFinding={setFocusId}
        rules={rules}
        header={header}
        bottomPad
      />
      <div className="pointer-events-none absolute inset-x-0 bottom-3.5 flex flex-col items-center gap-2 px-4">
        {report && <div className="pointer-events-auto w-full max-w-[560px]">{report}</div>}
        {findings.length > 0 && (
          <div className="pointer-events-auto flex max-w-full items-center gap-3 whitespace-nowrap rounded-[10px] bg-foreground py-1.5 pl-3.5 pr-2 text-[12.5px] text-background shadow-lg">
            <span>
              {p.approved} approved · {p.undecided} to decide
            </span>
            <Button
              variant="ghost"
              className="text-background hover:bg-background/15 hover:text-background"
              disabled={nextId == null}
              title={nextId == null ? "Nothing left to decide" : "Next undecided finding"}
              onClick={() => nextId != null && setFocusId(nextId)}
            >
              Next ↓
            </Button>
            {post}
          </div>
        )}
      </div>
    </div>
  );
}
