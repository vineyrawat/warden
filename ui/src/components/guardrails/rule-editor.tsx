// Rule editor side panel (UX "custom rules" variant A + C's test on a recent MR).
// New rules are saved as project overrides keyed by id (config::merge_overrides adds them); for
// rules from .warden.yml or the built-in set only severity and file scope can be changed locally.
import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { FlaskConical, Loader2, Trash2 } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { Segmented } from "@/components/segmented";
import type { RuleConfig } from "@/bindings/RuleConfig";
import type { RuleTestResult } from "@/bindings/RuleTestResult";
import type { Severity } from "@/bindings/Severity";
import * as api from "@/lib/api";
import { SEVERITIES, capitalize, formatRelative } from "@/lib/format";
import { errorMessage, qk, useRunRows } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { nextCustomId, withRule, withoutRule, type RuleOrigin } from "./logic";

type Kind = "claude" | "pattern" | "semgrep";

const kindOf = (r: RuleConfig): Kind => (r.semgrep ? "semgrep" : r.pattern ? "pattern" : "claude");

export const overridesKey = (projectId: number) => ["project-overrides", projectId] as const;

export type EditorTarget = { rule: RuleConfig | null; origin: RuleOrigin | null; duplicate?: boolean };

export function RuleEditor({
  projectId,
  target,
  onClose,
  existingIds,
}: {
  projectId: number;
  target: EditorTarget | null;
  onClose: () => void;
  existingIds: string[];
}) {
  const qc = useQueryClient();
  const open = target != null;
  const isNew = !target?.rule || !!target.duplicate;
  const isCustom = isNew || target?.origin === "custom";
  const base = !isCustom ? target?.rule ?? null : null;

  const [id, setId] = useState("");
  const [title, setTitle] = useState("");
  const [kind, setKind] = useState<Kind>("claude");
  const [instruction, setInstruction] = useState("");
  const [pattern, setPattern] = useState("");
  const [semgrep, setSemgrep] = useState("");
  const [appliesTo, setAppliesTo] = useState("");
  const [severity, setSeverity] = useState<Severity>("major");
  const [sample, setSample] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!target) return;
    const r = target.rule;
    setId(isNew ? nextCustomId(existingIds) : r!.id);
    setTitle(r ? (target.duplicate ? `${r.title} (copy)` : r.title) : "");
    setKind(r ? kindOf(r) : "claude");
    setInstruction(r?.instruction ?? "");
    setPattern(r?.pattern ?? "");
    setSemgrep(r?.semgrep ?? "");
    setAppliesTo(r?.applies_to ?? "");
    setSeverity(r?.severity ?? "major");
    setSample("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target]);

  const rule: RuleConfig = {
    id: id.trim(),
    title: title.trim(),
    applies_to: appliesTo.trim() || null,
    instruction: kind === "semgrep" ? null : instruction.trim() || null,
    pattern: kind === "pattern" ? pattern : null,
    semgrep: kind === "semgrep" ? semgrep.trim() || null : null,
    severity,
  };

  const patternError = useMemo(() => {
    if (kind !== "pattern" || !pattern) return null;
    try {
      new RegExp(pattern);
      return null;
    } catch (e) {
      return (e as Error).message.replace(/^Invalid regular expression: /, "");
    }
  }, [kind, pattern]);

  const problems = [
    !rule.id && "Give the rule an ID.",
    isNew && existingIds.map((x) => x.toUpperCase()).includes(rule.id.toUpperCase()) && `${rule.id} is already used.`,
    !rule.title && "Give the rule a name.",
    isCustom && kind === "claude" && !instruction.trim() && "Say what Claude should check.",
    isCustom && kind === "pattern" && !pattern && "Enter the pattern to look for.",
    isCustom && kind === "semgrep" && !semgrep.trim() && "Enter the semgrep rules file.",
    patternError && `Pattern: ${patternError}`,
  ].filter(Boolean) as string[];

  const save = async () => {
    setBusy(true);
    try {
      const stored = await api.get_project_overrides(projectId);
      await api.save_project_overrides(projectId, withRule(stored, rule, base));
      await refresh();
      toast.success(isNew ? `Added ${rule.id}` : `Saved ${rule.id}`);
      onClose();
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    try {
      const stored = await api.get_project_overrides(projectId);
      await api.save_project_overrides(projectId, withoutRule(stored, rule.id));
      await refresh();
      toast.success(isCustom ? `Deleted ${rule.id}` : `${rule.id} reset to its default`);
      onClose();
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const refresh = () =>
    Promise.all([
      qc.invalidateQueries({ queryKey: qk.projectConfig(projectId) }),
      qc.invalidateQueries({ queryKey: overridesKey(projectId) }),
      qc.invalidateQueries({ queryKey: ["simulate-guardrails"] }),
      qc.invalidateQueries({ queryKey: ["rule-stats"] }),
    ]);

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="left-auto right-0 top-0 h-full max-h-full w-[min(520px,100vw)] max-w-none translate-x-0 translate-y-0 content-start gap-3 rounded-none border-y-0 border-r-0 data-[state=closed]:zoom-out-100 data-[state=open]:zoom-in-100">
        <DialogHeader>
          <DialogTitle>{isNew ? "New rule" : isCustom ? `Edit ${rule.id}` : `Adjust ${rule.id}`}</DialogTitle>
          <DialogDescription>
            {isCustom
              ? "Saved for this project as a local override, on top of the repo's .warden.yml."
              : `This rule comes from ${target?.origin === "builtin" ? "Warden's built-in set" : ".warden.yml"}. You can change its severity and which files it applies to here; change its check in the repo.`}
          </DialogDescription>
        </DialogHeader>

        <div className="grid grid-cols-[minmax(0,1fr)_110px] gap-2">
          <Field label="Name" htmlFor="re-title">
            <Input id="re-title" autoFocus={isNew} disabled={!isCustom} value={title} onChange={(e) => setTitle(e.target.value)} placeholder="No print() left in Python code" className="font-sans" />
          </Field>
          <Field label="ID" htmlFor="re-id">
            <Input id="re-id" disabled={!isNew} value={id} onChange={(e) => setId(e.target.value.toUpperCase().replace(/\s+/g, "-"))} className="font-mono" />
          </Field>
        </div>

        {isCustom && (
          <Field label="How Warden checks it">
            <Segmented<Kind>
              label="Rule kind"
              value={kind}
              onChange={setKind}
              options={[
                { value: "claude", label: "Claude" },
                { value: "pattern", label: "Pattern" },
                { value: "semgrep", label: "Semgrep" },
              ]}
            />
            <p className="text-2xs text-muted-foreground">
              {kind === "claude"
                ? "Added to Claude's instructions for every review; Claude must point at a changed line."
                : kind === "pattern"
                  ? "A regular expression run on added lines. Free and deterministic."
                  : "A semgrep rules file in the repo, run when semgrep is installed."}
            </p>
          </Field>
        )}

        {isCustom && kind === "claude" && (
          <Field label="What Claude should check" htmlFor="re-instr">
            <Textarea id="re-instr" rows={4} value={instruction} onChange={(e) => setInstruction(e.target.value)} placeholder="User-facing strings in Python and JS must be wrapped in _() so they can be translated." />
          </Field>
        )}

        {isCustom && kind === "pattern" && (
          <>
            <Field label="Pattern (regex, run on added lines)" htmlFor="re-pattern" error={patternError}>
              <Input id="re-pattern" value={pattern} onChange={(e) => setPattern(e.target.value)} placeholder="^\s*print\(" className="font-mono" spellCheck={false} />
            </Field>
            <Field label="Note posted on a match" htmlFor="re-note">
              <Textarea id="re-note" rows={2} value={instruction} onChange={(e) => setInstruction(e.target.value)} placeholder="Remove the debug print before merging; use frappe.logger() if you need the output." />
            </Field>
            <PasteTest pattern={patternError ? "" : pattern} sample={sample} setSample={setSample} />
          </>
        )}

        {isCustom && kind === "semgrep" && (
          <Field label="Semgrep rules file (path in the repo)" htmlFor="re-semgrep">
            <Input id="re-semgrep" value={semgrep} onChange={(e) => setSemgrep(e.target.value)} placeholder="rules/no-raw-sql.yml" className="font-mono" />
          </Field>
        )}

        <div className="grid grid-cols-[minmax(0,1fr)_auto] items-end gap-2">
          <Field label="Applies to (file glob, optional)" htmlFor="re-glob">
            <Input id="re-glob" value={appliesTo} onChange={(e) => setAppliesTo(e.target.value)} placeholder="**/*.py" className="font-mono" />
          </Field>
          <Field label="Severity">
            <Segmented<Severity> label="Severity" value={severity} onChange={setSeverity} options={SEVERITIES.map((s) => ({ value: s, label: capitalize(s) }))} />
          </Field>
        </div>

        <RunTest projectId={projectId} rule={rule} disabled={!!patternError} />

        {problems.length > 0 && <div className="text-2xs text-orange">{problems[0]}</div>}

        <div className="mt-auto flex items-center gap-2 border-t pt-3">
          {!isNew && (
            <Button variant="ghost" className="text-red" disabled={busy} onClick={remove}>
              <Trash2 /> {isCustom ? "Delete rule" : "Reset to default"}
            </Button>
          )}
          <span className="flex-1" />
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="default" disabled={busy || problems.length > 0} onClick={save}>
            {busy && <Loader2 className="animate-spin" />}
            {isNew ? "Add rule" : "Save"}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function Field({ label, htmlFor, error, children }: { label: string; htmlFor?: string; error?: string | null; children: React.ReactNode }) {
  return (
    <div className="grid min-w-0 gap-1">
      <Label htmlFor={htmlFor} className="text-2xs text-muted-foreground">
        {label}
      </Label>
      {children}
      {error && <span className="text-2xs text-red">{error}</span>}
    </div>
  );
}

/** Paste code, see which lines the pattern matches (runs the same regex engine as the reviews). */
function PasteTest({ pattern, sample, setSample }: { pattern: string; sample: string; setSample: (s: string) => void }) {
  const [res, setRes] = useState<RuleTestResult | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    if (!pattern || !sample.trim()) {
      setRes(null);
      setErr(null);
      return;
    }
    const t = setTimeout(() => {
      api
        .test_rule_pattern(pattern, sample)
        .then((r) => {
          setRes(r);
          setErr(null);
        })
        .catch((e) => setErr(errorMessage(e)));
    }, 250);
    return () => clearTimeout(t);
  }, [pattern, sample]);
  const hit = new Set(res?.matches.map((m) => m.line) ?? []);
  const lines = sample.split("\n");
  return (
    <div className="grid gap-1">
      <div className="flex items-center gap-2 text-2xs text-muted-foreground">
        <span>Test: paste code</span>
        {res && <span className={cn("font-medium", hit.size ? "text-red" : "text-green")}>{hit.size ? `${hit.size} of ${lines.length} lines match` : "no lines match"}</span>}
        {err && <span className="text-red">{err}</span>}
      </div>
      <div className="relative overflow-hidden rounded-md border bg-codebg">
        <Textarea
          aria-label="Code to test the pattern on"
          rows={5}
          value={sample}
          onChange={(e) => setSample(e.target.value)}
          spellCheck={false}
          placeholder={"def import_rows(rows):\n    print(rows)"}
          className="min-h-[96px] resize-y rounded-none border-0 bg-transparent font-mono text-[11.5px] leading-[18px]"
        />
      </div>
      {hit.size > 0 && (
        <ul className="max-h-28 overflow-y-auto rounded-md border bg-red-soft/50 px-2 py-1 font-mono text-[11px] leading-[18px]">
          {res!.matches.map((m) => (
            <li key={m.line} className="truncate">
              <span className="mr-2 text-muted-foreground">{m.line}</span>
              <span className="text-red">{m.text}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** Run the unsaved rule over a recent MR's diff (C's test bench). Nothing is posted. */
function RunTest({ projectId, rule, disabled }: { projectId: number; rule: RuleConfig; disabled: boolean }) {
  const rows = useRunRows(200);
  const runs = useMemo(() => (rows.data ?? []).filter((r) => r.project_id === projectId).slice(0, 15), [rows.data, projectId]);
  const [runId, setRunId] = useState<number | null>(null);
  const chosen = runId ?? runs[0]?.run.id ?? null;
  const key = JSON.stringify({ rule, chosen });
  const [asked, setAsked] = useState<string | null>(null);
  const test = useQuery({
    queryKey: ["rule-test", key],
    queryFn: () => api.test_rule_on_run(rule, chosen!),
    enabled: asked === key && chosen != null,
    staleTime: 60_000,
    retry: false,
  });
  if (!runs.length) return null;
  const res = test.data;
  return (
    <section className="grid gap-1.5 rounded-md border px-2.5 py-2">
      <div className="flex items-center gap-2">
        <FlaskConical className="h-3.5 w-3.5 text-muted-foreground" aria-hidden />
        <span className="text-xs font-medium">Test on a recent MR</span>
        <span className="flex-1" />
        <Select value={chosen != null ? String(chosen) : undefined} onValueChange={(v) => setRunId(Number(v))}>
          <SelectTrigger aria-label="Merge request to test on" className="h-7 w-[230px] font-sans">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {runs.map((r) => (
              <SelectItem key={r.run.id} value={String(r.run.id)} className="text-xs">
                !{r.mr_iid} {r.mr_title} · {formatRelative(r.run.started_at)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button size="sm" disabled={disabled || !rule.title || test.isFetching} onClick={() => setAsked(key)}>
          {test.isFetching ? <Loader2 className="animate-spin" /> : null} Test
        </Button>
      </div>
      {test.isError && <div className="text-2xs text-red">{errorMessage(test.error)}</div>}
      {res?.note && <div className="text-2xs text-muted-foreground">{res.note}</div>}
      {res && !res.note && (
        <>
          <div className={cn("text-2xs font-medium", res.matches.length ? "text-red" : "text-green")}>
            {res.matches.length
              ? `${res.matches.length} ${res.matches.length === 1 ? "match" : "matches"} in ${new Set(res.matches.map((m) => m.file)).size} of ${res.files_checked} files`
              : `No matches in ${res.files_checked} ${res.files_checked === 1 ? "file" : "files"}`}
          </div>
          {res.matches.length > 0 && (
            <ul className="max-h-40 overflow-y-auto rounded-md border bg-codebg px-2 py-1 font-mono text-[11px] leading-[18px]">
              {res.matches.map((m) => (
                <li key={`${m.file}:${m.line}`} className="truncate" title={`${m.file}:${m.line}`}>
                  <span className="mr-2 text-muted-foreground">
                    {m.file.split("/").pop()}:{m.line}
                  </span>
                  <span className="text-red">{m.text}</span>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
      <div className="text-2xs text-muted-foreground">Nothing is posted. Pattern rules are tested on the MR's added lines.</div>
    </section>
  );
}
