import { useState } from "react";
import { Copy, FileCode } from "lucide-react";

import { Button } from "@/components/ui/button";
import { copyText } from "@/components/ci/pipeline-graph";
import { failureLocation, matchDiffPath, type FailureKind, type ParsedFailure } from "@/lib/ci-parse";
import { useNav } from "@/lib/nav";
import { cn } from "@/lib/utils";

const KIND_DOT: Record<FailureKind, string> = { test: "bg-red", type: "bg-red", lint: "bg-orange", secret: "bg-red", scan: "bg-orange" };
const LIMIT = 6;

/** Failures parsed from the job log, each linking into Review when its file is in the MR diff. */
export function ParsedFailures({ failures, diffPaths, runId }: { failures: ParsedFailure[]; diffPaths: string[]; runId: number }) {
  const { navigate } = useNav();
  const [all, setAll] = useState(false);
  if (failures.length === 0) return null;
  const shown = all ? failures : failures.slice(0, LIMIT);

  return (
    <div className="mx-4 mt-3 shrink-0 overflow-hidden rounded-lg border bg-card" aria-label="Parsed failures">
      <ul className="max-h-[220px] overflow-y-auto">
        {shown.map((f, i) => {
          const inDiff = matchDiffPath(f.file, diffPaths);
          const title = f.test ?? (f.code ? `${f.code} ${f.message}` : f.message);
          const sub = f.test ? f.message : null;
          return (
            <li key={i} className="grid gap-[3px] border-t px-3 py-2 text-[12.5px] first:border-t-0">
              <div className="flex min-w-0 items-center gap-2">
                <span aria-hidden className={cn("inline-block h-2 w-2 shrink-0 rounded-full", KIND_DOT[f.kind])} />
                <b className={cn("min-w-0 flex-1 truncate font-semibold", !f.test && "font-mono text-xs font-medium")} title={title}>
                  {title}
                </b>
                {f.file && (
                  <span className="max-w-[40%] shrink truncate font-mono text-[11.5px] text-muted-foreground" title={failureLocation(f)}>
                    {failureLocation(f)}
                  </span>
                )}
                {inDiff ? (
                  <Button size="sm" onClick={() => navigate({ screen: "review", runId, tab: "diff" })} title={`Open ${inDiff} in Review`}>
                    <FileCode /> Open in review
                  </Button>
                ) : f.file ? (
                  <Button size="sm" variant="ghost" onClick={() => copyText(failureLocation(f), "Path copied")} title="Not part of this MR's diff">
                    <Copy /> Copy path
                  </Button>
                ) : null}
              </div>
              {sub && <span className="truncate pl-4 font-mono text-[11.5px] text-muted-foreground" title={sub}>{sub}</span>}
            </li>
          );
        })}
      </ul>
      {failures.length > LIMIT && (
        <button type="button" onClick={() => setAll((v) => !v)} className="w-full border-t px-3 py-1.5 text-left text-xs text-brand hover:bg-accent">
          {all ? "Show fewer" : `Show all ${failures.length} failures`}
        </button>
      )}
    </div>
  );
}
