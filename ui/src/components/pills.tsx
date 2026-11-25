import { Badge } from "@/components/ui/badge";
import type { CiStatus } from "@/bindings/CiStatus";
import type { PostingMode } from "@/bindings/PostingMode";
import type { RuleKind } from "@/bindings/RuleKind";
import type { RunStatus } from "@/bindings/RunStatus";
import type { Severity } from "@/bindings/Severity";
import { capitalize, ciStatusLabel, ciStatusTone, runStatusLabel, runStatusTone, severityTone } from "@/lib/format";
import { cn } from "@/lib/utils";

export function SeverityPill({ severity, count, className }: { severity: Severity; count?: number; className?: string }) {
  return (
    <Badge tone={severityTone(severity)} className={className}>
      {count ?? capitalize(severity)}
    </Badge>
  );
}

export function RunStatusPill({ status, children }: { status: RunStatus; children?: React.ReactNode }) {
  return <Badge tone={runStatusTone(status)}>{children ?? runStatusLabel(status)}</Badge>;
}

export function CiStatusPill({ status, children }: { status: CiStatus; children?: React.ReactNode }) {
  return <Badge tone={ciStatusTone(status)}>{children ?? ciStatusLabel(status)}</Badge>;
}

export function ModePill({ mode }: { mode: PostingMode }) {
  return <Badge tone={mode === "draft" ? "grey" : "blue"}>{capitalize(mode)}</Badge>;
}

export function RuleKindPill({ kind }: { kind: RuleKind }) {
  return <Badge tone={kind === "claude" ? "blue" : "grey"}>{kind === "claude" ? "Claude" : kind}</Badge>;
}

/** Coloured dot + label, like the mockup's title-bar `.ok` / `.warnd` items. */
export function StatusDot({ ok, warn, children, className }: { ok?: boolean; warn?: boolean; children: React.ReactNode; className?: string }) {
  return (
    <span className={cn("inline-flex items-center gap-1.5 whitespace-nowrap", className)}>
      <span
        aria-hidden
        className={cn("inline-block h-[7px] w-[7px] shrink-0 rounded-full", ok ? "bg-green" : warn ? "bg-orange" : "bg-red")}
      />
      {children}
    </span>
  );
}
