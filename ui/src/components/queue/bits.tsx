// Small display pieces for the Approval queue (mockup `.sev`, `.kbd`, `.pill`).
import type { Severity } from "@/bindings/Severity";
import { cn } from "@/lib/utils";

const SEV_CLASS: Record<Severity, string> = {
  blocker: "bg-red-soft text-red",
  major: "bg-orange-soft text-orange",
  minor: "bg-yellow-soft text-yellow",
  nit: "bg-surface-2 text-muted-foreground",
};

/** Mono severity chip with the severity word or a count. */
export function SevChip({ severity, count, className }: { severity: Severity; count?: number; className?: string }) {
  return (
    <span className={cn("inline-block whitespace-nowrap rounded px-1.5 font-mono text-[10px] font-semibold uppercase leading-[1.6] tracking-[.03em]", SEV_CLASS[severity], className)}>
      {count ?? severity}
    </span>
  );
}

export function Kbd({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <kbd className={cn("rounded border border-b-2 bg-card px-[5px] font-mono text-[10px] font-medium leading-[1.45] text-muted-foreground", className)}>
      {children}
    </kbd>
  );
}

export const IS_MAC = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
export const MOD = IS_MAC ? "⌘" : "Ctrl+";
