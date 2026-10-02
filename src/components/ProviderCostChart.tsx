import type { UsageGroup } from "../api";
import { PROVIDERS } from "../data";
import type { UsageState } from "../hooks";

function fmtCost(value: number): string {
  return value < 0.01 && value > 0 ? `$${value.toFixed(4)}` : `$${value.toFixed(2)}`;
}

function labelFor(slug: string): string {
  return PROVIDERS.find((p) => p.slug === slug)?.label ?? slug;
}

/**
 * Cost by provider: one horizontal bar per provider, width scaled to the
 * largest known cost. Providers without a price show as unknown (never $0.00
 * as fact). Renders from live `/api/usage/summary?group_by=provider` data.
 */
export default function ProviderCostChart({ state }: { state: UsageState }) {
  const groups: UsageGroup[] =
    state.state === "ready" || state.state === "empty" ? (state.summary.groups ?? []) : [];
  const max = groups.reduce((n, g) => Math.max(n, g.estimated_cost ?? 0), 0);
  return (
    <section className="card scroll-box" aria-label="Cost by provider" data-testid="cost-chart">
      <h2>Cost by provider</h2>
      <div className="scroll-box-body">
      {state.state === "loading" ? <p className="muted">Loading…</p> : null}
      {state.state === "error" ? <p className="error">{state.message}</p> : null}
      {state.state !== "loading" && state.state !== "error" && groups.length === 0 ? (
        <p className="muted" data-testid="cost-chart-empty">
          No per-provider costs yet.
        </p>
      ) : null}
      {groups.map((g) => {
        const width = max > 0 && g.estimated_cost != null ? `${Math.max(2, (g.estimated_cost / max) * 100)}%` : "0%";
        const now = g.estimated_cost == null || max <= 0 ? 0 : Math.round((g.estimated_cost / max) * 100);
        return (
          <div key={g.key} className="bar-row" data-testid={`cost-bar-${g.key}`}>
            <span title={g.key}>{labelFor(g.key)}</span>
            <div className="bar-track" role="progressbar" aria-valuenow={now} aria-valuemin={0} aria-valuemax={100} aria-label={labelFor(g.key)}>
              <div className="bar-fill" style={{ width }} />
            </div>
            <span>{g.estimated_cost == null ? "unknown" : fmtCost(g.estimated_cost)}</span>
          </div>
        );
      })}
      </div>
    </section>
  );
}
