import { useMemo, useState } from "react";
import type { LocalAccountMeta } from "../auth";
import { displayIdentity, displayPlanFor, fullIdentity } from "../auth";
import type { RoutingAccount } from "../api";
import { quotaTone as barTone } from "./quotaTone";
import QuotaSparkline from "./QuotaSparkline";

/**
 * Provider-page account list matching the flat-list reference: no cards,
 * no sidebar. One row per account — censored identity + plan/back-in on the
 * left, the limits (5hr / weekly / total) in horizontal columns on the
 * right. Rows render only from real provider data.
 */
export type ProviderQuotaListProps = {
  accounts: LocalAccountMeta[];
  iconSrc: string;
  providerLabel: string;
  refreshing: boolean;
  onRefresh: (account: LocalAccountMeta) => void;
  onRemove: (account: LocalAccountMeta) => void;
  /** Backend-store status per account id. Absent = don't render (e.g. desktop). */
  backendLink?: Record<string, BackendLinkInfo>;
  onRetryBackend?: (account: LocalAccountMeta) => void;
  /** Canonical routing rows per account id (GET /api/routing/accounts). Absent = no editor. */
  routing?: Record<string, RoutingAccount>;
  onPatchRouting?: (account: LocalAccountMeta, patch: { label?: string; priority?: number; enabled?: boolean }) => void;
  /** Reordered account ids (visual order) after a drag/keyboard move. Absent = no drag handles. */
  onReorder?: (orderedIds: string[]) => void;
};

/** Whether a signed-in account actually reached the gateway store. */
export type BackendLinkState = "linked" | "linking" | "unreachable" | "rejected";
export type BackendLinkInfo = { state: BackendLinkState; message: string | null };

function shortBackIn(resetInText: string): string {
  const t = resetInText.trim();
  if (!t) return "";
  const m = t.match(/^in\s+(\d+)\s+(minute|minutes|hour|hours|day|days)/i);
  if (!m) return `back ${t}`;
  const n = m[1];
  const unit = m[2].toLowerCase();
  const short = unit.startsWith("minute") ? "m" : unit.startsWith("hour") ? "h" : "d";
  return `back in ${n}${short}`;
}

/**
 * Precise `back in 17h 21m` / `3d 2h` / `3h 21m` derived from the row's
 * `MM/DD, HH:mm` reset stamp. Falls back to the coarse relative text when
 * the stamp cannot be parsed.
 */
export function preciseBackIn(resetText: string, fallbackInText: string): string {
  const m = resetText.trim().match(/^(\d{1,2})\/(\d{1,2}),\s*(\d{1,2}):(\d{2})$/);
  if (m) {
    const now = new Date();
    let reset = new Date(now.getFullYear(), Number(m[1]) - 1, Number(m[2]), Number(m[3]), Number(m[4]));
    if (reset.getTime() < now.getTime() - 12 * 3600 * 1000) {
      reset = new Date(now.getFullYear() + 1, Number(m[1]) - 1, Number(m[2]), Number(m[3]), Number(m[4]));
    }
    const diff = reset.getTime() - now.getTime();
    // Year-wrap guard (Dec→Jan): a stamp 90d+ out is a stale previous-year
    // date (e.g. Dec stamp read in Jan), not a real future reset — fall back
    // to the coarse text instead of "back in 300d".
    if (diff > 90 * 86_400_000) return shortBackIn(fallbackInText);
    if (diff > 0) {
      const totalMins = Math.floor(diff / 60000);
      const d = Math.floor(totalMins / 1440);
      const h = Math.floor((totalMins % 1440) / 60);
      const min = totalMins % 60;
      if (d > 0) return h > 0 ? `back in ${d}d ${h}h` : `back in ${d}d`;
      if (h > 0) return min > 0 ? `back in ${h}h ${min}m` : `back in ${h}h`;
      return `back in ${Math.max(1, min)}m`;
    }
  }
  return shortBackIn(fallbackInText);
}

