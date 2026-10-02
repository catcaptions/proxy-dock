import { useEffect, useState } from "react";
import { apiPatch, apiPost, apiPut, clearCachedGatewayKey, gatewayBase, setPreviewGatewayKey } from "../api";
import { isTauri, maskKey } from "../auth";

/**
 * Settings: compact cards (Connection, Desktop, Data) with labeled rows and
 * one-line hints. All controls stay honest — the gateway URL is read-only
 * (discovered via the gateway_info Tauri command, same-origin fallback;
 * "unknown" when unreachable, never a hardcoded guess), the Proxy Dock key
 * row holds the one unified secret (app password + gateway bearer, masked +
 * Copy + confirm-gated Rotate, Set when missing), the Desktop card
 * renders only in the Tauri app (close behavior, launch-on-login, tray
 * actions via the parallel-stream Rust commands), and import applies
 * routing + pricing (accounts need live sign-in; secrets are never imported).
 */
export default function Settings() {
  const [gatewayUrl, setGatewayUrl] = useState<string | null>(null);
  const [gatewayLoaded, setGatewayLoaded] = useState(false);
  const [importState, setImportState] = useState<string | null>(null);
  const [importError, setImportError] = useState<string | null>(null);

  useEffect(() => {
    void gatewayBase()
      .then(setGatewayUrl)
      .catch(() => setGatewayUrl(null))
      .finally(() => setGatewayLoaded(true));
  }, []);

  const importFile = (file: File | undefined) => {
    if (!file) return;
    setImportState(null);
    setImportError(null);
    void file
      .text()
      .then(async (text) => {
        const bundle = JSON.parse(text) as {
          version?: unknown;
          routing?: Record<string, { accountId?: unknown; label?: unknown; priority?: unknown; enabled?: unknown }[]>;
          pricing?: { provider?: unknown; model?: unknown; price?: { prompt_per_1k?: unknown; completion_per_1k?: unknown }; source?: unknown }[];
          accounts?: unknown;
        };
        if (bundle.version !== 1 || typeof bundle.routing !== "object" || !bundle.routing) {
          throw new Error("Not a Proxy Dock export bundle (expected version 1 with routing).");
        }
        let appliedRouting = 0;
        for (const [provider, rows] of Object.entries(bundle.routing)) {
          if (!Array.isArray(rows)) continue;
          for (const row of rows) {
            if (typeof row.accountId !== "string" || !row.accountId) continue;
            const patch: Record<string, unknown> = {};
            if (typeof row.label === "string" && row.label) patch.label = row.label;
            if (typeof row.priority === "number") patch.priority = row.priority;
            if (typeof row.enabled === "boolean") patch.enabled = row.enabled;
            if (Object.keys(patch).length === 0) continue;
            await apiPatch(
              `/api/accounts/${encodeURIComponent(provider)}/${encodeURIComponent(row.accountId)}`,
              patch,
            );
            appliedRouting += 1;
          }
        }
        let appliedPricing = 0;
        let pricingSkipped = bundle.pricing === undefined;
        if (Array.isArray(bundle.pricing)) {
          for (const entry of bundle.pricing) {
            if (typeof entry.provider !== "string" || typeof entry.model !== "string" || !entry.provider || !entry.model) continue;
            const price = entry.price as { prompt_per_1k?: unknown; completion_per_1k?: unknown } | undefined;
            if (typeof price?.prompt_per_1k !== "number" || typeof price?.completion_per_1k !== "number") continue;
            await apiPut(
              `/api/pricing/${encodeURIComponent(entry.provider)}/${encodeURIComponent(entry.model)}`,
              { prompt_per_1k: price.prompt_per_1k, completion_per_1k: price.completion_per_1k, source: typeof entry.source === "string" ? entry.source : "import" },
            );
            appliedPricing += 1;
          }
        }
        const accountsNote = Array.isArray(bundle.accounts) && bundle.accounts.length > 0 ? "accounts need sign-in" : "accounts skipped";
        const pricingNote = pricingSkipped ? "pricing skipped" : `${appliedPricing} pricing`;
        setImportState(`Applied ${appliedRouting} routing, ${pricingNote}. ${accountsNote}.`);
      })
      .catch((err: unknown) => {
        setImportError(err instanceof Error ? err.message : "Import failed.");
      });
  };

  return (
    <div className="page" data-testid="page-settings">
      <h1>Settings</h1>
      <section className="card" aria-label="Connection settings">
        <h2>Connection</h2>
        <div className="settings-row">
          <label htmlFor="gateway-url">Gateway URL</label>
          <div className="settings-field">
            <input
              id="gateway-url"
              readOnly
              value={!gatewayLoaded ? "Detecting…" : (gatewayUrl ?? "unknown — gateway unreachable")}
              data-testid="gateway-url"
            />
            <p className="muted settings-hint">
              Discovered from the running gateway, never assumed. Falls back to an ephemeral port when 11434 is busy.
            </p>
          </div>
        </div>
        <div className="settings-row">
          <label htmlFor="gateway-key">Proxy Dock key</label>
          <div className="settings-field">
            <GatewayKeyField />
          </div>
        </div>
        <div className="settings-row">
          <label htmlFor="antigravity-secret">Antigravity secret</label>
          <div className="settings-field">
            <AntigravitySecretField />
          </div>
        </div>
      </section>
      {isTauri() ? <DesktopSettings /> : null}
      <section className="card" aria-label="Data settings">
        <h2>Data</h2>
        <div className="settings-row">
          <label htmlFor="import-routing">Import routing</label>
          <div className="settings-field">
            <input
              id="import-routing"
              type="file"
              accept="application/json"
              data-testid="import-file"
              onChange={(e) => {
                const file = e.target.files?.[0];
                e.target.value = "";
                importFile(file);
              }}
            />
            <p className="muted settings-hint">
              From an export bundle. Applies routing + pricing — accounts need sign-in.
            </p>
            {importState ? <p className="muted" data-testid="import-status">{importState}</p> : null}
            {importError ? <p className="error">{importError}</p> : null}
          </div>
        </div>
        <div className="settings-row">
          <span id="pricing-data-label">Pricing data</span>
          <div className="settings-field" aria-labelledby="pricing-data-label">
            <p className="muted settings-hint settings-hint-first">
              Fills missing model prices from the Models.dev catalog. Existing prices are never overwritten.
            </p>
            <PricingSync />
          </div>
        </div>
      </section>
    </div>
  );
}

