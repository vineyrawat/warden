import { useMemo, useState } from "react";

import type { ReviewRun } from "@/bindings/ReviewRun";
import type { Stats } from "@/bindings/Stats";
import { useStats } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { buildKpis, chartBars, compareWeeks, dayLabel, type Kpi, type KpiTone } from "./history-logic";

const TONE: Record<KpiTone, string> = { good: "text-green", bad: "text-red", neutral: "text-muted-foreground" };

/** Week-over-week numbers (last 7 days vs the 7 before) and a 14-day reviews-per-day chart. */
export function KpiStrip({ runs }: { runs: ReviewRun[] }) {
  const s14 = useStats(14);
  const s7 = useStats(7);
  const kpis = useMemo<Kpi[] | null>(() => (s14.data && s7.data ? buildKpis(compareWeeks(s14.data, s7.data, runs)) : null), [s14.data, s7.data, runs]);
  const error = s14.error ?? s7.error;

  return (
    <div className="grid grid-cols-[repeat(4,minmax(0,1fr))_minmax(0,1.7fr)] gap-2.5" aria-label="Last 7 days compared with the 7 days before">
      {(kpis ?? PLACEHOLDER).map((k) => (
        <div key={k.key} className="min-w-0 rounded-lg border bg-card px-3 py-2">
          <div className="font-mono text-xl font-semibold tabular-nums leading-tight">{k.value}</div>
          <div className="truncate text-2xs text-muted-foreground">{k.label}</div>
          <div className={cn("mt-0.5 truncate text-2xs", TONE[k.tone])} title={k.delta}>
            {kpis ? k.delta : error ? "Stats unavailable" : " "}
          </div>
        </div>
      ))}
      <ReviewsChart stats={s14.data} />
    </div>
  );
}

const PLACEHOLDER: Kpi[] = [
  { key: "reviews", value: "—", label: "reviews this week", delta: "", tone: "neutral" },
  { key: "notes", value: "—", label: "notes posted", delta: "", tone: "neutral" },
  { key: "resolved", value: "—", label: "threads resolved", delta: "", tone: "neutral" },
  { key: "median", value: "—", label: "median review", delta: "", tone: "neutral" },
];

const W = 280;
const H = 44;
const GAP = 5;

function ReviewsChart({ stats }: { stats: Stats | undefined }) {
  const [hover, setHover] = useState<number | null>(null);
  const days = stats?.per_day ?? [];
  const currentFrom = Math.max(0, days.length - 7);
  const { bars, max } = useMemo(() => chartBars(days, W, H, GAP, currentFrom), [days, currentFrom]);
  const hovered = hover != null ? bars[hover] : null;
  const hoverFrac = hovered ? (hovered.x + hovered.w / 2) / W : 0;
  const total = days.reduce((a, d) => a + d.reviews, 0);

  return (
    <div className="relative min-w-0 rounded-lg border bg-card px-3 pb-1.5 pt-2">
      <div className="mb-1 flex items-baseline gap-2 whitespace-nowrap text-2xs text-muted-foreground">
        <span className="truncate font-semibold text-foreground">Reviews per day</span>
        <span className="ml-auto flex shrink-0 items-center gap-2">
          <Legend className="bg-brand/40" label="prev. 7 days" />
          <Legend className="bg-brand" label="this week" />
        </span>
      </div>
      {!stats ? (
        <div className="flex h-[44px] items-center justify-center text-2xs text-muted-foreground">Loading…</div>
      ) : total === 0 ? (
        <div className="flex h-[44px] items-center justify-center text-2xs text-muted-foreground">No reviews in the last 14 days</div>
      ) : (
        <div className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-1.5">
          <div className="flex h-[44px] flex-col justify-between text-right font-mono text-[10px] leading-none text-muted-foreground" aria-hidden>
            <span>{max}</span>
            <span>0</span>
          </div>
          <div className="relative min-w-0">
            <svg
              viewBox={`0 0 ${W} ${H}`}
              preserveAspectRatio="none"
              className="h-[44px] w-full overflow-visible"
              role="img"
              aria-label={`Reviews per day over the last 14 days, ${total} in total, axis 0 to ${max}`}
              onMouseLeave={() => setHover(null)}
            >
              <line x1={0} x2={W} y1={H} y2={H} className="stroke-border" strokeWidth={1} vectorEffect="non-scaling-stroke" />
              <line x1={0} x2={W} y1={0} y2={0} className="stroke-border" strokeWidth={1} strokeDasharray="2 3" vectorEffect="non-scaling-stroke" />
              {bars.map((b, i) => (
                <g key={b.date} onMouseEnter={() => setHover(i)}>
                  <rect x={b.x - GAP / 2} y={0} width={b.w + GAP} height={H} className="fill-transparent" />
                  <rect
                    x={b.x}
                    y={b.reviews ? b.y : H - 1}
                    width={b.w}
                    height={b.reviews ? b.h : 1}
                    className={cn(b.reviews ? (b.current ? "fill-brand" : "fill-brand/40") : "fill-border", hover === i && "opacity-80")}
                  />
                </g>
              ))}
            </svg>
            {hovered && (
              <div
                role="tooltip"
                className={cn(
                  "pointer-events-none absolute bottom-full z-20 mb-1 whitespace-nowrap rounded-md bg-foreground px-2 py-1 text-2xs text-background shadow",
                  hoverFrac < 0.25 ? "-translate-x-2" : hoverFrac > 0.75 ? "-translate-x-[calc(100%-8px)]" : "-translate-x-1/2",
                )}
                style={{ left: `${hoverFrac * 100}%` }}
              >
                <b>{dayLabel(hovered.date, { weekday: "short", month: "short", day: "numeric" })}</b> · {hovered.reviews} review{hovered.reviews === 1 ? "" : "s"} ·{" "}
                {hovered.notes} note{hovered.notes === 1 ? "" : "s"} posted
              </div>
            )}
          </div>
          <span />
          <div className="mt-0.5 flex justify-between font-mono text-[10px] leading-none text-muted-foreground">
            <span>{days[0] ? dayLabel(days[0].date) : ""}</span>
            <span>Today</span>
          </div>
        </div>
      )}
    </div>
  );
}

function Legend({ className, label }: { className: string; label: string }) {
  return (
    <span className="inline-flex items-center gap-1">
      <i className={cn("inline-block h-2 w-2 rounded-sm", className)} aria-hidden />
      {label}
    </span>
  );
}
