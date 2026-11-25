// Small display pieces shared by the Inbox list and preview (mockup `.sev`, `.sdot`, `.cell`).
import type { CiStatus } from "@/bindings/CiStatus";
import type { Severity } from "@/bindings/Severity";
import { cn } from "@/lib/utils";
import type { DotTone } from "./inbox-logic";

const SEV_CLASS: Record<Severity, string> = {
  blocker: "bg-red-soft text-red",
  major: "bg-orange-soft text-orange",
  minor: "bg-yellow-soft text-yellow",
  nit: "bg-surface-2 text-muted-foreground",
};

/** Square-ish mono severity chip; shows a count or the severity word. */
export function SevChip({ severity, count, className }: { severity: Severity; count?: number; className?: string }) {
  return (
    <span
      className={cn("whitespace-nowrap rounded px-1.5 font-mono text-[10px] font-semibold uppercase leading-[1.6] tracking-[.03em]", SEV_CLASS[severity], className)}
      title={count != null ? `${count} ${severity}` : undefined}
    >
      {count ?? severity}
    </span>
  );
}

const DOT_CLASS: Record<DotTone, string> = {
  green: "bg-green",
  red: "bg-red",
  blue: "bg-brand",
  orange: "bg-orange",
  none: "bg-line",
};

export function Dot({ tone, label, className }: { tone: DotTone; label: string; className?: string }) {
  return <span role="img" aria-label={label} title={label} className={cn("inline-block h-2 w-2 shrink-0 rounded-full", DOT_CLASS[tone], className)} />;
}

function cellClass(s: CiStatus): string {
  switch (s) {
    case "passed":
      return "bg-green-soft text-green";
    case "failed":
    case "timed_out":
      return "bg-red-soft text-red";
    case "running":
    case "pending":
      return "bg-brand-soft text-brand";
    default:
      return "bg-surface-2 text-muted-foreground";
  }
}

/** CI job chip, e.g. "lint 12s" or "semgrep not installed". */
export function JobCell({ status, children, onClick, title }: { status: CiStatus; children: React.ReactNode; onClick?: () => void; title?: string }) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className={cn("inline-flex items-center gap-1.5 rounded-[5px] px-2 py-0.5 font-mono text-2xs font-medium hover:opacity-80", cellClass(status))}
    >
      {children}
    </button>
  );
}
