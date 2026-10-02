import { useMemo, useState } from "react";
import type { UsageSummary } from "../api";
import { displayIdentity, displayPlanFor, fullIdentity } from "../auth";
import AccountBlock from "../components/AccountBlock";
import DailyCostChart from "../components/DailyCostChart";
import OfflineBanner from "../components/OfflineBanner";
import ProviderCostChart from "../components/ProviderCostChart";
import { PROVIDERS, PROVIDER_SERIES_COLORS, compact, formatDayShort } from "../data";
import { rangeSince, useGatewayStatus, useLinkedAccounts, useQuotaRefresh, useUsageSummary } from "../hooks";
import type { RangeKey } from "../hooks";

type Props = {
  onOpenProvider: (slug: string) => void;
};

function costText(summary: UsageSummary): string {
  if (summary.estimated_cost == null) return "Cost unknown — no pricing snapshots yet.";
  return `${fmtCost(summary.estimated_cost)}${summary.estimated ? " · API estimate" : ""}`;
}

/** Human window label for the range control (`Sep 22 – Sep 29`, never raw ISO). */
function rangeWindow(range: RangeKey): string {
  const start = rangeSince(range).slice(0, 10);
  const end = new Date().toISOString().slice(0, 10);
  return `${formatDayShort(start)} – ${formatDayShort(end)}`;
}

/** Sub-cent costs need 4 decimals or everything reads $0.00. */
function fmtCost(value: number): string {
  return value < 0.01 && value > 0 ? `$${value.toFixed(4)}` : `$${value.toFixed(2)}`;
}

