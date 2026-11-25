import { useCallback, useEffect, useMemo, useState } from "react";

import type { Finding } from "@/bindings/Finding";
import type { FindingStatus } from "@/bindings/FindingStatus";
import { useSetFindingStatus } from "@/lib/queries";
import { applyOverrides, isFinal, pruneOverrides, type Decision } from "./logic";

export type Decisions = {
  /** Findings with local decisions applied (what the UI shows until the server catches up). */
  findings: Finding[];
  decide: (f: Finding, to: Decision) => void;
  /** Ids decided in this session, newest last (for U). */
  history: number[];
  busy: boolean;
  reset: () => void;
};

/**
 * One status per finding: approved, dismissed or draft (undecided). Decisions show immediately
 * and are written through `set_finding_status`; a failed write rolls the local state back.
 */
export function useDecisions(serverFindings: Finding[]): Decisions {
  const [overrides, setOverrides] = useState<Map<number, FindingStatus>>(() => new Map());
  const [history, setHistory] = useState<number[]>([]);
  const setStatus = useSetFindingStatus();

  useEffect(() => {
    setOverrides((m) => pruneOverrides(serverFindings, m));
  }, [serverFindings]);

  const findings = useMemo(() => applyOverrides(serverFindings, overrides), [serverFindings, overrides]);

  const decide = useCallback(
    (f: Finding, to: Decision) => {
      if (isFinal(f.status) || f.status === to) return;
      setOverrides((m) => new Map(m).set(f.id, to));
      setHistory((h) => (to === "draft" ? h.filter((id) => id !== f.id) : [...h.filter((id) => id !== f.id), f.id]));
      // mutateAsync so every call gets its own rollback (per-mutate callbacks only fire for the latest).
      setStatus.mutateAsync({ id: f.id, status: to, runId: f.run_id }).catch(() =>
        setOverrides((m) => {
          const next = new Map(m);
          next.delete(f.id);
          return next;
        }),
      );
    },
    [setStatus],
  );

  const reset = useCallback(() => {
    setOverrides(new Map());
    setHistory([]);
  }, []);

  return { findings, decide, history, busy: setStatus.isPending, reset };
}
