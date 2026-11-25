// Pure helpers for the Claude pane's app-wide limits form. Unit tested in limits.test.ts.
import type { AppSettings } from "@/bindings/AppSettings";

export type LimitsDraft = {
  model: string;
  max_turns: string;
  max_reviews_per_hour: string;
  timeout_min: string;
  mcp_port: string;
};

export const MODELS = ["opus", "sonnet", "haiku"];

export function draftFromSettings(s: AppSettings): LimitsDraft {
  return {
    model: s.model,
    max_turns: String(s.max_turns),
    max_reviews_per_hour: String(s.max_reviews_per_hour),
    timeout_min: String(Math.round((s.review_timeout_secs / 60) * 10) / 10),
    mcp_port: String(s.mcp_port),
  };
}

const RANGES: Record<Exclude<keyof LimitsDraft, "model">, [number, number, string]> = {
  max_turns: [1, 200, "Max turns"],
  max_reviews_per_hour: [1, 120, "Reviews per hour"],
  timeout_min: [1, 240, "Timeout"],
  mcp_port: [1024, 65535, "MCP port"],
};

/** Field → error message. Empty object = valid. */
export function validateLimits(d: LimitsDraft): Partial<Record<keyof LimitsDraft, string>> {
  const errors: Partial<Record<keyof LimitsDraft, string>> = {};
  if (!d.model.trim()) errors.model = "Pick a model";
  for (const [key, [min, max, label]] of Object.entries(RANGES) as [keyof typeof RANGES, [number, number, string]][]) {
    const raw = d[key].trim();
    const n = Number(raw);
    const integer = key !== "timeout_min";
    if (!raw || !Number.isFinite(n) || (integer && !Number.isInteger(n))) errors[key] = `${label} must be ${integer ? "a whole number" : "a number"}`;
    else if (n < min || n > max) errors[key] = `${label} must be between ${min} and ${max}`;
  }
  return errors;
}

export function applyLimits(s: AppSettings, d: LimitsDraft): AppSettings {
  return {
    ...s,
    model: d.model.trim(),
    max_turns: Number(d.max_turns),
    max_reviews_per_hour: Number(d.max_reviews_per_hour),
    review_timeout_secs: Math.round(Number(d.timeout_min) * 60),
    mcp_port: Number(d.mcp_port),
  };
}

export function limitsDirty(s: AppSettings, d: LimitsDraft): boolean {
  const base = draftFromSettings(s);
  return (Object.keys(base) as (keyof LimitsDraft)[]).some((k) => base[k].trim() !== d[k].trim());
}
