import { cn } from "@/lib/utils";

/** Screen sub-header, like the mockup window title (`.t`) with right-aligned meta (`.st`). */
export function ScreenHeader({ title, meta, actions, className }: { title: React.ReactNode; meta?: React.ReactNode; actions?: React.ReactNode; className?: string }) {
  return (
    <div className={cn("flex min-h-[41px] items-center gap-3 border-b bg-surface px-4 py-2", className)}>
      <h1 className="min-w-0 truncate text-[13px] font-semibold">{title}</h1>
      {meta && <div className="ml-auto flex min-w-0 items-center gap-3.5 text-xs text-muted-foreground">{meta}</div>}
      {actions && <div className={cn("flex items-center gap-2", !meta && "ml-auto")}>{actions}</div>}
    </div>
  );
}

/** Bordered section with a small heading (mockup `.fs`). */
export function Fieldset({ title, children, className, actions }: { title: React.ReactNode; children: React.ReactNode; className?: string; actions?: React.ReactNode }) {
  return (
    <section className={cn("min-w-0 rounded-lg border bg-card px-3.5 py-3", className)}>
      <div className="mb-2 flex items-center gap-2">
        <h2 className="text-[13px] font-semibold">{title}</h2>
        {actions && <div className="ml-auto flex items-center gap-2">{actions}</div>}
      </div>
      {children}
    </section>
  );
}

/** Label / control row with a dashed divider (mockup `.row`). */
export function Row({ label, hint, children, labelMuted, htmlFor }: { label: React.ReactNode; hint?: React.ReactNode; children?: React.ReactNode; labelMuted?: boolean; htmlFor?: string }) {
  const L = htmlFor ? "label" : "span";
  return (
    <div className="flex min-h-[34px] items-center justify-between gap-2.5 border-b border-dashed py-[7px] last:border-b-0">
      <div className="min-w-0">
        <L htmlFor={htmlFor} className={cn("block", labelMuted && "text-xs text-muted-foreground")}>
          {label}
        </L>
        {hint && <div className="text-2xs text-muted-foreground">{hint}</div>}
      </div>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
    </div>
  );
}

/** Read-only value box (mockup `.inp`). */
export function ValueBox({ children, className }: { children: React.ReactNode; className?: string }) {
  return (
    <span className={cn("inline-block min-w-[150px] max-w-[320px] truncate rounded-[5px] border bg-surface-2 px-2 py-1 font-mono text-xs", className)}>
      {children}
    </span>
  );
}

export function EmptyState({ title, children, className }: { title: string; children?: React.ReactNode; className?: string }) {
  return (
    <div className={cn("flex flex-col items-center justify-center gap-1 px-6 py-12 text-center", className)}>
      <div className="text-sm font-semibold">{title}</div>
      {children && <div className="max-w-md text-xs text-muted-foreground">{children}</div>}
    </div>
  );
}

export function Loading({ label = "Loading…" }: { label?: string }) {
  return <div className="px-4 py-10 text-center text-xs text-muted-foreground">{label}</div>;
}

export function ErrorBox({ error }: { error: unknown }) {
  const msg = error instanceof Error ? error.message : String(error);
  return <div className="m-4 rounded-md border border-red/40 bg-red-soft px-3 py-2 text-xs text-red">{msg}</div>;
}

/** Renders `inline code` spans in plain finding text. */
export function RichText({ text, className }: { text: string; className?: string }) {
  const parts = text.split(/(`[^`]+`)/g);
  return (
    <span className={cn("whitespace-pre-wrap", className)}>
      {parts.map((p, i) =>
        p.startsWith("`") && p.endsWith("`") && p.length > 2 ? (
          <code key={i} className="rounded bg-codebg px-1 py-px font-mono text-[0.92em]">
            {p.slice(1, -1)}
          </code>
        ) : (
          <span key={i}>{p}</span>
        ),
      )}
    </span>
  );
}
