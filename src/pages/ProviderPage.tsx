import { useCallback, useEffect, useRef, useState } from "react";
import {
  clearOAuthFlow,
  completeAntigravitySignIn,
  completeAutoOAuth,
  completeClaudeSignIn,
  completeCodexSignIn,
  deriveAccountId,
  forgetAccount,
  getToken,
  isTauri,
  localAccountInfo,
  openExternal,
  readOAuthFlow,
  readPreviewOAuthCredential,
  readRoster,
  refreshQuotaForAccount,
  resolveAccountMeta,
  saveCredential,
  startBrowserSignIn,
  startCommandCodeBrowserFlow,
  startPreviewOAuth,
  storeOAuthCredential,
  upsertAccountMeta,
  verifyClaudeKey,
  verifyCommandCodeKey,
  verifyOpencodeKey,
  waitForLoopbackCode,
} from "../auth";
import type { LocalAccountInfo, LocalAccountMeta, OAuthCredential, OAuthFlowState } from "../auth";
import { apiDelete, apiGet, apiPatch, apiPost, isGatewayUnreachable, mirrorCredentialToGateway } from "../api";
import type { ImportedCredential, RoutingAccount } from "../api";
import { useGatewayStatus } from "../hooks";
import ModelCatalogSection from "../components/ModelCatalogSection";
import OfflineBanner from "../components/OfflineBanner";
import ProviderQuotaList from "../components/ProviderQuotaList";
import type { BackendLinkInfo } from "../components/ProviderQuotaList";
import type { Provider } from "../data";

export type ProviderPageProvider = Provider;

type Props = { slug: string; provider?: ProviderPageProvider };

const OAUTH_SLUGS = ["chatgpt", "antigravity", "claude"];

function metaFromTauriInfo(providerSlug: string, accountId: string, info: LocalAccountInfo): LocalAccountMeta | null {
  if (info.status !== "token-present" || !info.key_suffix) return null;
  return {
    provider: providerSlug,
    accountId,
    email: info.email ?? null,
    keySuffix: info.key_suffix,
    verifiedAt: null,
    modelCount: null,
  };
}