type CloseBehavior = "tray" | "quit";

/** One compact key row: masked value + Copy + confirm-gated Rotate, or a Set
 * input when no key is known (first run, or preview after a 401 cleared it).
 * Desktop reads via IPC (memory only); preview preview keeps a session copy. */
function GatewayKeyField() {
  const [key, setKey] = useState<string | null>(null);
  const [fromEnv, setFromEnv] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => {
    setError(null);
    void (async () => {
      if (isTauri()) {
        try {
          const { invoke } = await import("@tauri-apps/api/core");
          const info = (await invoke("get_gateway_key")) as { key?: unknown; from_env?: unknown };
          setKey(typeof info.key === "string" && info.key ? info.key : null);
          setFromEnv(info.from_env === true);
        } catch (err: unknown) {
          setError(err instanceof Error ? err.message : "Key unavailable.");
          setKey(null);
        } finally {
          setLoaded(true);
        }
        return;
      }
      try {
        setKey(sessionStorage.getItem("proxydock-gateway-key"));
      } catch {
        setKey(null);
      } finally {
        setLoaded(true);
      }
    })();
  };

  useEffect(load, []);

  const applyKey = (next: string) => {
    setKey(next);
    setDraft("");
    setNotice(null);
    clearCachedGatewayKey();
  };

  const fail = (err: unknown, fallback: string) => {
    setError(err instanceof Error ? err.message : fallback);
  };

  const setKeyFromDraft = () => {
    const secret = draft.trim();
    if (!secret) return;
    setError(null);
    setNotice(null);
    setBusy(true);
    void (async () => {
      try {
        if (isTauri()) {
          const { invoke } = await import("@tauri-apps/api/core");
          await invoke("set_gateway_key", { key: secret });
          applyKey(secret);
        } else {
          setPreviewGatewayKey(secret);
          applyKey(secret);
        }
      } catch (err: unknown) {
        fail(err, "Could not set the key.");
      } finally {
        setBusy(false);
      }
    })();
  };

  const rotate = () => {
    if (!window.confirm("Rotating breaks existing client configs. Continue?")) return;
    setError(null);
    setNotice(null);
    setBusy(true);
    void (async () => {
      try {
        if (isTauri()) {
          const { invoke } = await import("@tauri-apps/api/core");
          const info = (await invoke("rotate_gateway_key")) as { key?: unknown };
          if (typeof info.key !== "string" || !info.key) throw new Error("Rotate returned no key.");
          setKey(info.key);
          setDraft("");
          setNotice("Rotated — update client configs.");
          clearCachedGatewayKey();
        }
      } catch (err: unknown) {
        fail(err, "Could not rotate the key.");
      } finally {
        setBusy(false);
      }
    })();
  };

  const copy = () => {
    if (!key) return;
    setError(null);
    void navigator.clipboard
      ?.writeText(key)
      .then(() => setNotice("Copied."))
      .catch(() => setError("Copy failed — select the key manually."));
  };

  if (!loaded) return <p className="muted settings-hint">Detecting…</p>;
  if (!key) {
    return (
      <>
        <input
          id="gateway-key"
          type="password"
          value={draft}
          disabled={busy}
          placeholder="Paste the Proxy Dock key"
          data-testid="gateway-key-set-input"
          onChange={(e) => setDraft(e.target.value)}
        />
        <button data-testid="gateway-key-set" disabled={busy || !draft.trim()} onClick={setKeyFromDraft}>
          Set key
        </button>
        <p className="muted settings-hint">App password + gateway bearer. Preview: paste the PROXYDOCK_GATEWAY_KEY the gateway runs with.</p>
        {error ? <p className="error">{error}</p> : null}
        {notice ? <p className="muted">{notice}</p> : null}
      </>
    );
  }
  return (
    <>
      <input id="gateway-key" readOnly value={maskKey(key)} data-testid="gateway-key" />
      <div>
        <button data-testid="gateway-key-copy" onClick={copy}>Copy</button>{" "}
        {isTauri() && !fromEnv ? (
          <button data-testid="gateway-key-rotate" disabled={busy} onClick={rotate}>Rotate</button>
        ) : null}
      </div>
      <p className="muted settings-hint">
        {fromEnv ? "From PROXYDOCK_GATEWAY_KEY — read-only here." : "App password + gateway bearer. External clients send it as Authorization: Bearer."}
      </p>
      {error ? <p className="error">{error}</p> : null}
      {notice ? <p className="muted">{notice}</p> : null}
    </>
  );
}

