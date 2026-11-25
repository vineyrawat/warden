// Small building blocks shared by the Settings panes.
import { useEffect, useState } from "react";
import { Check, Copy } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** Copies text with the async Clipboard API, falling back to a hidden textarea + execCommand. */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    /* fall through to the textarea fallback */
  }
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "fixed";
    ta.style.top = "-1000px";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    return ok;
  } catch {
    return false;
  }
}

export function CopyButton({ text, label = "Copy", className }: { text: string; label?: string; className?: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(t);
  }, [copied]);
  return (
    <Button
      size="sm"
      className={className}
      aria-label={copied ? "Copied" : `${label}: ${text}`}
      onClick={async () => {
        if (await copyText(text)) setCopied(true);
        else toast.error("Copy failed. Select the text and copy it by hand.");
      }}
    >
      {copied ? <Check /> : <Copy />} {copied ? "Copied" : label}
    </Button>
  );
}

/** A command line in a code box with a copy button (the checklist's "exact fix"). */
export function CommandLine({ command }: { command: string }) {
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-1.5">
      <code className="min-w-0 select-all break-all rounded border bg-card px-2 py-0.5 font-mono text-xs">{command}</code>
      <CopyButton text={command} />
    </span>
  );
}

/** Rounded settings box (mockup `.pane`). */
export function Pane({ title, children, className, actions }: { title?: React.ReactNode; children: React.ReactNode; className?: string; actions?: React.ReactNode }) {
  return (
    <section className={cn("grid content-start gap-1 rounded-[10px] border bg-card px-3.5 py-3", className)}>
      {(title || actions) && (
        <div className="flex items-center gap-2 pb-1">
          {title && <h2 className="text-[12.5px] font-semibold">{title}</h2>}
          {actions && <div className="ml-auto flex items-center gap-2">{actions}</div>}
        </div>
      )}
      <div className="grid min-w-0 gap-0">{children}</div>
    </section>
  );
}

/** Label / value row with a dashed divider (mockup `.kv`). */
export function Kv({ label, hint, children, htmlFor }: { label: React.ReactNode; hint?: React.ReactNode; children?: React.ReactNode; htmlFor?: string }) {
  const L = htmlFor ? "label" : "span";
  return (
    <div className="flex min-h-[38px] items-center justify-between gap-3 border-t border-dashed py-2 text-[12.5px] first:border-t-0">
      <div className="min-w-0">
        <L htmlFor={htmlFor} className="block">
          {label}
        </L>
        {hint && <small className="block text-2xs text-muted-foreground">{hint}</small>}
      </div>
      <div className="flex min-w-0 shrink-0 items-center gap-1.5">{children}</div>
    </div>
  );
}

export function PaneNote({ children }: { children: React.ReactNode }) {
  return <p className="text-2xs text-muted-foreground">{children}</p>;
}

/** Centered column used by every pane except Projects. */
export function PaneColumn({ children }: { children: React.ReactNode }) {
  return <div className="mx-auto grid w-full max-w-[680px] content-start gap-3.5 px-6 py-[18px]">{children}</div>;
}