export default function ProviderQuotaList(props: ProviderQuotaListProps) {
  const { accounts, providerLabel } = props;
  // Visual order follows routing priority (stable — ties keep roster order).
  const ordered = useMemo(() => {
    const prio = (id: string) => props.routing?.[id]?.priority ?? 1;
    return [...accounts].sort((a, b) => prio(a.accountId) - prio(b.accountId));
  }, [accounts, props.routing]);
  const reorderable = !!props.onReorder && ordered.length > 1;
  const [dragId, setDragId] = useState<string | null>(null);
  const [drop, setDrop] = useState<{ id: string; after: boolean } | null>(null);
  const clearDrag = () => {
    setDragId(null);
    setDrop(null);
  };
  const reorderTo = (id: string, targetIndex: number) => {
    const rest = ordered.map((a) => a.accountId).filter((x) => x !== id);
    rest.splice(Math.max(0, Math.min(targetIndex, rest.length)), 0, id);
    props.onReorder?.(rest);
  };
  const dropIndex = (targetId: string, after: boolean) => {
    const rest = ordered.map((a) => a.accountId).filter((x) => x !== dragId);
    return rest.indexOf(targetId) + (after ? 1 : 0);
  };
  return (
    <section className="quota-list" aria-label={`${providerLabel} accounts and limits`} data-testid="provider-quota-list">
      {ordered.map((account) => {
        const quota = account.quota ?? [];
        const bars = quota.filter((r) => typeof r.remainingPct === "number");
        const info = quota.filter((r) => r.remainingPct == null);
        const plan = displayPlanFor(account);
        const exhausted = bars.filter((r) => (r.remainingPct ?? 100) <= 0);
        const backSource = exhausted.length > 0 ? exhausted[0] : null;
        const backIn = backSource ? preciseBackIn(backSource.resetText, backSource.resetInText) : "";
        const ns = `${account.provider}-${account.accountId}`;
        const route = props.routing?.[account.accountId] ?? null;
        return (
          <div
            key={`${account.provider}-${account.accountId}`}
            className={[
              "quota-list-row",
              dragId === account.accountId ? "dragging" : "",
              drop?.id === account.accountId ? (drop.after ? "drop-after" : "drop-before") : "",
            ].join(" ")}
            data-testid={`qp-account-${ns}`}
            onDragOver={(e) => {
              if (!reorderable || !dragId || dragId === account.accountId) return;
              e.preventDefault();
              e.dataTransfer.dropEffect = "move";
              const rect = e.currentTarget.getBoundingClientRect();
              setDrop({ id: account.accountId, after: e.clientY > rect.top + rect.height / 2 });
            }}
            onDragLeave={(e) => {
              if (!e.currentTarget.contains(e.relatedTarget as Node)) {
                setDrop((d) => (d?.id === account.accountId ? null : d));
              }
            }}
            onDrop={(e) => {
              e.preventDefault();
              if (!reorderable || !dragId || dragId === account.accountId) {
                clearDrag();
                return;
              }
              const rect = e.currentTarget.getBoundingClientRect();
              const after = e.clientY > rect.top + rect.height / 2;
              const at = dropIndex(account.accountId, after);
              const id = dragId;
              clearDrag();
              reorderTo(id, at);
            }}
          >
            <div className="quota-list-identity">
              <div className="quota-list-name" data-testid={`qp-account-email-${ns}`} title={fullIdentity(account)}>
                {displayIdentity(account)}
              </div>
              <div className="quota-list-sub">
                <span className="quota-list-plan" data-testid={`qp-account-plan-${ns}`}>
                  {plan ?? "—"}
                </span>
                {backIn ? <span className="quota-list-backin"> · {backIn}</span> : null}
                {route?.isDefault ? (
                  <span className="muted" data-testid={`routing-default-${ns}`}>
                    {" · default"}
                  </span>
                ) : null}
                {props.backendLink ? (
                  <BackendLinkStatus
                    account={account}
                    ns={ns}
                    info={props.backendLink[account.accountId] ?? null}
                    onRetry={props.onRetryBackend ? () => props.onRetryBackend?.(account) : null}
                  />
                ) : null}
              </div>
              <div className="quota-list-row-actions">
                {reorderable ? (
                  <button
                    type="button"
                    className="drag-grip"
                    draggable
                    data-testid={`qp-drag-${ns}`}
                    title="Drag to reorder"
                    aria-label={`Reorder ${fullIdentity(account)}`}
                    onDragStart={(e) => {
                      e.dataTransfer.effectAllowed = "move";
                      try {
                        e.dataTransfer.setData("text/plain", account.accountId);
                      } catch {
                        // ignore (some browsers restrict setData timing)
                      }
                      setDragId(account.accountId);
                    }}
                    onDragEnd={clearDrag}
                    onKeyDown={(e) => {
                      if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
                      e.preventDefault();
                      const i = ordered.findIndex((a) => a.accountId === account.accountId);
                      reorderTo(account.accountId, i + (e.key === "ArrowUp" ? -1 : 1));
                    }}
                  >
                    <span aria-hidden="true">⋮⋮</span>
                  </button>
                ) : null}
                {route && props.onPatchRouting ? (
                  <label className="quota-list-enable" title={route.enabled ? "Routing on" : "Routing off"}>
                    <input
                      type="checkbox"
                      checked={route.enabled}
                      aria-label={`Enable ${fullIdentity(account)} for routing`}
                      data-testid={`routing-enable-${ns}`}
                      onChange={(e) => props.onPatchRouting?.(account, { enabled: e.target.checked })}
                    />
                  </label>
                ) : null}
                <button
                  className="quota-list-refresh"
                  data-testid={`qp-refresh-${ns}`}
                  disabled={props.refreshing}
                  title="Refresh live quota from the provider"
                  aria-label={`Refresh quota for ${fullIdentity(account)}`}
                  onClick={() => props.onRefresh(account)}
                >
                  <span aria-hidden="true">↻</span>
                </button>
                <button
                  className="quota-list-remove"
                  data-testid={`qp-remove-${ns}`}
                  title="Remove this account"
                  aria-label={`Remove ${fullIdentity(account)}`}
                  onClick={() => props.onRemove(account)}
                >
                  ×
                </button>
              </div>
            </div>
            <div className="quota-list-limits">
              {bars.length === 0 && info.length === 0 ? (
                <div className="muted quota-list-empty">No quota data yet.</div>
              ) : (
                <>
                {bars.map((row) => {
                  const pct = typeof row.remainingPct === "number" ? Math.max(0, Math.min(100, row.remainingPct)) : null;
                  const tone = barTone(row.remainingPct);
                  return (
                  <div className="quota-cell" key={row.id} data-testid={`qp-quota-${ns}-${row.id}`}>
                  <div className="quota-cell-head">
                    <span className="quota-cell-label">{row.label}</span>
                    <strong className="quota-cell-pct">{pct != null ? `${Math.round(pct)}%` : ""}</strong>
                    <QuotaSparkline provider={account.provider} accountId={account.accountId} rowId={row.id} />
                  </div>
                  {pct != null ? (
                    <div
                      className="bar-track quota-cell-bar"
                      role="progressbar"
                      aria-valuenow={pct}
                      aria-valuemin={0}
                      aria-valuemax={100}
                      aria-label={row.label}
                    >
                      <div className={`bar-fill quota-fill-${tone}`} style={{ width: `${pct}%` }} />
                    </div>
                  ) : null}
                  <div className="quota-cell-reset">
                    {row.resetText || row.resetInText ? (
                      <>
                        {row.resetInText ? (
                          <span className={row.urgent ? "limit-soon" : "quota-cell-in"}>{row.resetInText}</span>
                        ) : null}
                        {row.resetInText && row.resetText ? <span className="quota-cell-dot"> · </span> : null}
                        {row.resetText ? <span className="muted">{row.resetText}</span> : null}
                      </>
                    ) : (
                      <span className="muted">No reset pending</span>
                    )}
                  </div>
                </div>
                  );
                })}
                {info.map((row) => (
                  <div className="quota-cell-info" key={row.id} data-testid={`qp-quota-${ns}-${row.id}`}>
                    <span className="quota-cell-label">{row.label}</span>
                    <span className="quota-cell-info-meta">
                      {row.resetText ? <span className="muted">{row.resetText}</span> : null}
                      {row.resetText && row.resetInText ? <span className="quota-cell-dot"> · </span> : null}
                      {row.resetInText ? (
                        <span className={row.urgent ? "limit-soon" : "muted"}>{row.resetInText}</span>
                      ) : null}
                    </span>
                  </div>
                ))}
                </>
              )}
            </div>
          </div>
        );
      })}
    </section>
  );
}

/** One-line honest backend-store status. Renders nothing until checked. */
function BackendLinkStatus({
  account,
  ns,
  info,
  onRetry,
}: {
  account: LocalAccountMeta;
  ns: string;
  info: BackendLinkInfo | null;
  onRetry: (() => void) | null;
}) {
  if (!info) return null;
  if (info.state === "linked") {
    return (
      <span className="muted" data-testid={`qp-backend-${ns}`}>
        {" · backend connected"}
      </span>
    );
  }
  if (info.state === "linking") {
    return (
      <span className="muted" data-testid={`qp-backend-${ns}`}>
        {" · sending to backend…"}
      </span>
    );
  }
  return (
    <span className="limit-soon" data-testid={`qp-backend-${ns}`}>
      {` · ${info.message ?? "Backend not reached."}`}
      {onRetry ? (
        <button className="ghost-btn" data-testid={`qp-backend-retry-${ns}`} onClick={onRetry}>
          Retry
        </button>
      ) : null}
    </span>
  );
}