/** Frontend invoke for the app's own Rust commands (needs no capability entry). */
async function invokeDesktop(cmd: string, args: Record<string, unknown> = {}): Promise<void> {
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke(cmd, args);
}

/** Desktop-only card (never rendered in browser preview): close behavior,
 * launch-on-login, and tray actions. Close defaults to tray. */
/**
 * Antigravity OAuth client secret: presence-only (the value is never shown).
 * Desktop stores it in the vault; browser preview needs
 * PROXYDOCK_ANTIGRAVITY_SECRET on the dev server instead.
 */
function AntigravitySecretField() {
  const [present, setPresent] = useState<boolean | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isTauri()) {
      setPresent(null);
      return;
    }
    void (async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        setPresent((await invoke("antigravity_secret_status")) === true);
      } catch (err: unknown) {
        setError(err instanceof Error ? err.message : "Secret unavailable.");
        setPresent(false);
      }
    })();
  }, []);

  if (!isTauri()) {
    return (
      <p className="muted settings-hint settings-hint-first">
        Set PROXYDOCK_ANTIGRAVITY_SECRET for the dev server.
      </p>
    );
  }

  const save = () => {
    const secret = draft.trim();
    if (!secret) return;
    setError(null);
    setNotice(null);
    setBusy(true);
    void (async () => {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        await invoke("set_antigravity_secret", { secret });
        setDraft("");
        setPresent(true);
        setNotice("Saved.");
      } catch (err: unknown) {
        setError(err instanceof Error ? err.message : "Save failed.");
      } finally {
        setBusy(false);
      }
    })();
  };

  return (
    <div>
      {present === true ? (
        <p className="muted settings-hint settings-hint-first" data-testid="antigravity-secret-present">
          Stored.
        </p>
      ) : (
        <p className="muted settings-hint settings-hint-first">
          Needed for Antigravity sign-in. Paste once, stored in the vault.
        </p>
      )}
      <input
        id="antigravity-secret"
        type="password"
        autoComplete="off"
        value={draft}
        maxLength={128}
        disabled={busy}
        placeholder="Paste client secret"
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") save();
        }}
      />
      <button type="button" disabled={busy || !draft.trim()} onClick={save}>
        Save
      </button>
      {notice ? <p className="muted">{notice}</p> : null}
      {error ? <p className="error">{error}</p> : null}
    </div>
  );
}

