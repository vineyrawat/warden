// Posting tab: posting mode cards, auto-post rules (Assist), note budgets, summary note.
import { useRef } from "react";

import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { Segmented } from "@/components/segmented";
import type { MrStatusMode } from "@/bindings/MrStatusMode";
import type { PostingMode } from "@/bindings/PostingMode";
import type { Severity } from "@/bindings/Severity";
import { POSTING_MODES, SEVERITIES, capitalize, modeDescription, modeLabel } from "@/lib/format";
import { cn } from "@/lib/utils";
import { ControlRow, clampInt } from "./bits";
import { sortSeverities, type Draft } from "./logic";

type Set = <K extends keyof Draft>(k: K, v: Draft[K]) => void;

const MODE_TAG: Record<PostingMode, string> = { draft: "you approve every note", assist: "safe notes post on their own", auto: "no approval step" };

export function PostingTab({ draft, set, paused }: { draft: Draft; set: Set; paused: boolean }) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const idx = POSTING_MODES.indexOf(draft.mode);
  const move = (d: number) => {
    const next = (idx + d + POSTING_MODES.length) % POSTING_MODES.length;
    set("mode", POSTING_MODES[next]);
    refs.current[next]?.focus();
  };
  const assist = draft.mode === "assist";
  const toggleSeverity = (s: Severity, on: boolean) => set("autoSeverities", sortSeverities(on ? [...draft.autoSeverities, s] : draft.autoSeverities.filter((x) => x !== s)));

  return (
    <div className="grid gap-3.5">
      {paused && (
        <div className="rounded-md bg-orange-soft px-2.5 py-1.5 text-xs text-orange">
          Posting is paused for every project (Settings → Advanced). Reviews still run and wait in the approval queue.
        </div>
      )}
      <section className="rounded-lg border bg-card px-3.5 py-3">
        <h3 id="g-mode" className="mb-2 text-[12.5px] font-semibold">
          Posting mode
        </h3>
        <div
          role="radiogroup"
          aria-labelledby="g-mode"
          className="grid gap-2 @xl:grid-cols-3"
          onKeyDown={(e) => {
            if (e.key === "ArrowRight" || e.key === "ArrowDown") {
              e.preventDefault();
              move(1);
            } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
              e.preventDefault();
              move(-1);
            }
          }}
        >
          {POSTING_MODES.map((m, i) => {
            const on = m === draft.mode;
            return (
              <button
                key={m}
                ref={(el) => {
                  refs.current[i] = el;
                }}
                type="button"
                role="radio"
                aria-checked={on}
                tabIndex={on ? 0 : -1}
                onClick={() => set("mode", m)}
                className={cn(
                  "flex flex-col items-start gap-1 rounded-md border px-3 py-2.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                  on ? "border-primary bg-brand-soft" : "bg-card hover:bg-accent",
                )}
              >
                <span className="flex w-full items-center gap-2">
                  <span className={cn("inline-block h-3 w-3 shrink-0 rounded-full border-[1.5px]", on ? "border-primary bg-primary shadow-[inset_0_0_0_2px_hsl(var(--surface))]" : "border-muted-foreground")} aria-hidden />
                  <span className="text-[12.5px] font-semibold">{modeLabel(m)}</span>
                </span>
                <span className="text-2xs font-medium text-foreground/80">{MODE_TAG[m]}</span>
                <span className="text-2xs leading-snug text-muted-foreground">{modeDescription(m, draft.autoSeverities, draft.autoPostConfidence).replace(/^\w+: /, "")}</span>
              </button>
            );
          })}
        </div>
      </section>

      <section className={cn("rounded-lg border bg-card px-3.5 py-3", !assist && "opacity-70")}>
        <div className="mb-1 flex items-center gap-2">
          <h3 className="text-[12.5px] font-semibold">Auto-post rules</h3>
          <span className="text-2xs text-muted-foreground">{assist ? "used in Assist mode" : "only used in Assist mode"}</span>
        </div>
        <ControlRow label="Severities that post on their own" hint="Everything else waits in the approval queue">
          <div className="flex gap-1" role="group" aria-label="Auto-post severities">
            {SEVERITIES.map((s) => {
              const on = draft.autoSeverities.includes(s);
              return (
                <button
                  key={s}
                  type="button"
                  aria-pressed={on}
                  onClick={() => toggleSeverity(s, !on)}
                  className={cn("rounded-full border px-2 py-0.5 text-2xs font-medium", on ? "border-primary bg-primary text-primary-foreground" : "bg-card text-muted-foreground hover:bg-accent")}
                >
                  {capitalize(s)}
                </button>
              );
            })}
          </div>
        </ControlRow>
        <ControlRow label="Minimum confidence to auto-post">
          <Slider className="w-[130px]" min={0.5} max={1} step={0.01} value={[draft.autoPostConfidence]} onValueChange={([v]) => set("autoPostConfidence", v)} aria-label="Minimum confidence to auto-post" />
          <span className="w-9 text-right font-mono text-xs">{draft.autoPostConfidence.toFixed(2)}</span>
        </ControlRow>
      </section>

      <section className="rounded-lg border bg-card px-3.5 py-3">
        <h3 className="mb-1 text-[12.5px] font-semibold">Note budget</h3>
        <ControlRow label="Notes per MR" hint="Most severe, most confident first; the rest are dropped" htmlFor="g-max-notes">
          <Input id="g-max-notes" type="number" min={1} max={50} className="w-20" value={draft.maxNotes} onChange={(e) => set("maxNotes", clampInt(e.target.value, 1, 50, draft.maxNotes))} />
        </ControlRow>
        <ControlRow label="Notes per hour" hint="Counts every note Warden posted in the last hour; notes over the limit are skipped" htmlFor="g-max-hour">
          <Input id="g-max-hour" type="number" min={1} max={500} className="w-20" value={draft.maxNotesPerHour} onChange={(e) => set("maxNotesPerHour", clampInt(e.target.value, 1, 500, draft.maxNotesPerHour))} />
        </ControlRow>
        <ControlRow label="Summary note" hint="One note on the MR with the findings table and CI results">
          <Switch checked={draft.summaryNote} onCheckedChange={(v) => set("summaryNote", v)} aria-label="Post summary note" />
        </ControlRow>
      </section>

      <section className="rounded-lg border bg-card px-3.5 py-3">
        <h3 className="text-[12.5px] font-semibold">MR status after posting</h3>
        <p className="mb-2 text-2xs text-muted-foreground">
          Runs right after Warden posts notes. In Draft mode that only happens when you click Post. In Assist and Auto, a review with nothing to post can still approve the MR; it never requests changes without a note to point at.
        </p>
        <Segmented<MrStatusMode>
          label="MR status after posting"
          value={draft.mrStatusMode}
          onChange={(v) => set("mrStatusMode", v)}
          options={[
            { value: "off", label: "Off" },
            { value: "labels", label: "Labels" },
            { value: "review", label: "Approve / request changes" },
          ]}
        />
        <p className="mt-1.5 text-2xs leading-snug text-muted-foreground">
          {draft.mrStatusMode === "off"
            ? "The MR's status is left alone; only notes, the summary and the ai-reviewed label are posted."
            : draft.mrStatusMode === "labels"
              ? "Adds warden::approved or warden::changes-requested (and removes the other). Works on every GitLab and GitHub."
              : "GitHub: submits an Approve or Request changes review. GitLab: approves the MR, or withdraws Warden's approval and adds warden::changes-requested (GitLab's API has no request-changes call). The token's user must be allowed to approve."}
        </p>
        <div className={cn("mt-2", draft.mrStatusMode === "off" && "pointer-events-none opacity-50")}>
          <ControlRow label="Request changes when a note is" hint="A posted or still pending finding at these severities, or Claude's verdict, means changes requested; otherwise the MR is approved">
            <div className="flex gap-1">
              {SEVERITIES.map((sv) => {
                const on = draft.requestChangesOn.includes(sv);
                return (
                  <button
                    key={sv}
                    type="button"
                    aria-pressed={on}
                    disabled={draft.mrStatusMode === "off"}
                    onClick={() => set("requestChangesOn", sortSeverities(on ? draft.requestChangesOn.filter((x) => x !== sv) : [...draft.requestChangesOn, sv]))}
                    className={cn("rounded-full border px-2 py-0.5 text-2xs font-medium", on ? "border-primary bg-primary text-primary-foreground" : "bg-card text-muted-foreground hover:bg-accent")}
                  >
                    {capitalize(sv)}
                  </button>
                );
              })}
            </div>
          </ControlRow>
        </div>
      </section>
    </div>
  );
}
