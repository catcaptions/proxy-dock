/**
 * Single offline banner (plan §3.5): the exact Home `gateway-offline` markup,
 * rendered on Home AND provider pages. No new wording, no new styles.
 */
export default function OfflineBanner() {
  return (
    <section className="card" aria-label="Gateway unreachable" data-testid="gateway-offline">
      <h2>Gateway unreachable</h2>
      <p className="muted">
        Start the Proxy Dock desktop app for live usage. Your linked accounts and quota below keep working from local
        data.
      </p>
    </section>
  );
}
