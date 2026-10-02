export type QuotaTone = "good" | "warn" | "critical" | "empty" | "unknown";

/**
 * Shared remaining-% tone (single copy — AccountBlock and ProviderQuotaList
 * both render through here so bar colors can never diverge).
 */
export function quotaTone(pct: number | null | undefined): QuotaTone {
  if (pct == null || Number.isNaN(pct)) return "unknown";
  if (pct <= 0) return "empty";
  if (pct < 30) return "critical";
  if (pct < 60) return "warn";
  return "good";
}