export default function ProviderPage({ slug, provider }: Props) {
  const [signInOpen, setSignInOpen] = useState(false);
  const [token, setToken] = useState("");
  const [callbackText, setCallbackText] = useState("");
  const [authUrl, setAuthUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [, setSaved] = useState(false);
  const [signingIn, setSigningIn] = useState(false);
  const [completing, setCompleting] = useState(false);
  const [verifying, setVerifying] = useState(false);
  const [, setVerifiedModels] = useState<string[] | null>(null);
  const [linkedAccounts, setLinkedAccounts] = useState<LocalAccountMeta[]>([]);
  const [refreshing, setRefreshing] = useState(false);
  const [copied, setCopied] = useState(false);
  const abortRef = useRef<AbortController | null>(null);
  // Matches the Rust 300s callback servers (secrets.rs wait_for_callback /
  // start_commandcode_sign_in) — the desktop await must unlatch on the same
  // budget as the preview waitForLoopbackCode default.
  const DESKTOP_SIGNIN_TIMEOUT_MS = 5 * 60 * 1000;
  const desktop = isTauri();
  const browserCapable = provider != null && (OAUTH_SLUGS.includes(provider.slug) || provider.slug === "commandcode");
  const isOpencode = provider?.slug === "opencode";

  // Render-read staleness fix: the OAuth pending flow is read once into state
  // (on panel open), not on every render. Saves after mount explicitly sync it.
  const [pendingFlow, setPendingFlow] = useState<OAuthFlowState | null>(() => {
    try {
      return !desktop && provider && OAUTH_SLUGS.includes(provider.slug) ? readOAuthFlow(provider.slug) : null;
    } catch {
      return null;
    }
  });

  useEffect(() => {
    if (signInOpen && !desktop && provider && OAUTH_SLUGS.includes(provider.slug)) {
      try {
        setPendingFlow(readOAuthFlow(provider.slug));
      } catch {
        setPendingFlow(null);
      }
    }
  }, [signInOpen, desktop, provider]);

  const reloadLinked = useCallback(async () => {
    if (!provider) return;
    const slug = provider.slug;
    // Every roster id plus the legacy "default" slot (pre-multi-account).
    // Same identity can never appear twice: upsertAccountMeta dedupes on
    // write, and this filters defensively on read.
    const roster = readRoster(slug);
    const ids = roster ?? ["default"];
    const out: LocalAccountMeta[] = [];
    for (const id of ids) {
      const account = await resolveAccountMeta(slug, id).catch(() => null);
      if (account && !out.some((a) => a.accountId === account.accountId)) out.push(account);
    }
    setLinkedAccounts(out);
  }, [provider]);

  useEffect(() => {
    void reloadLinked();
  }, [reloadLinked]);

  // Backend link: every signed-in account must exist in the gateway store
  // (OS keyring + account row) with zero clicks. The gateway is the source
  // of truth for hasToken; anything local-but-unlinked is sent automatically.
  const [backendLink, setBackendLink] = useState<Record<string, BackendLinkInfo>>({});
  const [routing, setRouting] = useState<Record<string, RoutingAccount>>({});
  const linkingRef = useRef(false);

  const ensureBackendLink = useCallback(async () => {
    if (!provider || linkedAccounts.length === 0) return;
    if (linkingRef.current) return;
    const slug = provider.slug;
    linkingRef.current = true;
    try {
      let rows: RoutingAccount[] = [];
      try {
        const res = await apiGet<{ accounts: RoutingAccount[] }>(
          `/api/routing/accounts?provider=${encodeURIComponent(slug)}`,
        );
        rows = res.accounts;
      } catch {
        setBackendLink((prev) => {
          const next = { ...prev };
          for (const a of linkedAccounts) {
            if (next[a.accountId]?.state !== "linked") {
              next[a.accountId] = { state: "unreachable", message: "Backend unreachable — will retry." };
            }
          }
          return next;
        });
        return;
      }
      const byId = new Map(rows.map((r) => [r.accountId, r]));
      setRouting(Object.fromEntries(byId));
      for (const a of linkedAccounts) {
        if (byId.get(a.accountId)?.hasToken) {
          setBackendLink((prev) => ({ ...prev, [a.accountId]: { state: "linked", message: null } }));
          continue;
        }
        if (isTauri()) {
          // Desktop stores straight to the keyring via Tauri — nothing local to send.
          setBackendLink((prev) => ({
            ...prev,
            [a.accountId]: { state: "rejected", message: "Not in the backend store — sign in again." },
          }));
          continue;
        }
        setBackendLink((prev) => ({ ...prev, [a.accountId]: { state: "linking", message: null } }));
        const oauth = readPreviewOAuthCredential(slug, a.accountId);
        const raw: string | null = getToken(slug, a.accountId);
        const secret = oauth ? JSON.stringify({ ...oauth, account_id: a.accountId }) : raw;
        if (!secret) {
          setBackendLink((prev) => ({
            ...prev,
            [a.accountId]: { state: "rejected", message: "No credential in this browser — sign in again here." },
          }));
          continue;
        }
        try {
          await apiPost<ImportedCredential>(`/api/accounts/${encodeURIComponent(slug)}/credentials`, { secret });
          setBackendLink((prev) => ({ ...prev, [a.accountId]: { state: "linked", message: null } }));
        } catch (err: unknown) {
          setBackendLink((prev) => ({
            ...prev,
            [a.accountId]:
              err !== null && isGatewayUnreachable(err)
                ? { state: "unreachable", message: "Backend unreachable — will retry." }
                : { state: "rejected", message: err instanceof Error ? err.message : "Backend rejected it." },
          }));
        }
      }
    } finally {
      linkingRef.current = false;
    }
  }, [provider, linkedAccounts]);

  useEffect(() => {
    void ensureBackendLink();
  }, [ensureBackendLink]);

  // Self-heal latched "unreachable" states: a page opened while the gateway
  // was down latches backendLink failures once (ensureBackendLink only runs
  // on account changes), so re-run the check when the gateway becomes
  // reachable again — mirroring the Home banner's re-probe. The single-flight
  // latch resets in ensureBackendLink's finally; never force-clear it here
  // (that would let two link passes overlap).
  const gatewayUp = useGatewayStatus();
  const gatewayWasUp = useRef<boolean | null>(null);
  useEffect(() => {
    if (gatewayUp === true && gatewayWasUp.current !== true) {
      void ensureBackendLink();
    }
    gatewayWasUp.current = gatewayUp;
  }, [gatewayUp, ensureBackendLink]);

  const retryBackendLink = useCallback(() => {
    void ensureBackendLink();
  }, [ensureBackendLink]);

  // Routing editor (plan §3B): optimistic PATCH + rollback, confirm-gated
  // DELETE. Compact — no new section chrome, reuses row styles in
  // ProviderQuotaList.
  const patchRouting = useCallback(
    async (account: LocalAccountMeta, patch: { label?: string; priority?: number; enabled?: boolean }) => {
      if (!provider) return;
      const prev = routing[account.accountId];
      setRouting((old) => ({
        ...old,
        [account.accountId]: { ...(old[account.accountId] ?? prev ?? { accountId: account.accountId, label: "", priority: 1, enabled: true, plan: null, hasToken: false, isDefault: false }), ...patch },
      }));
      try {
        const updated = await apiPatch<RoutingAccount>(
          `/api/accounts/${encodeURIComponent(provider.slug)}/${encodeURIComponent(account.accountId)}`,
          patch,
        );
        setRouting((old) => ({ ...old, [account.accountId]: updated }));
      } catch (err: unknown) {
        if (prev) setRouting((old) => ({ ...old, [account.accountId]: prev }));
        else {
          setRouting((old) => {
            const next = { ...old };
            delete next[account.accountId];
            return next;
          });
        }
        setError(err instanceof Error ? err.message : "Routing update failed.");
      }
    },
    [provider, routing],
  );

  // Drag reorder: priorities follow visual order (index). Optimistic set-all
  // + rollback, then refetch canonical rows so the default badge is honest.
  const reorderRouting = useCallback(
    async (orderedIds: string[]) => {
      if (!provider) return;
      const prev = routing;
      const next: Record<string, RoutingAccount> = { ...routing };
      for (let i = 0; i < orderedIds.length; i++) {
        const id = orderedIds[i];
        const cur =
          next[id] ??
          ({
            accountId: id,
            label: "",
            priority: 1,
            enabled: true,
            plan: null,
            hasToken: false,
            isDefault: false,
          } as RoutingAccount);
        if (cur.priority !== i) next[id] = { ...cur, priority: i };
      }
      setRouting(next);
      try {
        await Promise.all(
          orderedIds.map((id, i) => {
            if (routing[id] && routing[id].priority === i) return Promise.resolve();
            return apiPatch<RoutingAccount>(
              `/api/accounts/${encodeURIComponent(provider.slug)}/${encodeURIComponent(id)}`,
              { priority: i },
            );
          }),
        );
        const res = await apiGet<{ accounts: RoutingAccount[] }>(
          `/api/routing/accounts?provider=${encodeURIComponent(provider.slug)}`,
        );
        setRouting(Object.fromEntries(res.accounts.map((r) => [r.accountId, r])));
      } catch (err: unknown) {
        setRouting(prev);
        setError(err instanceof Error ? err.message : "Reorder failed.");
      }
    },
    [provider, routing],
  );

  const deleteRoutingAccount = useCallback(
    async (account: LocalAccountMeta) => {
      if (!provider) return;
      if (!window.confirm("Remove this account?")) return;
      try {
        await apiDelete(`/api/accounts/${encodeURIComponent(provider.slug)}/${encodeURIComponent(account.accountId)}`);
      } catch (err: unknown) {
        if (!isGatewayUnreachable(err)) {
          setError(err instanceof Error ? err.message : "Remove failed.");
          return;
        }
        // Gateway unreachable — still forget the local traces.
      }
      await forgetAccount(account.provider, account.accountId).catch(() => undefined);
      try {
        clearOAuthFlow(account.provider);
      } catch {
        // ignore
      }
      setLinkedAccounts((prev) => prev.filter((a) => a.accountId !== account.accountId));
    },
    [provider],
  );

  useEffect(() => {
    return () => {
      abortRef.current?.abort();
    };
  }, []);

  if (!provider) {
    return (
      <div className="page" data-testid="page-provider-missing">
        <h1>Unknown provider</h1>
        <p className="muted">No provider matches slug {slug}.</p>
      </div>
    );
  }

  const persistOAuthCredential = async (slug: string, credential: OAuthCredential) => {
    const accountId = await deriveAccountId(credential.email || null, credential.access_token);
    await storeOAuthCredential(slug, accountId, credential);
    // Headless UI has no Tauri IPC: mirror into the gateway store so serving,
    // quota, and catalog see the account. Best-effort (preview works local).
    if (!isTauri()) void mirrorCredentialToGateway(slug, JSON.stringify(credential));
    upsertAccountMeta({
      provider: slug,
      accountId,
      email: credential.email || null,
      keySuffix: credential.access_token.slice(-3),
      verifiedAt: new Date().toISOString(),
      modelCount: null,
      plan: null,
      authKind: "oauth",
      quota: null,
    });
    await reloadLinked();
  };

  const handleDesktopOAuth = (slug: string) => {
    setError(null);
    setSaved(false);
    setCopied(false);
    setSigningIn(true);
    const controller = new AbortController();
    abortRef.current?.abort();
    abortRef.current = controller;
    // NO DOUBLE-OPEN: Rust `start_*_sign_in` commands already open the system
    // browser via secrets.rs `open_browser` — frontend must not call
    // openExternal on the desktop path.
    // Abort/timeout parity with the preview path: race the Rust await against
    // cancel + the 5-minute Rust budget so "Waiting for browser…" never latches.
    const invoked = startBrowserSignIn(slug);
    const aborted: Promise<never> = new Promise((_, reject) => {
      if (controller.signal.aborted) {
        reject(new Error("Sign-in was cancelled."));
        return;
      }
      controller.signal.addEventListener("abort", () => reject(new Error("Sign-in was cancelled.")), { once: true });
    });
    const timedOut: Promise<never> = new Promise((_, reject) => {
      setTimeout(() => reject(new Error("Sign-in timed out after 5 minutes — try again.")), DESKTOP_SIGNIN_TIMEOUT_MS);
    });
    Promise.race([invoked, aborted, timedOut])
      .then(async (result) => {
        if (controller.signal.aborted) return;
        // The keyring now holds the credential under result.account.
        const info = await localAccountInfo(slug, result.account);
        if (info && typeof (info as LocalAccountInfo).key_suffix === "string") {
          const built = metaFromTauriInfo(slug, result.account, info as LocalAccountInfo);
          if (built) {
            if (!built.email && result.email) built.email = result.email;
            built.verifiedAt = new Date().toISOString();
            built.authKind = "oauth";
            upsertAccountMeta(built);
          }
        }
        await reloadLinked();
        setSaved(true);
        setError(null);
        setSignInOpen(false);
      })
      .catch((err: unknown) => {
        // Cancel leaves quietly (parity with the preview path); timeout and
        // Rust errors surface honestly.
        if (controller.signal.aborted) return;
        setError(err instanceof Error ? err.message : "Browser sign-in failed.");
      })
      .finally(() => {
        setSigningIn(false);
        if (abortRef.current === controller) abortRef.current = null;
      });
  };

  const cancelDesktopOAuth = () => {
    abortRef.current?.abort();
    abortRef.current = null;
    setSigningIn(false);
  };

  const finishPreviewOAuth = (credential: OAuthCredential) => {
    if (!provider) return Promise.resolve();
    return persistOAuthCredential(provider.slug, credential).then(() => {
      setCallbackText("");
      setAuthUrl(null);
      setPendingFlow(null);
      setSaved(true);
      setSignInOpen(false);
    });
  };

  const startPreviewOAuthAuto = (slug: "chatgpt" | "antigravity" | "claude") => {
    setError(null);
    setSaved(false);
    setAuthUrl(null);
    setCopied(false);
    setSigningIn(true);
    const controller = new AbortController();
    abortRef.current?.abort();
    abortRef.current = controller;
    startPreviewOAuth(slug)
      .then((started) => {
        setAuthUrl(started.authUrl);
        try {
          setPendingFlow(readOAuthFlow(slug));
        } catch {
          // ignore
        }
        if (!started.auto) {
          // Loopback port taken — the paste fallback below stays available.
          setSigningIn(false);
          if (abortRef.current === controller) abortRef.current = null;
          return;
        }
        return waitForLoopbackCode(slug, started.state, controller.signal)
          .then((code) => {
            setCompleting(true);
            return completeAutoOAuth(slug, code).then((credential) => finishPreviewOAuth(credential));
          })
          .catch((err: unknown) => {
            // Cancel leaves quietly; timeout keeps the paste fallback visible.
            if (controller.signal.aborted) return;
            setError(err instanceof Error ? err.message : "Could not complete sign-in.");
          })
          .finally(() => {
            setCompleting(false);
            setSigningIn(false);
            if (abortRef.current === controller) abortRef.current = null;
          });
      })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Could not start the browser sign-in — try again.");
        setSigningIn(false);
        if (abortRef.current === controller) abortRef.current = null;
      });
  };

  const cancelPreviewOAuth = () => {
    abortRef.current?.abort();
    abortRef.current = null;
    setSigningIn(false);
    setCompleting(false);
  };

  const completePreviewOAuth = () => {
    if (!provider || !OAUTH_SLUGS.includes(provider.slug)) return;
    setError(null);
    setSaved(false);
    setCompleting(true);
    const finish =
      provider.slug === "chatgpt"
        ? completeCodexSignIn(callbackText)
        : provider.slug === "claude"
          ? completeClaudeSignIn(callbackText)
          : completeAntigravitySignIn(callbackText);
    finish
      .then((credential) => finishPreviewOAuth(credential))
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Could not complete sign-in.");
      })
      .finally(() => setCompleting(false));
  };

  const startCommandCodeBrowser = () => {
    if (!provider || provider.slug !== "commandcode") return;
    setError(null);
    setSaved(false);
    setSigningIn(true);
    const controller = new AbortController();
    abortRef.current?.abort();
    abortRef.current = controller;
    // NO DOUBLE-OPEN: on desktop the Rust `start_commandcode_sign_in` command
    // already opens the system browser (secrets.rs `open_browser`) — the flow
    // below must not call openExternal on the desktop path (see auth.ts).
    startCommandCodeBrowserFlow(controller.signal)
      .then(async (payload) => {
        if (desktop) {
          // Desktop stored + verified the key in the keyring under
          // payload.account; same identity upserts, never duplicates.
          if (payload.account) {
            const info = await localAccountInfo("commandcode", payload.account);
            if (info && typeof (info as LocalAccountInfo).key_suffix === "string") {
              const built = metaFromTauriInfo("commandcode", payload.account, info as LocalAccountInfo);
              if (built) {
                if (!built.email && payload.userName) built.email = payload.userName;
                built.verifiedAt = new Date().toISOString();
                built.authKind = "key";
                upsertAccountMeta(built);
              }
            }
          }
          await reloadLinked();
          setSaved(true);
          setSignInOpen(false);
          return;
        }
        // Preview: verify the captured key (stores it + returns identity).
        const verified = await verifyCommandCodeKey(payload.apiKey);
        void mirrorCredentialToGateway("commandcode", payload.apiKey);
        upsertAccountMeta({
          provider: "commandcode",
          accountId: verified.account,
          email: verified.email || verified.userName || null,
          keySuffix: payload.apiKey.trim().slice(-3),
          verifiedAt: new Date().toISOString(),
          modelCount: null,
          plan: null,
          authKind: "key",
          quota: null,
        });
        await reloadLinked();
        setSaved(true);
        setSignInOpen(false);
      })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Browser sign-in failed.");
      })
      .finally(() => {
        setSigningIn(false);
        if (abortRef.current === controller) abortRef.current = null;
      });
  };

  const saveCommandCodePaste = () => {
    setError(null);
    setSaved(false);
    setVerifying(true);
    const pasted = token;
    verifyCommandCodeKey(pasted)
      .then(async (verified) => {
        void mirrorCredentialToGateway("commandcode", pasted);
        upsertAccountMeta({
          provider: "commandcode",
          accountId: verified.account,
          email: verified.email || verified.userName || null,
          keySuffix: pasted.trim().slice(-3),
          verifiedAt: new Date().toISOString(),
          modelCount: null,
          plan: null,
          authKind: "key",
          quota: null,
        });
        await reloadLinked();
        setSaved(true);
        setToken("");
        setSignInOpen(false);
      })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Key verification failed.");
      })
      .finally(() => setVerifying(false));
  };

  const saveClaudePaste = () => {
    const pasted = token;
    // OAuth setup tokens are OAuth bearers without refresh — store raw like
    // other pasted OAuth tokens (the gateway validates them on quota
    // refresh). API keys verify live against the Models API first.
    if (pasted.trim().startsWith("sk-ant-oat-")) {
      saveOAuthPaste("claude");
      return;
    }
    setError(null);
    setSaved(false);
    setVerifying(true);
    verifyClaudeKey(pasted)
      .then(async (verified) => {
        void mirrorCredentialToGateway("claude", pasted);
        upsertAccountMeta({
          provider: "claude",
          accountId: verified.account,
          email: null,
          keySuffix: pasted.trim().slice(-3),
          verifiedAt: new Date().toISOString(),
          modelCount: verified.models.length,
          // Anthropic exposes no plan name — null stays null (unknown).
          plan: null,
          authKind: "key",
          quota: null,
        });
        await reloadLinked();
        setSaved(true);
        setToken("");
        setSignInOpen(false);
      })
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Key verification failed.");
      })
      .finally(() => setVerifying(false));
  };

  const saveOAuthPaste = (slug: string) => {
    setError(null);
    setSaved(false);
    const pasted = token;
    void (async () => {
      try {
        // Pasted tokens carry no verified email — slot by key hash so the
        // same paste upserts instead of duplicating.
        const accountId = await deriveAccountId(null, pasted);
        await saveCredential(slug, accountId, pasted);
        // Re-read for suffix/email where the backend can provide it.
        const info = await localAccountInfo(slug, accountId);
        if (desktop && info && typeof (info as LocalAccountInfo).key_suffix === "string") {
          const built = metaFromTauriInfo(slug, accountId, info as LocalAccountInfo);
          if (built) {
            built.authKind = "key";
            upsertAccountMeta(built);
          }
        } else if (!desktop) {
          void mirrorCredentialToGateway(slug, pasted);
          upsertAccountMeta({
            provider: slug,
            accountId,
            email: null,
            keySuffix: pasted.trim().slice(-3),
            verifiedAt: null,
            modelCount: null,
            plan: null,
            authKind: "key",
            quota: null,
          });
        }
        await reloadLinked();
        setSaved(true);
        setToken("");
        setSignInOpen(false);
      } catch (err: unknown) {
        setError(err instanceof Error ? err.message : "Could not save the credential.");
      }
    })();
  };

  const linkedCount = linkedAccounts.length;

  /**
   * Refresh live quota for one account via the shared helper (desktop keyring
   * or preview fetch, incl. opencode re-verify). Refresh NEVER deletes
   * accounts — failures only surface as messages; re-signing restores.
   */
  const refreshQuotaFor = (account: LocalAccountMeta) => {
    setError(null);
    setRefreshing(true);
    void refreshQuotaForAccount(account)
      .then(() => reloadLinked().then(() => setError(null)))
      .catch((err: unknown) => {
        setError(err instanceof Error ? err.message : "Quota refresh failed.");
      })
      .finally(() => setRefreshing(false));
  };

  /** Close the sign-in modal, abandoning any in-progress browser wait. */
  const closeSignIn = useCallback(() => {
    abortRef.current?.abort();
    abortRef.current = null;
    setSignInOpen(false);
    setSigningIn(false);
    setCompleting(false);
    setToken("");
    setCallbackText("");
    setError(null);
  }, []);

  useEffect(() => {
    if (!signInOpen) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeSignIn();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [signInOpen, closeSignIn]);

  return (
    <div className="page provider-layout" data-testid={`page-provider-${provider.slug}`}>
      <div className="provider-main">
        {gatewayUp === false ? <OfflineBanner /> : null}
        <div className="provider-head">
          <h1>{provider.label} <span className="provider-count" data-testid="credential-count">{linkedCount}</span></h1>
          <button
            className="signin-btn"
            data-testid={`signin-${provider.slug}`}
            title={provider.signInTitle}
            aria-label={`Sign in to ${provider.label}`}
            aria-expanded={signInOpen}
            onClick={() => { setSignInOpen((v) => !v); setError(null); setSaved(false); }}
          >
            <span aria-hidden="true">↗</span> Sign In
          </button>
        </div>
        {signInOpen ? (
          <div
            className="signin-overlay"
            data-testid={`signin-overlay-${provider.slug}`}
            onClick={closeSignIn}
          >
          <section
            className="signin-modal"
            role="dialog"
            aria-modal="true"
            aria-label={`Sign in to ${provider.label}`}
            data-testid={`signin-panel-${provider.slug}`}
            onClick={(event) => event.stopPropagation()}
          >
            <div className="signin-modal-head">
              <button
                className="signin-close"
                data-testid={`signin-close-${provider.slug}`}
                aria-label="Close sign-in"
                title="Close"
                onClick={closeSignIn}
              >
                ×
              </button>
              <h2>{provider.signInTitle}</h2>
            </div>
            {error ? <p className="error" data-testid={`signin-modal-error-${provider.slug}`}>{error}</p> : null}
            {provider.slug === "commandcode" ? (
              <div className="signin-actions">
                <button
                  data-testid="signin-browser-commandcode"
                  disabled={signingIn}
                  title="Open the official Command Code login in your browser"
                  onClick={startCommandCodeBrowser}
                >
                  {signingIn ? "Waiting for browser…" : "Sign in with Command Code in browser"}
                </button>
                {signingIn ? (
                  <button
                    className="ghost-btn"
                    onClick={() => {
                      abortRef.current?.abort();
                      setSigningIn(false);
                      setError("Sign-in was cancelled.");
                    }}
                  >
                    Cancel
                  </button>
                ) : null}
              </div>
            ) : null}
            {signingIn && provider.slug === "commandcode" ? (
              <p className="muted" data-testid="signin-waiting-commandcode">
                {desktop
                  ? "Approve the sign-in in the browser window — Proxy Dock captures the issued key automatically. If Studio shows “Copy your API key” instead, paste it below."
                  : "Approve the sign-in in the browser tab — Proxy Dock captures the issued key automatically. If Studio shows “Copy your API key” instead, paste it below."}
              </p>
            ) : null}
            {provider.slug !== "commandcode" && provider.slug !== "opencode" ? (
              <>
              <div className="signin-actions">
                <button
                  data-testid={`signin-browser-${provider.slug}`}
                  disabled={signingIn}
                  title={
                    desktop
                      ? `Open ${provider.label} sign-in in your browser`
                      : `Open ${provider.label} sign-in in a new browser tab`
                  }
                  onClick={() => {
                    if (desktop) handleDesktopOAuth(provider.slug);
                    else startPreviewOAuthAuto(provider.slug as "chatgpt" | "antigravity" | "claude");
                  }}
                >
                  {signingIn
                    ? "Waiting for browser…"
                    : provider.slug === "chatgpt"
                      ? "Sign in with ChatGPT in browser"
                      : provider.slug === "claude"
                        ? "Sign in with Claude in browser"
                        : "Sign in with Google in browser"}
                </button>
                {signingIn ? (
                  <button
                    className="ghost-btn"
                    data-testid={`signin-cancel-${provider.slug}`}
                    onClick={desktop ? cancelDesktopOAuth : cancelPreviewOAuth}
                  >
                    Cancel
                  </button>
                ) : null}
              </div>
              {signingIn ? (
                <p className="muted" data-testid={`signin-waiting-${provider.slug}`}>
                  {desktop
                    ? "Approve the sign-in in the browser window — Proxy Dock captures the callback automatically."
                    : completing
                      ? "Browser approved — linking your account…"
                      : "Approve the sign-in in the browser tab — Proxy Dock captures the redirect automatically, nothing to copy."}
                </p>
              ) : null}
              </>
            ) : null}
            {!desktop && pendingFlow && provider.slug !== "opencode" && provider.slug !== "commandcode" ? (
              <div>
                <p className="muted" data-testid={`signin-callback-hint-${provider.slug}`}>
                  {signingIn
                    ? "If the automatic capture does not complete, copy the address-bar URL you land on and paste it below — no need to wait."
                    : "If the browser did not finish automatically, copy the address-bar URL you land on and paste it below to finish signing in."}
                </p>
                {authUrl ? (
                  <div>
                    <p
                      className="muted"
                      data-testid={`signin-authurl-${provider.slug}`}
                      style={{ overflowWrap: "anywhere", wordBreak: "break-all" }}
                    >
                      Sign-in page: <a href={authUrl} target="_blank" rel="noreferrer">{authUrl}</a>
                    </p>
                    <div className="signin-actions">
                      <button
                        data-testid={`signin-copy-url-${provider.slug}`}
                        onClick={() => {
                          try {
                            const done = navigator.clipboard?.writeText(authUrl);
                            if (!done) return;
                            void done.then(
                              () => {
                                setCopied(true);
                                setTimeout(() => setCopied(false), 2000);
                              },
                              () => setCopied(false),
                            );
                          } catch {
                            setCopied(false);
                          }
                        }}
                      >
                        {copied ? "Copied" : "Copy"}
                      </button>
                      <button
                        data-testid={`signin-open-url-${provider.slug}`}
                        onClick={() => {
                          void openExternal(authUrl);
                        }}
                      >
                        Open in browser
                      </button>
                      <button className="ghost-btn" onClick={cancelPreviewOAuth}>
                        Cancel
                      </button>
                    </div>
                  </div>
                ) : null}
                <label className="token-label">
                  Paste callback URL
                  <input
                    type="text"
                    autoComplete="off"
                    spellCheck={false}
                    placeholder="Paste the full callback URL"
                    value={callbackText}
                    onChange={(event) => setCallbackText(event.target.value)}
                    data-testid={`signin-callback-${provider.slug}`}
                  />
                </label>
                <div className="signin-actions">
                  <button data-testid={`signin-complete-${provider.slug}`} disabled={completing || callbackText.trim().length === 0} onClick={completePreviewOAuth}>
                    {completing ? "Completing…" : "Complete sign-in"}
                  </button>
                </div>
              </div>
            ) : null}
            <label className="token-label">
              {provider.slug === "opencode"
                ? "Paste Go API key"
                : provider.slug === "commandcode" || provider.slug === "claude"
                  ? "Paste API key"
                  : "Paste credential"}
              <input
                type="password"
                autoComplete="off"
                spellCheck={false}
                placeholder={provider.signInMethod === "api-key" ? "Paste API key" : "Paste access token"}
                value={token}
                onChange={(event) => setToken(event.target.value)}
                data-testid={`signin-token-${provider.slug}`}
              />
            </label>
            <div className="signin-actions">
              <button
                data-testid={`signin-save-${provider.slug}`}
                disabled={verifying || token.trim().length === 0}
                onClick={() => {
                  setError(null);
                  setSaved(false);
                  setVerifiedModels(null);
                  if (provider.slug === "opencode") {
                    const pasted = token;
                    setVerifying(true);
                      verifyOpencodeKey(pasted)
                      .then(async (result) => {
                        setVerifiedModels(result.models);
                        setSaved(true);
                        setToken("");
                        setSignInOpen(false);
                        // No plan guessing: null stays null (unknown), never "Go".
                        void mirrorCredentialToGateway("opencode", pasted);
                        upsertAccountMeta({
                          provider: "opencode",
                          accountId: result.account,
                          email: null,
                          keySuffix: pasted.trim().slice(-3),
                          verifiedAt: new Date().toISOString(),
                          modelCount: result.models.length,
                          plan: result.plan ?? null,
                          authKind: "key",
                          quota: result.quota ?? null,
                        });
                        await reloadLinked();
                      })
                      .catch((err: unknown) => {
                        setError(err instanceof Error ? err.message : "Key verification failed.");
                      })
                      .finally(() => setVerifying(false));
                    return;
                  }
                  if (provider.slug === "commandcode") {
                    saveCommandCodePaste();
                    return;
                  }
                  if (provider.slug === "claude") {
                    saveClaudePaste();
                    return;
                  }
                  saveOAuthPaste(provider.slug);
                }}
              >
                {provider.slug === "opencode" || provider.slug === "claude"
                  ? verifying ? "Verifying…" : "Verify and save key"
                  : provider.slug === "commandcode"
                    ? verifying ? "Verifying…" : "Verify and save key"
                    : "Save credential"}
              </button>
              <button className="ghost-btn" onClick={closeSignIn}>
                Cancel
              </button>
            </div>
          </section>
          </div>
        ) : null}
        {isOpencode ? (
          <>
            {linkedAccounts.length === 0 ? (
              <section className="card" aria-label="No accounts yet" data-testid={`no-accounts-${provider.slug}`}>
                <h2>No accounts yet</h2>
                <p className="muted">Sign in above to add your first {provider.label} credential. The key is tested against opencode.ai before it is saved.</p>
              </section>
            ) : (
              <ProviderQuotaList
                accounts={linkedAccounts}
                iconSrc={provider.icon}
                providerLabel={provider.label}
                refreshing={refreshing}
                onRefresh={(account) => refreshQuotaFor(account)}
                onRemove={(account) => {
                  void deleteRoutingAccount(account).then(() => setVerifiedModels(null));
                }}
                backendLink={backendLink}
                onRetryBackend={retryBackendLink}
                routing={routing}
                onPatchRouting={(account, patch) => void patchRouting(account, patch)}
                onReorder={(ids) => void reorderRouting(ids)}
              />
            )}
          </>
        ) : browserCapable ? (
          <>
            {linkedAccounts.length === 0 ? (
              <section className="card" aria-label="No accounts yet" data-testid={`no-accounts-${provider.slug}`}>
                <h2>No accounts yet</h2>
                <p className="muted">Sign in above to add your first {provider.label} credential. No credential files to hunt — the browser flow captures it for you.</p>
              </section>
            ) : (
              <ProviderQuotaList
                accounts={linkedAccounts}
                iconSrc={provider.icon}
                providerLabel={provider.label}
                refreshing={refreshing}
                onRefresh={(account) => refreshQuotaFor(account)}
                onRemove={(account) => void deleteRoutingAccount(account)}
                backendLink={backendLink}
                onRetryBackend={retryBackendLink}
                routing={routing}
                onPatchRouting={(account, patch) => void patchRouting(account, patch)}
                onReorder={(ids) => void reorderRouting(ids)}
              />
            )}
          </>
        ) : null}
        <ModelCatalogSection provider={provider.slug} label={provider.label} hasAccounts={linkedAccounts.length > 0} />
        {error ? <p className="error" data-testid={`signin-error-${provider.slug}`}>{error}</p> : null}
      </div>
    </div>
  );
}
