import type { LocalAccountMeta, QuotaRow } from "../auth";
import { quotaTone } from "./quotaTone";
import QuotaSparkline from "./QuotaSparkline";

export type AccountBlockProps = {
  account: LocalAccountMeta;
  iconSrc: string;
  /** Censored one-line identity (`las•••@g•••.com`, `•••••••xyz`). */
  title: string;
  /** Full identity — hover tooltip only, never rendered as text. */
  fullTitle: string;
  plan: string | null;
  quota: QuotaRow[] | null;
  /** Compact nudge shown only for never-verified keys with no data yet. */
  showUnverifiedHint: boolean;
  refreshing: boolean;
  refreshTitle: string;
  onRefresh: () => void;
  /** Omit to hide Remove (e.g. overview pages — manage on provider pages). */
  onRemove?: (() => void) | null;
  /** When set, the title navigates (e.g. Home blocks open the provider). */
  onTitleClick?: (() => void) | null;
  /** Series color of the account's provider — renders a small identity dot. Omit to hide. */
  providerColor?: string | null;
};

/**
 * Account block in the style of the quota-card reference: identity header,
 * plan pill, limit rows with bars (info rows without percentages render as
 * plain meta lines), and pill Refresh quota button. Limit rows render only
 * from real provider data (account.quota) — no rows means no quota data,
 * never fabricated bars. Shared by provider pages and Home so both stay
 * identical.
 */
export default function AccountBlock(props: AccountBlockProps) {
  const { account } = props;
  const quota = props.quota ?? [];
  const ns = `${account.provider}-${account.accountId}`;
  return (
    <section
      className="card account-block"
      aria-label={`${props.title} ending ${account.keySuffix}`}
      data-testid={`account-${ns}`}
    >
      <div className="account-identity">
        {props.providerColor ? (
          <span
            className="account-dot"
            aria-hidden="true"
            data-testid={`account-dot-${ns}`}
            style={{ background: props.providerColor }}
          />
        ) : null}
        <img src={props.iconSrc} alt="" aria-hidden="true" className="account-icon" />
        {props.onTitleClick ? (
          <button
            type="button"
            className="account-name account-name-btn"
            data-testid={`account-email-${ns}`}
            title={props.fullTitle}
            onClick={props.onTitleClick}
          >
            {props.title}
          </button>
        ) : (
          <div className="account-name" data-testid={`account-email-${ns}`} title={props.fullTitle}>
            {props.title}
          </div>
        )}
      </div>
      {props.plan ? (
        <div className="account-plan">
          <span className="muted">Plan</span>
          {/pro/i.test(props.plan) ? (
            <strong className="plan-pill plan-pill-pro" data-testid={`account-plan-${ns}`}>{props.plan}</strong>
          ) : (
            <strong className="plan-value" data-testid={`account-plan-${ns}`}>{props.plan}</strong>
          )}
        </div>
      ) : null}
      {quota.map((row, index) => {
        const pct = typeof row.remainingPct === "number" ? Math.max(0, Math.min(100, row.remainingPct)) : null;
        const tone = quotaTone(row.remainingPct);
        const isInfo = pct == null;
        const isResetCredit = row.id.startsWith("reset-credit:");
        const prevIsReset = index > 0 && quota[index - 1].id.startsWith("reset-credit:");
        if (isResetCredit) {
          return (
            <div key={row.id}>
              {!prevIsReset ? <div className="limit-label reset-expiry-label">Manual reset expiry</div> : null}
              <div className="reset-credit-box" data-testid={`quota-${ns}-${row.id}`}>
                <span className="reset-credit-name">{row.label}</span>
                <span className="reset-credit-meta">
                  {row.resetText ? <span className="muted">{row.resetText}</span> : null}
                  {row.resetText && row.resetInText ? <span className="muted"> · </span> : null}
                  {row.resetInText ? <span className={row.urgent ? "limit-soon" : "muted"}>{row.resetInText}</span> : null}
                </span>
              </div>
            </div>
          );
        }
        return (
          <div className={`limit-row${isInfo ? " limit-row-info" : ""}`} key={row.id} data-testid={`quota-${ns}-${row.id}`}>
            <div className="limit-head">
              <span className="limit-label">{row.label}</span>
              <span className="limit-meta">
                {pct != null ? <strong>{Math.round(pct)}%</strong> : null}
                <QuotaSparkline provider={account.provider} accountId={account.accountId} rowId={row.id} />
                {row.resetText ? <span className="muted">{row.resetText}</span> : null}
                {row.resetInText ? (
                  <span className={row.urgent ? "limit-soon" : "muted"}>· {row.resetInText}</span>
                ) : null}
              </span>
            </div>
            {pct != null ? (
              <div
                className="bar-track limit-bar"
                role="progressbar"
                aria-valuenow={pct}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-label={row.label}
              >
                <div className={`bar-fill limit-fill-${tone}`} style={{ width: `${pct}%` }} />
              </div>
            ) : null}
          </div>
        );
      })}
      {props.showUnverifiedHint ? (
        <div className="account-status muted" data-testid={`account-status-${ns}`}>
          Not verified yet — open Sign In above to verify.
        </div>
      ) : null}
      <div className="account-actions">
        {props.onRemove ? (
          <button className="ghost-btn account-remove" data-testid={`remove-${ns}`} onClick={props.onRemove}>
            Remove
          </button>
        ) : null}
        <button
          className="refresh-btn"
          title={props.refreshTitle}
          aria-label="Refresh quota"
          data-testid={`refresh-${ns}`}
          disabled={props.refreshing}
          onClick={props.onRefresh}
        >
          <span aria-hidden="true">↻</span> Refresh quota
        </button>
      </div>
    </section>
  );
}
