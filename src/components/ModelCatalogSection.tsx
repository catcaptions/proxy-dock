import { useCallback, useEffect, useState } from "react";
import { apiGet } from "../api";
import type { CatalogResponse } from "../api";

type LoadState =
  | { state: "loading" }
  | { state: "empty"; reason: string | null }
  | { state: "ready"; data: CatalogResponse }
  | { state: "error"; message: string };

/**
 * Live model catalog per provider (native ids, serving-wire badges,
 * updated_at/stale). Four honest states — loading / empty / stale / error —
 * never fabricated entries. Refresh hits the token-gated live endpoint.
 * The catalog fetches live automatically whenever accounts are linked;
 * the plain cached read is only for the signed-out state.
 */
export default function ModelCatalogSection({ provider, label, hasAccounts }: { provider: string; label: string; hasAccounts: boolean }) {
  const [value, setValue] = useState<LoadState>({ state: "loading" });
  const [refreshing, setRefreshing] = useState(false);

  const load = useCallback(
    (refresh: boolean) => {
      if (refresh) setRefreshing(true);
      else setValue({ state: "loading" });
      apiGet<CatalogResponse>(`/api/models?provider=${encodeURIComponent(provider)}${refresh ? "&refresh=1" : ""}`)
        .then((data) => {
          if (data.models.length === 0) setValue({ state: "empty", reason: data.reason });
          else setValue({ state: "ready", data });
        })
        .catch((err: unknown) => {
          setValue({ state: "error", message: err instanceof Error ? err.message : "Catalog unavailable." });
        })
        .finally(() => setRefreshing(false));
    },
    [provider],
  );

  useEffect(() => {
    load(hasAccounts);
  }, [load, hasAccounts]);

  return (
    <section className="card" aria-label={`${label} models`} data-testid={`models-${provider}`}>
      <div className="breakdown-head">
        <h2>Models</h2>
        <button className="ghost-btn" disabled={refreshing} data-testid={`models-refresh-${provider}`} onClick={() => load(true)}>
          {refreshing ? "Refreshing…" : "Refresh catalog"}
        </button>
      </div>
      {value.state === "loading" ? <p className="muted">Loading catalog…</p> : null}
      {value.state === "empty" ? (
        <p className="muted" data-testid={`models-empty-${provider}`}>
          {value.reason ?? "No models cached yet."}
        </p>
      ) : null}
      {value.state === "error" ? (
        <p className="error" data-testid={`models-error-${provider}`}>
          {value.message}
        </p>
      ) : null}
      {value.state === "ready" ? (
        <>
          <p className="muted" data-testid={`models-updated-${provider}`}>
            {value.data.models.length} models
            {value.data.updated_at ? ` · updated ${value.data.updated_at} UTC` : ""}
            {value.data.stale ? " · stale" : ""}
            {value.data.models.some((m) => m.source === "observed")
              ? " · includes models proven by your requests"
              : ""}
          </p>
          <ul className="model-catalog">
            {value.data.models.map((m) => (
              <li key={m.nativeId} data-testid={`model-native-${m.nativeId}`}>
                <code>{m.nativeId}</code>
                {m.endpoints.map((wire) => (
                  <span key={wire} className="endpoint-badge" data-testid={`endpoint-badge-${m.nativeId}-${wire}`}>
                    {wire}
                  </span>
                ))}
              </li>
            ))}
          </ul>
        </>
      ) : null}
    </section>
  );
}