export default function Home({ onOpenProvider }: Props) {
  const [range, setRange] = useState<RangeKey>("7d");
  const [breakdownTab, setBreakdownTab] = useState<"model" | "day">("model");
  const { accounts: linked, reload } = useLinkedAccounts();
  const { refreshing, error: refreshError, refresh } = useQuotaRefresh(reload);
  const usage = useUsageSummary(range);
  const modelGroups = useUsageSummary(range, "model");
  const dayGroups = useUsageSummary(range, "day");
  const providerGroups = useUsageSummary(range, "provider");
  const gateway = useGatewayStatus();
  const chartSince = useMemo(() => rangeSince(range), [range]);

  const linkedByProvider = new Map<string, typeof linked>();
  for (const account of linked) {
    const list = linkedByProvider.get(account.provider) ?? [];
    list.push(account);
    linkedByProvider.set(account.provider, list);
  }

  return (
    <div className="page" data-testid="page-home">
      <h1>Home</h1>
      <p className="muted">Cross-provider usage from the gateway. Costs are estimates unless reconciled.</p>
      {gateway === false ? <OfflineBanner /> : null}

      {linked.length === 0 ? (
        <section className="card" aria-label="No accounts signed in" data-testid="home-empty">
          <h2>No accounts signed in yet</h2>
          <p className="muted">Sign in to a provider to see usage, cost, and quota here.</p>
          <div className="cards">
            {PROVIDERS.map((provider) => (
              <article key={provider.slug} className="card" data-testid={`home-${provider.slug}`}>
                <h2>{provider.label}</h2>
                <p className="muted">{provider.signInTitle}</p>
                <button onClick={() => onOpenProvider(provider.slug)}>Sign in to {provider.label}</button>
              </article>
            ))}
          </div>
        </section>
      ) : (
        <>
          <div className="range-tabs" role="tablist" aria-label="Time range">
            {(["24h", "7d", "30d"] as RangeKey[]).map((key) => (
              <button
                key={key}
                role="tab"
                aria-selected={range === key}
                className={range === key ? "tab active" : "tab"}
                data-testid={`range-${key}`}
                onClick={() => setRange(key)}
              >
                {key}
              </button>
            ))}
            <span className="muted range-window">{rangeWindow(range)}</span>
          </div>

          <section className="card cost-overview" aria-label="Cost overview">
            <div className="cost-left">
              {usage.state === "loading" ? <p className="muted">Loading usage…</p> : null}
              {usage.state === "error" ? (
                <p className="error" data-testid="home-usage-error">
                  {usage.message}
                </p>
              ) : null}
              {usage.state === "empty" ? (
                <p className="muted" data-testid="daily-empty">
                  No gateway traffic in this range yet — usage appears here after signed-in requests flow.
                </p>
              ) : null}
              {usage.state === "ready" ? (
                <>
                  <div className="cost-total" data-testid="cost-total">
                    {usage.summary.estimated_cost == null ? "—" : fmtCost(usage.summary.estimated_cost)}
                  </div>
                  <div className="muted">
                    {usage.summary.requests} requests · {costText(usage.summary)}
                  </div>
                </>
              ) : null}
              {refreshError ? (
                <p className="error" data-testid="home-refresh-error">
                  {refreshError}
                </p>
              ) : null}
            </div>
          </section>

          <div className="accounts-grid" data-testid="home-accounts" role="list" aria-label="Linked accounts">
            {PROVIDERS.flatMap((provider) =>
              (linkedByProvider.get(provider.slug) ?? []).map((account) => (
                <div role="listitem" key={`${provider.slug}:${account.accountId}`}>
                  <AccountBlock
                    account={account}
                    iconSrc={provider.icon}
                    title={displayIdentity(account)}
                    fullTitle={fullIdentity(account)}
                    plan={displayPlanFor(account)}
                    quota={account.quota ?? null}
                    providerColor={PROVIDER_SERIES_COLORS[provider.slug]}
                    showUnverifiedHint={!account.verifiedAt && !(account.quota ?? []).length}
                    refreshing={refreshing === `${account.provider}:${account.accountId}`}
                    refreshTitle={`Refresh quota for ${provider.label}`}
                    onRefresh={() => refresh(account)}
                    onTitleClick={() => onOpenProvider(provider.slug)}
                  />
                </div>
              )),
            )}
          </div>

          <section className="card" aria-label="Totals">
            <h2>Totals</h2>
            {usage.state === "loading" ? <p className="muted">Loading…</p> : null}
            {usage.state === "error" ? <p className="error">{usage.message}</p> : null}
            {usage.state === "empty" ? (
              <p className="muted" data-testid="totals-empty">
                No gateway traffic in this range.
              </p>
            ) : null}
            {usage.state === "ready" ? (
              <div className="totals-grid" data-testid="totals-grid">
                <div>
                  <span className="muted">Requests</span>
                  <strong>{usage.summary.requests}</strong>
                </div>
                <div>
                  <span className="muted">Prompt tokens</span>
                  <strong>{compact(usage.summary.prompt_tokens)}</strong>
                </div>
                <div>
                  <span className="muted">Completion tokens</span>
                  <strong>{compact(usage.summary.completion_tokens)}</strong>
                </div>
                <div>
                  <span className="muted">Total tokens</span>
                  <strong>{compact(usage.summary.prompt_tokens + usage.summary.completion_tokens)}</strong>
                </div>
                <div>
                  <span className="muted">Estimated cost</span>
                  <strong>{usage.summary.estimated_cost == null ? "unknown" : fmtCost(usage.summary.estimated_cost)}</strong>
                </div>
                <div>
                  <span className="muted">Cached input</span>
                  <strong>{compact((usage.summary.cached_tokens ?? 0) + (usage.summary.cache_creation_tokens ?? 0))}</strong>
                </div>
                <div>
                  <span className="muted">Cache savings</span>
                  <strong>{fmtCost(usage.summary.cache_savings ?? 0)}</strong>
                </div>
              </div>
            ) : null}
          </section>

          <div className="home-lower">
            <div className="home-lower-left">
              <ProviderCostChart state={providerGroups} />
              <section className="card scroll-box" aria-label="Breakdown">
            <div className="breakdown-head">
              <h2>Breakdown</h2>
              <div className="tabs" role="tablist" aria-label="Breakdown grouping">
                <button
                  role="tab"
                  aria-selected={breakdownTab === "model"}
                  className={breakdownTab === "model" ? "tab active" : "tab"}
                  onClick={() => setBreakdownTab("model")}
                >
                  Model
                </button>
                <button
                  role="tab"
                  aria-selected={breakdownTab === "day"}
                  className={breakdownTab === "day" ? "tab active" : "tab"}
                  onClick={() => setBreakdownTab("day")}
                >
                  Day
                </button>
              </div>
            </div>
            <div className="scroll-box-body">
            {breakdownTab === "model" ? (
              modelGroups.state === "loading" ? (
                <p className="muted">Loading…</p>
              ) : modelGroups.state === "error" ? (
                <p className="error" data-testid="breakdown-error">
                  {modelGroups.message}
                </p>
              ) : modelGroups.state === "empty" || (modelGroups.state === "ready" && (modelGroups.summary.groups ?? []).length === 0) ? (
                <p className="muted" data-testid="breakdown-empty">
                  No per-model usage yet.
                </p>
              ) : (
                <table className="breakdown-table" data-testid="breakdown-model">
                  <thead>
                    <tr>
                      <th scope="col">Model</th>
                      <th scope="col">Cost</th>
                      <th scope="col">Share (tokens)</th>
                      <th scope="col">Tokens</th>
                    </tr>
                  </thead>
                  <tbody>
                    {(modelGroups.state === "ready" ? (modelGroups.summary.groups ?? []) : []).map((row) => {
                      const tokens = row.prompt_tokens + row.completion_tokens;
                      const total =
                        modelGroups.state === "ready"
                          ? (modelGroups.summary.groups ?? []).reduce((n, g) => n + g.prompt_tokens + g.completion_tokens, 0)
                          : 0;
                      return (
                        <tr key={row.key} data-testid={`model-row-${row.key}`}>
                          <td>{row.key}</td>
                          <td>{row.estimated_cost == null ? "unknown" : fmtCost(row.estimated_cost)}</td>
                          <td>{total === 0 ? "—" : `${((tokens / total) * 100).toFixed(1)}%`}</td>
                          <td>{compact(tokens)}</td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              )
            ) : dayGroups.state === "loading" ? (
              <p className="muted">Loading…</p>
            ) : dayGroups.state === "error" ? (
              <p className="error" data-testid="breakdown-day-error">
                {dayGroups.message}
              </p>
            ) : dayGroups.state === "empty" || (dayGroups.state === "ready" && (dayGroups.summary.groups ?? []).length === 0) ? (
              <p className="muted" data-testid="breakdown-day-empty">
                No daily history yet.
              </p>
            ) : (
              <table className="breakdown-table" data-testid="breakdown-day">
                <thead>
                  <tr>
                    <th scope="col">Day</th>
                    <th scope="col">Cost</th>
                    <th scope="col">Tokens</th>
                  </tr>
                </thead>
                <tbody>
                  {(dayGroups.state === "ready" ? (dayGroups.summary.groups ?? []) : []).map((row) => (
                    <tr key={row.key} data-testid={`day-row-${row.key}`}>
                      <td>{row.key}</td>
                      <td>{row.estimated_cost == null ? "unknown" : fmtCost(row.estimated_cost)}</td>
                      <td>{compact(row.prompt_tokens + row.completion_tokens)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            </div>
          </section>
            </div>
            <div className="home-lower-right">
              <DailyCostChart since={chartSince} />
            </div>
          </div>
        </>
      )}
    </div>
  );
}
