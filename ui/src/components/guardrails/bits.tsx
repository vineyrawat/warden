// Small building blocks for the Guardrails panel: debounce hook, "Always on" card, tag list input.
import { useEffect, useState } from "react";
import { Lock, X } from "lucide-react";

import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

export function useDebounced<T>(value: T, ms = 300): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

/** Guarantees enforced by warden-core regardless of settings (see guardrails.rs, engine/mod.rs, poster.rs). */
const ALWAYS_ON: { label: string; hint: string }[] = [
  { label: "Anchor on a changed line", hint: "Notes must point at a line this MR added or removed." },
  { label: "Drop duplicates", hint: "Same file, title and rule as an earlier note (fingerprint), or a near-copy of an open thread." },
  { label: "Rule id or evidence required", hint: "Every finding cites a rule or quotes the code it is about." },
  { label: "Tone check", hint: "Hedged notes (\"might\", \"maybe\") without evidence are sent back to Claude." },
  { label: "Redact secrets", hint: "Secrets are masked in the diff sent to Claude and in every posted note." },
  { label: "Config from the base commit", hint: ".warden.yml is read from the target branch, so an MR cannot loosen its own review." },
  { label: "No shell or network for Claude", hint: "Claude reads the repo through Warden's tools only; CI runs through your recipes." },
  { label: "No forge write access for Claude", hint: "Claude never sees a token. Warden posts the notes." },
];

export function AlwaysOnCard({ className }: { className?: string }) {
  return (
    <section aria-labelledby="g-always-on" className={cn("rounded-lg bg-green-soft px-3.5 py-3", className)}>
      <div className="mb-1 flex items-center gap-2">
        <h3 id="g-always-on" className="text-[12.5px] font-semibold">
          Always on
        </h3>
        <span className="text-2xs text-muted-foreground">enforced for every project, cannot be turned off</span>
      </div>
      <ul className="grid gap-x-4 @lg:grid-cols-2">
        {ALWAYS_ON.map((a) => (
          <li key={a.label} className="flex items-start gap-2 border-b border-dashed border-green/20 py-1.5 last:border-b-0 @lg:[&:nth-last-child(2)]:border-b-0">
            <div className="min-w-0 flex-1">
              <div className="text-xs">{a.label}</div>
              <div className="text-2xs text-muted-foreground">{a.hint}</div>
            </div>
            <span className="mt-0.5 inline-flex shrink-0 items-center gap-1 text-2xs font-medium text-green">
              <Lock className="h-3 w-3" aria-hidden /> locked
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

/** Chips with a remove button plus an input that adds on Enter or comma. */
export function TagListInput({ values, onChange, placeholder, label, mono }: { values: string[]; onChange: (v: string[]) => void; placeholder: string; label: string; mono?: boolean }) {
  const [text, setText] = useState("");
  const commit = () => {
    const add = text
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s && !values.includes(s));
    if (add.length) onChange([...values, ...new Set(add)]);
    setText("");
  };
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {values.map((v) => (
        <span key={v} className={cn("inline-flex items-center gap-1 rounded-full border bg-surface-2 py-0.5 pl-2 pr-1 text-2xs", mono && "font-mono")}>
          {v}
          <button type="button" className="rounded-full p-0.5 text-muted-foreground hover:bg-accent hover:text-foreground" onClick={() => onChange(values.filter((x) => x !== v))} aria-label={`Remove ${v}`}>
            <X className="h-3 w-3" />
          </button>
        </span>
      ))}
      <Input
        aria-label={label}
        className={cn("h-6 w-40 text-xs", mono && "font-mono")}
        value={text}
        placeholder={placeholder}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === ",") {
            e.preventDefault();
            commit();
          } else if (e.key === "Backspace" && !text && values.length) {
            onChange(values.slice(0, -1));
          }
        }}
      />
    </div>
  );
}

/** Label + hint on the left, control on the right (like layout `Row`, with a wider hint). */
export function ControlRow({ label, hint, children, htmlFor, className }: { label: React.ReactNode; hint?: React.ReactNode; children: React.ReactNode; htmlFor?: string; className?: string }) {
  const L = htmlFor ? "label" : "span";
  return (
    <div className={cn("flex min-h-[38px] items-center justify-between gap-3 border-b border-dashed py-2 last:border-b-0", className)}>
      <div className="min-w-0">
        <L htmlFor={htmlFor} className="block text-[12.5px]">
          {label}
        </L>
        {hint && <div className="text-2xs text-muted-foreground">{hint}</div>}
      </div>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
    </div>
  );
}

export const clampInt = (v: string, min: number, max: number, fallback: number) => {
  const n = Number.parseInt(v, 10);
  return Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : fallback;
};