function DesktopSettings() {
  const [behavior, setBehavior] = useState<CloseBehavior>("tray");
  const [behaviorBusy, setBehaviorBusy] = useState(false);
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const { isEnabled } = await import("@tauri-apps/plugin-autostart");
        const enabled = await isEnabled();
        if (!cancelled) setAutostart(enabled);
      } catch {
        if (!cancelled) setAutostart(null);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const changeBehavior = (next: CloseBehavior) => {
    setBehavior(next);
    setError(null);
    setBehaviorBusy(true);
    void invokeDesktop("set_close_behavior", { behavior: next })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Could not save close behavior.");
      })
      .finally(() => setBehaviorBusy(false));
  };

  const toggleAutostart = (next: boolean) => {
    setError(null);
    setAutostartBusy(true);
    void (async () => {
      const { enable, disable } = await import("@tauri-apps/plugin-autostart");
      if (next) await enable();
      else await disable();
      setAutostart(next);
    })()
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Could not change launch-on-login.");
      })
      .finally(() => setAutostartBusy(false));
  };

  const desktopAction = (cmd: string, failure: string) => {
    setError(null);
    void invokeDesktop(cmd, {}).catch((err: unknown) => {
      setError(err instanceof Error ? err.message : failure);
    });
  };

  return (
    <section className="card" aria-label="Desktop settings">
      <h2>Desktop</h2>
      <div className="settings-row">
        <label htmlFor="close-behavior">When closing</label>
        <div className="settings-field">
          <select
            id="close-behavior"
            data-testid="close-behavior"
            value={behavior}
            disabled={behaviorBusy}
            onChange={(e) => changeBehavior(e.target.value as CloseBehavior)}
          >
            <option value="tray">Minimize to tray</option>
            <option value="quit">Quit app</option>
          </select>
          <p className="muted settings-hint">Close hides to the tray, or quits outright.</p>
        </div>
      </div>
      <div className="settings-row">
        <label htmlFor="launch-login">Launch at login</label>
        <div className="settings-field">
          <input
            id="launch-login"
            type="checkbox"
            data-testid="launch-login"
            checked={autostart ?? false}
            disabled={autostart === null || autostartBusy}
            onChange={(e) => toggleAutostart(e.target.checked)}
          />
          <p className="muted settings-hint">
            {autostart === null ? "Detecting…" : "Start Proxy Dock when you sign in."}
          </p>
        </div>
      </div>
      <div className="settings-row">
        <span id="desktop-actions-label">Window</span>
        <div className="settings-field" aria-labelledby="desktop-actions-label">
          <button data-testid="minimize-tray" onClick={() => desktopAction("hide_main_window", "Could not minimize to tray.")}>
            Minimize to tray
          </button>
          <button data-testid="quit-app" onClick={() => desktopAction("quit_app", "Could not quit.")}>
            Quit Proxy Dock
          </button>
          {error ? <p className="error">{error}</p> : null}
        </div>
      </div>
    </section>
  );
}

/** One-button models.dev refresh: status line only, no clutter. */
function PricingSync() {
  const [running, setRunning] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [unmatched, setUnmatched] = useState<string[]>([]);

  const sync = () => {
    setError(null);
    setStatus(null);
    setUnmatched([]);
    setRunning(true);
    void apiPost<{ inserted: number; refreshed: number; skipped: number; unmatched: string[] }>(
      "/api/pricing/sync",
    )
      .then((r) => {
        const when = new Date().toLocaleTimeString();
        setStatus(
          `Updated ${when} — ${r.inserted} new, ${r.refreshed} refreshed, ${r.skipped} already priced, ${r.unmatched.length} without match.`,
        );
        setUnmatched(r.unmatched);
      })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Pricing update failed.");
      })
      .finally(() => setRunning(false));
  };

  return (
    <>
      <button data-testid="pricing-sync" disabled={running} onClick={sync}>
        {running ? "Updating…" : "Update pricing data"}
      </button>
      {status ? <p className="muted" data-testid="pricing-sync-status">{status}</p> : null}
      {error ? <p className="error" data-testid="pricing-sync-error">{error}</p> : null}
      {unmatched.length > 0 ? (
        <details>
          <summary className="muted" data-testid="pricing-sync-unmatched-toggle">
            {unmatched.length} models without a published price
          </summary>
          <p className="muted" data-testid="pricing-sync-unmatched">
            {unmatched.join(", ")}
          </p>
        </details>
      ) : null}
    </>
  );
}