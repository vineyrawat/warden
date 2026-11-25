/** One-line summary of an audit payload: the tool / event / decision first, then key=value pairs. */
export function summarize(payload: unknown): string {
  if (payload == null) return "";
  if (typeof payload !== "object") return String(payload);
  const p = payload as Record<string, unknown>;
  const head = p.tool ?? p.event ?? p.decision ?? (p.summary ? "summary note" : undefined) ?? p.finding;
  const rest = Object.entries(p)
    .filter(([k]) => !["tool", "event"].includes(k))
    .map(([k, v]) => `${k}=${typeof v === "object" ? JSON.stringify(v) : String(v)}`)
    .join(" ");
  return head ? `${String(head)}${rest ? ` · ${rest}` : ""}` : rest;
}
