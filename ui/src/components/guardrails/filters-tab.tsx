// Filters tab: which findings survive (confidence, severity floor, nits) and what Claude looks at
// (focus areas, ignored paths).
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import type { Severity } from "@/bindings/Severity";
import { SEVERITIES, capitalize } from "@/lib/format";
import { ControlRow, TagListInput } from "./bits";
import type { Draft } from "./logic";

type Set = <K extends keyof Draft>(k: K, v: Draft[K]) => void;

export function FiltersTab({ draft, set }: { draft: Draft; set: Set }) {
  return (
    <div className="grid gap-3.5">
      <section className="rounded-lg border bg-card px-3.5 py-3">
        <h3 className="mb-1 text-[12.5px] font-semibold">Which findings are kept</h3>
        <ControlRow label="Minimum confidence" hint="Findings below this are dropped. Regex and semgrep rule hits are exempt.">
          <Slider className="w-[130px]" min={0} max={1} step={0.05} value={[draft.minConfidence]} onValueChange={([v]) => set("minConfidence", v)} aria-label="Minimum confidence" />
          <span className="w-9 text-right font-mono text-xs">{draft.minConfidence.toFixed(2)}</span>
        </ControlRow>
        <ControlRow label="Lowest severity posted" hint="Anything less severe is dropped">
          <Select value={draft.severityFloor} onValueChange={(v) => set("severityFloor", v as Severity)}>
            <SelectTrigger className="w-[120px]" aria-label="Lowest severity posted">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {SEVERITIES.map((sv) => (
                <SelectItem key={sv} value={sv}>
                  {capitalize(sv)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </ControlRow>
        <ControlRow label="Post nits" hint="Style and naming remarks">
          <Switch checked={draft.nits} onCheckedChange={(v) => set("nits", v)} aria-label="Post nits" />
        </ControlRow>
      </section>

      <section className="rounded-lg border bg-card px-3.5 py-3">
        <h3 className="text-[12.5px] font-semibold">What Claude looks at</h3>
        <p className="mb-1 text-2xs text-muted-foreground">Applies from the next review. The preview drops findings in ignored paths; focus areas only change what Claude looks for next time.</p>
        <div className="border-b border-dashed py-2">
          <div className="text-[12.5px]">Focus areas</div>
          <div className="mb-1.5 text-2xs text-muted-foreground">Categories Claude is told to prioritise</div>
          <TagListInput label="Add focus area" placeholder="Add area…" values={draft.focus} onChange={(v) => set("focus", v)} />
        </div>
        <div className="py-2">
          <div className="text-[12.5px]">Ignored paths</div>
          <div className="mb-1.5 text-2xs text-muted-foreground">Globs left out of the diff Claude reviews and of rule checks</div>
          <TagListInput label="Add ignored path" placeholder="e.g. **/migrations/**" mono values={draft.ignorePaths} onChange={(v) => set("ignorePaths", v)} />
        </div>
      </section>
    </div>
  );
}
