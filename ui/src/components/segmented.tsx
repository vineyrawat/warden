import { useRef } from "react";
import { cn } from "@/lib/utils";

type Option<T extends string> = { value: T; label: string };

/**
 * Segmented control styled like the mockup's `.seg`. Interactive = radio group with arrow-key
 * navigation; with `readOnly` it is a static display (used in the Review footer).
 */
export function Segmented<T extends string>({
  options,
  value,
  onChange,
  readOnly,
  label,
  className,
}: {
  options: Option<T>[];
  value: T;
  onChange?: (v: T) => void;
  readOnly?: boolean;
  label: string;
  className?: string;
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const idx = options.findIndex((o) => o.value === value);
  const move = (d: number) => {
    const next = (idx + d + options.length) % options.length;
    onChange?.(options[next].value);
    refs.current[next]?.focus();
  };
  return (
    <div
      role={readOnly ? "group" : "radiogroup"}
      aria-label={label}
      className={cn("inline-flex overflow-hidden rounded-[7px] border bg-card", className)}
      onKeyDown={(e) => {
        if (readOnly) return;
        if (e.key === "ArrowRight" || e.key === "ArrowDown") {
          e.preventDefault();
          move(1);
        } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
          e.preventDefault();
          move(-1);
        }
      }}
    >
      {options.map((o, i) => {
        const on = o.value === value;
        const cls = cn(
          "border-r px-3 py-[5px] text-xs last:border-r-0",
          on ? "bg-primary font-semibold text-primary-foreground" : "text-foreground",
        );
        if (readOnly)
          return (
            <span key={o.value} className={cls} aria-current={on ? "true" : undefined}>
              {o.label}
            </span>
          );
        return (
          <button
            key={o.value}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={on}
            tabIndex={on ? 0 : -1}
            className={cn(cls, !on && "hover:bg-accent", "focus-visible:relative focus-visible:z-10")}
            onClick={() => onChange?.(o.value)}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}
