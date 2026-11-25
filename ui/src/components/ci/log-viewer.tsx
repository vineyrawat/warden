import { useEffect, useMemo, useRef, useState } from "react";
import { ArrowDown, ArrowUp, ChevronDown, ChevronRight, Search } from "lucide-react";

import { Button } from "@/components/ui/button";
import { logSections, type LogSection } from "@/lib/ci-parse";
import { classifyLogLine, type LogTone } from "@/lib/format";
import { cn } from "@/lib/utils";

const TONE: Record<LogTone, string> = { error: "bg-red-soft/70 text-red", ok: "text-green", meta: "text-muted-foreground", plain: "" };

/** Request to reveal a line; `n` changes on every request so the same line can be jumped to twice. */
export type LogFocus = { line: number; n: number };

/**
 * Monospace CI log: collapsible sections per `$ command`, line numbers, error highlighting,
 * search, and live tail (sticks to the bottom while running unless the user scrolled up).
 */
export function LogViewer({
  lines,
  loading,
  running,
  raw,
  focus,
  emptyText,
}: {
  lines: string[];
  loading: boolean;
  running: boolean;
  raw: boolean;
  focus: LogFocus | null;
  emptyText?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const [toggled, setToggled] = useState<Map<number, boolean>>(new Map());
  const [query, setQuery] = useState("");
  const [cur, setCur] = useState(0);
  const [flash, setFlash] = useState<number | null>(null);

  const tones = useMemo(() => lines.map(classifyLogLine), [lines]);
  const sections: LogSection[] = useMemo(
    () => (raw || lines.length === 0 ? [{ title: null, start: 0, end: lines.length }] : logSections(lines)),
    [lines, raw],
  );
  const q = query.trim().toLowerCase();
  const matches = useMemo(() => {
    if (!q) return [];
    const out: number[] = [];
    lines.forEach((l, i) => l.toLowerCase().includes(q) && out.push(i));
    return out;
  }, [lines, q]);
  const curLine = matches.length ? matches[Math.min(cur, matches.length - 1)] : null;

  const errorsIn = (s: LogSection) => {
    let n = 0;
    for (let i = s.start; i < s.end; i++) if (tones[i] === "error") n++;
    return n;
  };
  const isOpen = (s: LogSection, idx: number) => {
    if (!s.title) return true;
    if (curLine != null && curLine >= s.start && curLine < s.end) return true;
    const t = toggled.get(s.start);
    if (t !== undefined) return t;
    return sections.length <= 1 || idx === sections.length - 1 || errorsIn(s) > 0;
  };

  const reveal = (line: number) => {
    requestAnimationFrame(() => {
      const el = ref.current?.querySelector<HTMLElement>(`[data-line="${line}"]`);
      el?.scrollIntoView({ block: "center" });
    });
  };

  // Jump requests from the parent ("Jump to first error").
  useEffect(() => {
    if (!focus) return;
    stick.current = false;
    const s = sections.find((x) => focus.line >= x.start && focus.line < x.end);
    if (s?.title) setToggled((m) => new Map(m).set(s.start, true));
    setFlash(focus.line);
    reveal(focus.line);
    const t = setTimeout(() => setFlash(null), 1600);
    return () => clearTimeout(t);
  }, [focus?.n]);

  useEffect(() => {
    if (curLine != null) {
      stick.current = false;
      reveal(curLine);
    }
  }, [curLine]);

  // Live tail.
  useEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [lines.length]);

  const step = (d: 1 | -1) => {
    if (!matches.length) return;
    setCur((c) => (Math.min(c, matches.length - 1) + d + matches.length) % matches.length);
  };

  const renderLine = (i: number) => {
    const l = lines[i];
    const isMatch = q && l.toLowerCase().includes(q);
    return (
      <div
        key={i}
        data-line={i}
        className={cn(
          "-mx-3.5 grid grid-cols-[3.25rem_minmax(0,1fr)] px-3.5",
          TONE[tones[i]],
          isMatch && "bg-yellow-soft",
          curLine === i && "bg-yellow-soft shadow-[inset_2px_0_0_hsl(var(--yellow))]",
          flash === i && "bg-brand-soft shadow-[inset_2px_0_0_hsl(var(--brand))]",
        )}
      >
        <span aria-hidden className="select-none pr-3 text-right text-muted-foreground/60">
          {i + 1}
        </span>
        <span className="whitespace-pre">{isMatch ? <Highlight text={l} q={q} /> : l || " "}</span>
      </div>
    );
  };

  const empty = lines.length === 0 || (lines.length === 1 && !lines[0]);

  return (
    <div className="flex min-h-[160px] flex-1 flex-col overflow-hidden rounded-lg border bg-codebg">
      <div className="flex shrink-0 items-center gap-1.5 border-b px-2 py-1.5">
        <div className="relative w-[240px] max-w-full">
          <Search aria-hidden className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <input
            type="search"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setCur(0);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                step(e.shiftKey ? -1 : 1);
              } else if (e.key === "Escape") setQuery("");
            }}
            placeholder="Search log"
            aria-label="Search log"
            className="h-6 w-full rounded-[5px] border bg-card pl-7 pr-2 font-mono text-2xs placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          />
        </div>
        {q && (
          <>
            <span className="font-mono text-2xs text-muted-foreground" aria-live="polite">
              {matches.length ? `${Math.min(cur, matches.length - 1) + 1} / ${matches.length}` : "no matches"}
            </span>
            <Button size="icon" variant="ghost" className="h-6 w-6" aria-label="Previous match" disabled={!matches.length} onClick={() => step(-1)}>
              <ArrowUp />
            </Button>
            <Button size="icon" variant="ghost" className="h-6 w-6" aria-label="Next match" disabled={!matches.length} onClick={() => step(1)}>
              <ArrowDown />
            </Button>
          </>
        )}
        <span className="ml-auto font-mono text-2xs text-muted-foreground">
          {running ? (
            <span className="inline-flex items-center gap-1.5">
              <span aria-hidden className="inline-block h-1.5 w-1.5 animate-pulse rounded-full bg-brand" /> live
            </span>
          ) : (
            !empty && `${lines.length} lines`
          )}
        </span>
      </div>
      <div
        ref={ref}
        tabIndex={0}
        role="log"
        aria-live={running ? "polite" : "off"}
        aria-label="Job log"
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
        className="min-h-0 flex-1 overflow-auto px-3.5 py-2.5 font-mono text-xs leading-[1.6]"
      >
        {loading ? (
          <span className="text-muted-foreground">Loading log…</span>
        ) : empty ? (
          <span className="text-muted-foreground">{running ? "Waiting for output…" : (emptyText ?? "No output.")}</span>
        ) : (
          sections.map((s, idx) => {
            if (!s.title) return <div key={s.start}>{range(s.start, s.end).map(renderLine)}</div>;
            const open = isOpen(s, idx);
            const errs = errorsIn(s);
            const Chev = open ? ChevronDown : ChevronRight;
            return (
              <div key={s.start}>
                <button
                  type="button"
                  aria-expanded={open}
                  data-line={s.start}
                  onClick={() => setToggled((m) => new Map(m).set(s.start, !open))}
                  className={cn(
                    "-mx-3.5 grid w-[calc(100%+1.75rem)] grid-cols-[3.25rem_minmax(0,1fr)_auto] items-center px-3.5 text-left font-medium hover:bg-accent/60",
                    flash === s.start && "bg-brand-soft",
                  )}
                >
                  <span aria-hidden className="flex select-none items-center justify-end gap-1 pr-3 text-muted-foreground/60">
                    <Chev className="h-3 w-3" />
                    {s.start + 1}
                  </span>
                  <span className="truncate">
                    <span className="text-muted-foreground">$ </span>
                    {q && s.title.toLowerCase().includes(q) ? <Highlight text={s.title} q={q} /> : s.title}
                  </span>
                  <span className="pl-3 text-2xs font-normal text-muted-foreground">
                    {s.end - s.start - 1} lines
                    {errs > 0 && <span className="text-red"> · {errs} error{errs === 1 ? "" : "s"}</span>}
                  </span>
                </button>
                {open && range(s.start + 1, s.end).map(renderLine)}
              </div>
            );
          })
        )}
        {running && <div className="mt-1 animate-pulse text-muted-foreground">▍</div>}
      </div>
    </div>
  );
}

function range(a: number, b: number): number[] {
  const out: number[] = [];
  for (let i = a; i < b; i++) out.push(i);
  return out;
}

function Highlight({ text, q }: { text: string; q: string }) {
  const lower = text.toLowerCase();
  const parts: React.ReactNode[] = [];
  let i = 0;
  let k = 0;
  while (q && i < text.length) {
    const j = lower.indexOf(q, i);
    if (j < 0) break;
    if (j > i) parts.push(text.slice(i, j));
    parts.push(
      <mark key={k++} className="rounded-sm bg-yellow/30 text-inherit">
        {text.slice(j, j + q.length)}
      </mark>,
    );
    i = j + q.length;
  }
  parts.push(text.slice(i));
  return <>{parts}</>;
}
