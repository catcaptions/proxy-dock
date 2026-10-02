import { useCallback, useEffect, useState } from "react";
import { apiGet } from "./api";
import type { UsageSummary } from "./api";
import { listLinkedAccounts, refreshQuotaForAccount } from "./auth";
import type { LocalAccountMeta } from "./auth";

export type RangeKey = "24h" | "7d" | "30d";

export function rangeSince(range: RangeKey): string {
  const hours = range === "24h" ? 24 : range === "7d" ? 7 * 24 : 30 * 24;
  return new Date(Date.now() - hours * 3600 * 1000).toISOString();
}

export type UsageState =
  | { state: "loading" }
  | { state: "empty"; summary: UsageSummary }
  | { state: "ready"; summary: UsageSummary }
  | { state: "error"; message: string };

/**
 * Gateway usage summary (+ optional model/day groups) for a time range.
 * Empty (zero requests) is its own state — never rendered as $0.00 fact.
 */
export function useUsageSummary(range: RangeKey, groupBy?: "provider" | "model" | "day"): UsageState {
  const [value, setValue] = useState<UsageState>({ state: "loading" });
  useEffect(() => {
    const controller = new AbortController();
    setValue({ state: "loading" });
    const since = encodeURIComponent(rangeSince(range));
    const group = groupBy ? `&group_by=${groupBy}` : "";
    apiGet<UsageSummary>(`/api/usage/summary?since=${since}${group}`, { signal: controller.signal })
      .then((summary) => {
        if (controller.signal.aborted) return;
        setValue(summary.requests === 0 ? { state: "empty", summary } : { state: "ready", summary });
      })
      .catch((err: unknown) => {
        if (controller.signal.aborted) return;
        if (err instanceof DOMException && err.name === "AbortError") return;
        setValue({ state: "error", message: err instanceof Error ? err.message : "Usage unavailable." });
      });
    return () => {
      controller.abort();
    };
  }, [range, groupBy]);
  return value;
}

/**
 * Gateway presence probe (single copy). `null` while probing, `true` when
 * `/api/health` answers, `false` when the desktop app isn't running (preview
 * without gateway). Re-probed every 20s so the banner clears itself when a
 * restarted gateway comes back — a one-shot probe would latch `false`
 * forever on a page loaded during a restart. Pages show one banner for
 * `false` instead of a wall of per-section proxy errors.
 */
export function useGatewayStatus(): boolean | null {
  const [status, setStatus] = useState<boolean | null>(null);
  useEffect(() => {
    let cancelled = false;
    const probe = () => {
      apiGet<{ ok: boolean }>("/api/health")
        .then(() => {
          if (!cancelled) setStatus(true);
        })
        .catch(() => {
          if (!cancelled) setStatus(false);
        });
    };
    probe();
    const timer = setInterval(probe, 20_000);
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, []);
  return status;
}

/** Linked accounts across providers (single copy — Home and provider pages
 * share this hook instead of duplicating reload logic). */
export function useLinkedAccounts(): { accounts: LocalAccountMeta[]; reload: () => void } {
  const [accounts, setAccounts] = useState<LocalAccountMeta[]>([]);
  const reload = useCallback(() => {
    void listLinkedAccounts()
      .then(setAccounts)
      .catch(() => setAccounts([]));
  }, []);
  useEffect(reload, [reload]);
  return { accounts, reload };
}

/** Per-account quota refresh with inline errors (single copy). */
export function useQuotaRefresh(onDone?: () => void): {
  refreshing: string | null;
  error: string | null;
  refresh: (account: LocalAccountMeta) => void;
} {
  const [refreshing, setRefreshing] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(
    (account: LocalAccountMeta) => {
      if (refreshing) return;
      setError(null);
      setRefreshing(`${account.provider}:${account.accountId}`);
      void refreshQuotaForAccount(account)
        .then(() => onDone?.())
        .catch((err: unknown) => {
          setError(err instanceof Error ? err.message : "Quota refresh failed.");
        })
        .finally(() => setRefreshing(null));
    },
    [refreshing, onDone],
  );
  return { refreshing, error, refresh };
}
