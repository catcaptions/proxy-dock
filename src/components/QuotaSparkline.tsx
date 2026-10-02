import { useEffect, useState } from "react";
import { apiGet } from "../api";

/**
 * Tiny inline quota sparkline (plan 3C): one 40×12 SVG per quota row, drawn
 * only from `GET .../quota/history`. No panel, no text — aria-hidden, so the
 * existing row labels stay the single source of truth.
 */
export default function QuotaSparkline({
  provider,
  accountId,
  rowId,
}: {
  provider: string;
  accountId: string;
  rowId: string;
}) {
  const [points, setPoints] = useState<number[] | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    apiGet<{ observations?: { remaining?: unknown }[] }>(
      `/api/accounts/${encodeURIComponent(provider)}/${encodeURIComponent(accountId)}/quota/history?limit=30`,
      { signal: controller.signal },
    )
      .then((data) => {
        if (controller.signal.aborted) return;
        const obs = Array.isArray(data.observations) ? data.observations : [];
        // History is DESC — reverse to chronological, then pull this row's
        // remainingPct from each observation's remaining payload.
        const values: number[] = [];
        for (const o of [...obs].reverse()) {
          const pct = pctForRow(o.remaining, rowId);
          if (pct !== null) values.push(pct);
        }
        setPoints(values.length >= 2 ? values : null);
      })
      .catch(() => {
        if (!controller.signal.aborted) setPoints(null);
      });
    return () => controller.abort();
  }, [provider, accountId, rowId]);

  if (!points) return null;
  const min = Math.min(...points);
  const max = Math.max(...points);
  const span = max - min || 1;
  const w = 40;
  const h = 12;
  const step = points.length > 1 ? w / (points.length - 1) : 0;
  const d = points.map((v, i) => `${(i * step).toFixed(1)},${(h - ((v - min) / span) * (h - 2) - 1).toFixed(1)}`).join(" ");
  return (
    <svg
      width={w}
      height={h}
      viewBox={`0 0 ${w} ${h}`}
      aria-hidden="true"
      data-testid={`sparkline-${provider}-${accountId}-${rowId}`}
      style={{ flex: "none" }}
    >
      <polyline points={d} fill="none" stroke="currentColor" strokeWidth={1} opacity={0.6} />
    </svg>
  );
}

function pctForRow(remaining: unknown, rowId: string): number | null {
  if (!remaining) return null;
  // Canonical shape: Vec<QuotaRow> (see quota.rs) — [{id, remainingPct?...}].
  if (Array.isArray(remaining)) {
    for (const r of remaining) {
      if (r && typeof r === "object") {
        const rec = r as Record<string, unknown>;
        if (rec.id === rowId && typeof rec.remainingPct === "number" && Number.isFinite(rec.remainingPct)) {
          return Math.max(0, Math.min(100, rec.remainingPct));
        }
      }
    }
    return null;
  }
  if (typeof remaining === "object") {
    const rec = remaining as Record<string, unknown>;
    const direct = rec[rowId];
    if (typeof direct === "number" && Number.isFinite(direct)) return Math.max(0, Math.min(100, direct));
    // Some shapes nest under rows/data.
    for (const key of ["rows", "data", "quota"]) {
      const nested = rec[key];
      if (Array.isArray(nested)) {
        const found = pctForRow(nested, rowId);
        if (found !== null) return found;
      }
    }
  }
  return null;
}
