import type { FileDiff } from "@/bindings/FileDiff";
import type { Finding } from "@/bindings/Finding";
import { lineMarker } from "@/lib/diff";
import { cn } from "@/lib/utils";
import { codeContext } from "./logic";

const ROW_BG = { add: "bg-diff-add", del: "bg-diff-del", context: "" } as const;

/** ±6 lines of the run's diff around a finding, with a file:line link that opens the Diff tab. */
export function CodeContextBlock({ file, finding: f, onOpen, radius = 6 }: { file: FileDiff | undefined; finding: Finding; onOpen?: () => void; radius?: number }) {
  const ctx = codeContext(file, f, radius);
  const loc = `${f.file}:${f.line}`;
  return (
    <div className="overflow-hidden rounded-md border bg-codebg font-mono text-xs">
      <div className="flex items-center gap-2 border-b bg-surface-2 px-2.5 py-1">
        {onOpen ? (
          <button type="button" onClick={onOpen} className="min-w-0 truncate rounded text-left text-primary hover:underline" title="Open in the Diff tab">
            {loc}
          </button>
        ) : (
          <span className="min-w-0 truncate text-muted-foreground">{loc}</span>
        )}
        {ctx && <span className="ml-auto truncate text-2xs text-muted-foreground">{ctx.header}</span>}
      </div>
      {ctx ? (
        <div className="overflow-x-auto">
          <table className="min-w-full border-collapse leading-[1.7]">
            <tbody>
              {ctx.lines.map((l, i) => {
                const hl = i === ctx.anchor;
                return (
                  <tr key={i} className={cn(ROW_BG[l.kind], hl && "bg-brand-soft shadow-[inset_3px_0_0_hsl(var(--brand))]")} aria-current={hl ? "true" : undefined}>
                    <td className="w-10 select-none px-2 text-right text-muted-foreground">{(f.side === "old" ? l.old_line : l.new_line) ?? l.old_line ?? ""}</td>
                    <td className="w-4 select-none text-muted-foreground">{lineMarker(l.kind)}</td>
                    <td className={cn("whitespace-pre pr-3", hl && "font-semibold")}>{l.text || " "}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ) : (
        <div className="px-2.5 py-2 font-sans text-2xs text-muted-foreground">
          {file ? `Line ${f.line} is not part of this run's diff.` : "This file is not part of this run's diff."}
        </div>
      )}
    </div>
  );
}
