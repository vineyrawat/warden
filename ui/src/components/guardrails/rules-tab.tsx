// Rules tab (8B): rule catalogue with kind, source, severity, on/off switch and 30-day hit stats.
import { useMemo, useState } from "react";
import { Search } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tip } from "@/components/ui/tooltip";
import { RuleKindPill, SeverityPill } from "@/components/pills";
import { Segmented } from "@/components/segmented";
import type { RuleConfig } from "@/bindings/RuleConfig";
import type { RuleKind } from "@/bindings/RuleKind";
import type { RuleStat } from "@/bindings/RuleStat";
import type { SimulatedFinding } from "@/bindings/SimulatedFinding";
import { formatRelative, ruleKind } from "@/lib/format";
import { cn } from "@/lib/utils";
import { NOISY_RATE, dismissedRate, filterRules, ruleAdvice, ruleCatches, ruleSource, ruleSourceLabel, unmatchedStats, type RuleFilter, type RuleSource } from "./logic";

export const STATS_DAYS = 30;

export function RulesTab({
  rules,
  disabled,
  onToggle,
  stats,
  statsLoading,
  recent,
  branch,
}: {
  rules: RuleConfig[];
  disabled: string[];
  onToggle: (id: string, on: boolean) => void;
  stats: RuleStat[] | undefined;
  statsLoading: boolean;
  /** Recent findings (from the preview query), used to suggest a narrower path for noisy rules. */
  recent: SimulatedFinding[] | undefined;
  branch: string;
}) {
  const [filter, setFilter] = useState<RuleFilter>({ kind: "all", source: "all", query: "" });
  const statById = useMemo(() => new Map((stats ?? []).map((s) => [s.rule_id, s])), [stats]);
  const filesByRule = useMemo(() => {
    const m = new Map<string, string[]>();
    for (const r of recent ?? []) {
      const id = r.finding.rule_id;
      if (!id) continue;
      m.set(id, [...(m.get(id) ?? []), r.finding.file]);
    }
    return m;
  }, [recent]);
  const shown = filterRules(rules, filter);
  const advice = rules.map((r) => ruleAdvice(r, statById.get(r.id), filesByRule.get(r.id))).filter((a): a is string => !!a);
  const other = unmatchedStats(stats ?? [], rules);
  const repoCount = rules.filter((r) => ruleSource(r) === "repo").length;

  return (
    <div className="grid gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <Segmented<RuleKind | "all">
          label="Rule kind"
          value={filter.kind}
          onChange={(kind) => setFilter({ ...filter, kind })}
          options={[
            { value: "all", label: "All" },
            { value: "claude", label: "Claude" },
            { value: "regex", label: "regex" },
            { value: "semgrep", label: "semgrep" },
          ]}
        />
        <Select value={filter.source} onValueChange={(v) => setFilter({ ...filter, source: v as RuleSource | "all" })}>
          <SelectTrigger className="h-7 w-[150px]" aria-label="Rule source">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">All sources</SelectItem>
            <SelectItem value="repo">.warden.yml</SelectItem>
            <SelectItem value="builtin">Built-in</SelectItem>
          </SelectContent>
        </Select>
        <div className="relative ml-auto">
          <Search className="pointer-events-none absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" aria-hidden />
          <Input className="h-7 w-48 pl-7 text-xs" placeholder="Search rules" aria-label="Search rules" value={filter.query} onChange={(e) => setFilter({ ...filter, query: e.target.value })} />
        </div>
      </div>
      <p className="text-2xs text-muted-foreground">
        {repoCount > 0 ? `${repoCount} of ${rules.length} rules come from .warden.yml on ${branch}` : `No rules in .warden.yml on ${branch}; using the built-in set`} · hits from the last {STATS_DAYS} days ·
        regex and semgrep rules are deterministic and cost nothing; Claude rules become instructions it must cite.
      </p>

      {advice.length > 0 && (
        <div className="grid gap-1 rounded-md bg-orange-soft px-2.5 py-2 text-xs">
          {advice.map((a) => (
            <div key={a} className="flex items-start gap-2">
              <span aria-hidden className="mt-[5px] inline-block h-[7px] w-[7px] shrink-0 rounded-full bg-orange" />
              <span>{a}</span>
            </div>
          ))}
          <div className="pl-[15px] text-2xs text-muted-foreground">Change a rule's severity or scope in .warden.yml on {branch}.</div>
        </div>
      )}

      {rules.length === 0 ? (
        <div className="py-6 text-center text-xs text-muted-foreground">No rules configured.</div>
      ) : shown.length === 0 ? (
        <div className="py-6 text-center text-xs text-muted-foreground">No rules match these filters.</div>
      ) : (
        <div className="grid gap-2.5 @2xl:grid-cols-2">
          {shown.map((r) => (
            <RuleCard key={r.id} rule={r} on={!disabled.includes(r.id)} onToggle={(v) => onToggle(r.id, v)} stat={statById.get(r.id)} statsLoading={statsLoading} />
          ))}
        </div>
      )}

      {other.length > 0 && (
        <section className="rounded-lg border bg-card px-3.5 py-3">
          <h3 className="text-[12.5px] font-semibold">Findings without a rule</h3>
          <p className="mb-1.5 text-2xs text-muted-foreground">Claude categories and scanner hits in the last {STATS_DAYS} days. Filter them with confidence and severity on the Filters tab.</p>
          <ul className="divide-y text-xs">
            {other.map((s) => (
              <li key={s.rule_id} className="flex items-center gap-2 py-1.5">
                <span className="font-mono text-muted-foreground">{s.rule_id}</span>
                <span className="min-w-0 flex-1 truncate">{s.title}</span>
                <StatLine stat={s} />
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}

function RuleCard({ rule: r, on, onToggle, stat, statsLoading }: { rule: RuleConfig; on: boolean; onToggle: (v: boolean) => void; stat: RuleStat | undefined; statsLoading: boolean }) {
  const source = ruleSource(r);
  const catches = ruleCatches(r);
  return (
    <article className={cn("flex min-w-0 flex-col gap-1.5 rounded-lg border bg-card px-3 py-2.5", !on && "opacity-60")} aria-label={`Rule ${r.id}`}>
      <div className="flex items-center gap-2">
        <b className="font-mono text-xs">{r.id}</b>
        <SeverityPill severity={r.severity} />
        <span className="flex-1" />
        <Switch checked={on} onCheckedChange={onToggle} aria-label={`${on ? "Disable" : "Enable"} rule ${r.id}`} />
      </div>
      <div className="text-[12.5px] font-medium leading-snug">{r.title}</div>
      {catches && (
        <div className="line-clamp-2 text-2xs text-muted-foreground" title={catches}>
          {catches}
        </div>
      )}
      <div className="mt-auto flex flex-wrap items-center gap-1.5 pt-0.5">
        <RuleKindPill kind={ruleKind(r)} />
        <Tip content={source === "repo" ? "Defined in .warden.yml on the target branch" : "Warden's built-in Frappe rule set"}>
          <span tabIndex={0} className="rounded-full">
            <Badge tone="outline" className="font-mono font-medium">
              {ruleSourceLabel(source)}
            </Badge>
          </span>
        </Tip>
        {r.applies_to && <span className="max-w-[140px] truncate font-mono text-2xs text-muted-foreground" title={r.applies_to}>{r.applies_to}</span>}
        <span className="flex-1" />
        {statsLoading ? <span className="text-2xs text-muted-foreground">…</span> : <StatLine stat={stat} />}
      </div>
    </article>
  );
}

/** "11 hits · 3 posted · 6 dismissed" with a proportion bar; orange when the rule is noisy. */
function StatLine({ stat }: { stat: RuleStat | undefined }) {
  if (!stat || stat.hits === 0) return <span className="text-2xs text-muted-foreground">no hits</span>;
  const rate = dismissedRate(stat);
  const noisy = rate != null && rate >= NOISY_RATE;
  const pct = (n: number) => `${(n / stat.hits) * 100}%`;
  const other = Math.max(0, stat.hits - stat.posted - stat.dismissed - stat.rejected);
  return (
    <Tip content={`${stat.hits} hits · ${stat.posted} posted · ${stat.dismissed} dismissed · ${stat.rejected} dropped by guardrails · last ${formatRelative(stat.last_hit_at)}`}>
      <span tabIndex={0} className="inline-flex items-center gap-2 rounded">
        <span className={cn("whitespace-nowrap text-2xs", noisy ? "font-medium text-orange" : "text-muted-foreground")}>
          {stat.hits} hits · {stat.posted} posted · {stat.dismissed} dismissed
        </span>
        <span className="flex h-1.5 w-12 overflow-hidden rounded-full bg-surface-2" aria-hidden>
          <i className="block bg-green" style={{ width: pct(stat.posted) }} />
          <i className="block bg-orange" style={{ width: pct(stat.dismissed) }} />
          <i className="block bg-muted-foreground/40" style={{ width: pct(stat.rejected) }} />
          <i className="block" style={{ width: pct(other) }} />
        </span>
      </span>
    </Tip>
  );
}
