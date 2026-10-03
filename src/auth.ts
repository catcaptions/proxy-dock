import { GatewayKeyError, GatewayUnreachableError, gatewayAuthHeader, isGatewayUnreachable, resolveGatewayBase } from "./api";

type TokenStatus = "no-token" | "token-present";

declare global {
  interface Window {
    __TAURI__?: unknown;
    __TAURI_INTERNALS__?: unknown;
  }
}

function inTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    (window.__TAURI__ !== undefined || window.__TAURI_INTERNALS__ !== undefined)
  );
}

export function isTauri(): boolean {
  return inTauri();
}

/**
 * Opens a URL in the system browser: opener plugin on desktop, new tab in
 * browser preview. The preview path calls window.open synchronously (no
 * await before it), exactly like the direct call it replaces.
 */
export async function openExternal(url: string): Promise<void> {
  if (isTauri()) {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
    return;
  }
  window.open(url, "_blank", "noopener");
}

async function tauriInvoke<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export async function saveCredential(providerSlug: string, accountId: string, token: string): Promise<void> {
  const secret = token.trim();
  if (secret.length < 8) {
    throw new Error("That looks too short — paste the full credential.");
  }
  if (inTauri()) {
    await tauriInvoke("set_local_token", { providerSlug, accountId, token: secret });
    return;
  }
  // Preview persistence: localStorage survives tabs/restarts (sessionStorage
  // alone dies with the tab). Both are cleared on sign-out/remove.
  setToken(providerSlug, accountId, secret);
}

export async function tokenStatus(providerSlug: string, accountId: string): Promise<TokenStatus> {
  if (inTauri()) {
    const status = await tauriInvoke<string>("local_token_status", { providerSlug, accountId });
    return status === "token-present" ? "token-present" : "no-token";
  }
  const stored = getToken(providerSlug, accountId);
  return stored && stored.trim().length >= 8 ? "token-present" : "no-token";
}

export async function clearCredential(providerSlug: string, accountId: string): Promise<void> {
  if (inTauri()) {
    await tauriInvoke("clear_local_token", { providerSlug, accountId });
    return;
  }
  clearToken(providerSlug, accountId);
}

export type BrowserSignInResult = {
  provider: string;
  /** Stable account slot id (email or key-hash) — never display as identity. */
  account: string;
  /** Display identity (email, or username where the provider gives no email). */
  email: string;
};

export async function startBrowserSignIn(providerSlug: string): Promise<BrowserSignInResult> {
  if (!inTauri()) {
    throw new Error("Browser sign-in needs the desktop app — paste your credential instead, or open Proxy Dock from Tauri.");
  }
  // NO DOUBLE-OPEN: each Rust `start_*_sign_in` command already opens the
  // system browser via secrets.rs `open_browser` — callers must not also call
  // openExternal on the desktop path.
  switch (providerSlug) {
    case "chatgpt":
      return tauriInvoke<BrowserSignInResult>("start_codex_sign_in", {});
    case "antigravity":
      return tauriInvoke<BrowserSignInResult>("start_antigravity_sign_in", {});
    case "claude":
      return tauriInvoke<BrowserSignInResult>("start_claude_sign_in", {});
    case "commandcode":
      return tauriInvoke<BrowserSignInResult>("start_commandcode_sign_in", {});
    default:
      throw new Error(`Browser sign-in is not available for ${providerSlug}.`);
  }
}

export type OpencodeVerified = {
  provider: string;
  account: string;
  models: string[];
  /** Provider-reported subscription plan ("Go", or "Free" for a valid key without a Go subscription). */
  plan: string;
  /** Live quota rows parsed from the usage response (empty for Free keys). */
  quota: QuotaRow[];
};

export const OPENCODE_USAGE_URL = "https://opencode.ai/zen/go/v1/usage";
export const OPENCODE_MODELS_URL = "https://opencode.ai/zen/go/v1/models";

/** Last-3 display for a key, e.g. `•••••••xyz`. Full key is never rendered. */
export function maskKey(secret: string): string {
  const trimmed = secret.trim();
  if (trimmed.length === 0) return "•••";
  return `•••••••${trimmed.slice(-3)}`;
}

/**
 * Censored account identity for card headers, e.g.
 * `someone@example.com` → `som•••@e•••.com`. The full value is never
 * rendered — pair with a `title` tooltip holding the real identity.
 */
export function censorEmail(email: string): string {
  const at = email.indexOf("@");
  if (at < 0) return maskKey(email);
  const local = email.slice(0, at);
  const domain = email.slice(at + 1);
  const shortLocal = local.length <= 4 ? `${local.charAt(0) ?? ""}•••` : `${local.slice(0, 3)}•••`;
  const dot = domain.lastIndexOf(".");
  const tld = dot >= 0 ? domain.slice(dot + 1) : "";
  const first = domain.charAt(0) ?? "";
  const maskedDomain = tld ? `${first}•••.${tld}` : `${first}•••`;
  return `${shortLocal}@${maskedDomain}`;
}

/** Plan label for display: stored plan only, never guessed. A plan-less
 * account renders no pill (unknown means unknown — the old opencode "Go"
 * fallback fabricated a fact and is deleted). */
export function displayPlanFor(account: LocalAccountMeta): string | null {
  if (account.plan) return account.plan;
  return null;
}

/** Censored one-line identity for display (`las•••@g•••.com`, `•••••••xyz`). */
export function displayIdentity(account: LocalAccountMeta): string {
  if (account.email && account.email.length > 0) return censorEmail(account.email);
  return maskKey(account.keySuffix);
}

/** Full identity for hover tooltips only — never rendered as text. */
export function fullIdentity(account: LocalAccountMeta): string {
  if (account.email && account.email.length > 0) return account.email;
  return `Key ending ${account.keySuffix}`;
}

export type LocalAccountMeta = {
  provider: string;
  accountId: string;
  /** Email when known (OAuth flows). OpenCode Go keys don't expose one. */
  email: string | null;
  keySuffix: string;
  verifiedAt: string | null;
  modelCount: number | null;
  /** Plan label, set only when factually known at save time (e.g. "Go"). */
  plan?: string | null;
  /** How the credential was obtained — known at save time. */
  authKind?: "oauth" | "key" | null;
  /**
   * Quota/limit rows, rendered only when a provider API actually returned
   * them. Never fabricated: no rows means no quota data (yet).
   */
  quota?: QuotaRow[] | null;
};

/** One usage-limit row, mirroring the account-block design. */
export type QuotaRow = {
  id: string;
  label: string;
  /** Remaining percent 0–100, or null when unknown. */
  remainingPct: number | null;
  /** Reset timestamp display, e.g. "09/25, 19:09". */
  resetText: string;
  /** Relative reset display, e.g. "in 12 minutes". */
  resetInText: string;
  /** Accent the relative time (e.g. resetting within the hour). */
  urgent?: boolean | null;
};

const metaKey = (providerSlug: string, accountId: string) => `proxydock-account:${providerSlug}:${accountId}`;

export function readAccountMeta(providerSlug: string, accountId: string): LocalAccountMeta | null {
  try {
    const raw = localStorage.getItem(metaKey(providerSlug, accountId));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as LocalAccountMeta;
    if (typeof parsed.keySuffix !== "string") return null;
    // Normalize metas saved before the optional fields existed.
    const quota = Array.isArray(parsed.quota)
      ? parsed.quota.filter(
          (row): row is QuotaRow =>
            !!row &&
            typeof row.id === "string" &&
            typeof row.label === "string" &&
            typeof row.resetText === "string" &&
            typeof row.resetInText === "string" &&
            (row.remainingPct === null || row.remainingPct === undefined || typeof row.remainingPct === "number"),
        )
      : null;
    return {
      provider: typeof parsed.provider === "string" ? parsed.provider : providerSlug,
      accountId: typeof parsed.accountId === "string" ? parsed.accountId : accountId,
      email: typeof parsed.email === "string" ? parsed.email : null,
      keySuffix: parsed.keySuffix,
      verifiedAt: typeof parsed.verifiedAt === "string" ? parsed.verifiedAt : null,
      modelCount: typeof parsed.modelCount === "number" ? parsed.modelCount : null,
      plan: typeof parsed.plan === "string" ? parsed.plan : null,
      authKind: parsed.authKind === "oauth" || parsed.authKind === "key" ? parsed.authKind : null,
      quota,
    };
  } catch {
    return null;
  }
}

export function writeAccountMeta(meta: LocalAccountMeta): void {
  try {
    localStorage.setItem(metaKey(meta.provider, meta.accountId), JSON.stringify(meta));
  } catch {
    // storage full / blocked — non-fatal
  }
}

export function removeAccountMeta(providerSlug: string, accountId: string): void {
  try {
    localStorage.removeItem(metaKey(providerSlug, accountId));
  } catch {
    // ignore
  }
}

/**
 * Stable account slot id for a credential. Mirrors the Rust `account_id_for`
 * exactly — email (trimmed, lowercased) when verified, else
 * `key-<first 12 hex of sha256(secret)>`. Same identity always maps to the
 * same slot, so re-signing updates in place and can never duplicate or
 * clobber a different account.
 */
export async function deriveAccountId(email: string | null, secret: string): Promise<string> {
  const normalized = (email ?? "").trim().toLowerCase();
  if (normalized) return normalized;
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(secret.trim()));
  const hex = [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
  return `key-${hex.slice(0, 12)}`;
}

const rosterKey = (providerSlug: string) => `proxydock-accounts:${providerSlug}`;

/**
 * Ordered account ids for a provider, or null when no roster was ever
 * written (pre-multi-account era — callers fall back to ["default"]).
 */
export function readRoster(providerSlug: string): string[] | null {
  try {
    const raw = localStorage.getItem(rosterKey(providerSlug));
    if (raw === null) return null;
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((id): id is string => typeof id === "string" && id.length > 0);
  } catch {
    return [];
  }
}

function writeRoster(providerSlug: string, ids: string[]): void {
  try {
    localStorage.setItem(rosterKey(providerSlug), JSON.stringify(ids));
  } catch {
    // ignore storage errors
  }
}

function tokenStoreKey(providerSlug: string, accountId: string): string {
  return `proxydock-token:${providerSlug}:${accountId}`;
}

/**
 * Single preview-token store (plan §3.5): one session-first priority,
 * try/catch everywhere. Session holds the freshest per-tab value (e.g. a
 * rotated OAuth token when localStorage is blocked); local persists across
 * tabs/restarts. All preview token reads/writes go through here — never
 * touch sessionStorage/localStorage directly for tokens.
 */
export function getToken(providerSlug: string, accountId: string): string | null {
  const key = tokenStoreKey(providerSlug, accountId);
  try {
    const fromSession = sessionStorage.getItem(key);
    if (fromSession !== null) return fromSession;
  } catch {
    // storage blocked — fall through to local
  }
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function setToken(providerSlug: string, accountId: string, secret: string): void {
  const key = tokenStoreKey(providerSlug, accountId);
  try {
    sessionStorage.setItem(key, secret);
  } catch {
    // ignore storage errors (private mode etc.)
  }
  try {
    localStorage.setItem(key, secret);
  } catch {
    // ignore storage errors (private mode etc.)
  }
}

export function clearToken(providerSlug: string, accountId: string): void {
  const key = tokenStoreKey(providerSlug, accountId);
  try {
    sessionStorage.removeItem(key);
  } catch {
    // ignore
  }
  try {
    localStorage.removeItem(key);
  } catch {
    // ignore
  }
}

/**
 * One-time app rename `proxy-hub` -> `proxy-dock`. Copies every `proxyhub-*`
 * browser-storage key (tokens, metas, rosters, OAuth flows) to its
 * `proxydock-*` twin when the twin is absent, then removes the old key.
 * Idempotent; safe to run on every startup. Run BEFORE
 * migrateCommandCodeSlug so the slug migration sees the new keys.
 */
export function migrateProxyHubKeys(): void {
  for (const store of [sessionStorage, localStorage]) {
    try {
      const olds: string[] = [];
      for (let i = 0; i < store.length; i++) {
        const k = store.key(i);
        if (k && k.startsWith("proxyhub-")) olds.push(k);
      }
      for (const old of olds) {
        const next = `proxydock-${old.slice("proxyhub-".length)}`;
        try {
          if (store.getItem(next) === null) {
            const v = store.getItem(old);
            if (v !== null) store.setItem(next, v);
          }
          store.removeItem(old);
        } catch {
          // ignore storage errors
        }
      }
    } catch {
      // Migration must never break startup.
    }
  }
}

/**
 * One-time slug rename `commandcode-go` -> `commandcode` (Go is a plan, not
 * a provider). Moves the roster, account metas (fixing the stored provider
 * field), and preview tokens to the new keys, merging when both eras exist.
 * Idempotent; safe to run on every startup.
 */
export function migrateCommandCodeSlug(): void {
  const OLD = "commandcode-go";
  const NEXT = "commandcode";
  try {
    const oldRoster = readRoster(OLD);
    const ids = new Set<string>([...(readRoster(NEXT) ?? []), ...(oldRoster ?? [])]);
    if (oldRoster === null) {
      // Pre-roster era: probe the legacy "default" slot.
      const legacy = readAccountMeta(OLD, "default");
      const hasToken = getToken(OLD, "default");
      if (legacy || hasToken) ids.add("default");
    }
    if (ids.size === 0 && oldRoster === null) return;
    for (const id of ids) {
      const meta = readAccountMeta(OLD, id);
      if (meta && !readAccountMeta(NEXT, id)) {
        writeAccountMeta({ ...meta, provider: NEXT });
      } else if (meta) {
        const current = readAccountMeta(NEXT, id);
        if (current && current.provider !== NEXT) writeAccountMeta({ ...current, provider: NEXT });
      }
      const secret = getToken(OLD, id);
      if (secret && !getToken(NEXT, id)) {
        setToken(NEXT, id, secret);
      }
      clearToken(OLD, id);
      removeAccountMeta(OLD, id);
    }
    writeRoster(NEXT, [...ids]);
    try {
      localStorage.removeItem(rosterKey(OLD));
    } catch {
      // ignore storage errors
    }
  } catch {
    // Migration must never break startup.
  }
}

function sameIdentity(a: LocalAccountMeta, b: LocalAccountMeta): boolean {
  if (a.accountId === b.accountId) return true;
  const emailA = (a.email ?? "").trim().toLowerCase();
  const emailB = (b.email ?? "").trim().toLowerCase();
  if (emailA && emailA === emailB) return true;
  // Same stored key material (e.g. a legacy "default" slot re-verified under
  // its key-hash id). Suffixes are short, so this is a heuristic — but the
  // alternative (showing the same key twice) is strictly worse, and a merge
  // is self-healing via Remove + re-add.
  if (a.keySuffix && a.keySuffix === b.keySuffix) return true;
  return false;
}

/**
 * Insert or update an account: same identity replaces in place (no
 * duplicates ever), new identities append. Migrates the legacy single
 * "default" slot: a superseded legacy entry is dropped, a distinct one is
 * kept alongside.
 */
export function upsertAccountMeta(meta: LocalAccountMeta): void {
  const slug = meta.provider;
  const roster = readRoster(slug);
  const ids = roster ?? ["default"];
  const survivors: string[] = [];
  for (const id of ids) {
    if (id === meta.accountId) continue;
    if (id === "default" && roster === null) {
      const legacy = readAccountMeta(slug, "default");
      if (legacy && sameIdentity(legacy, meta)) {
        // Superseded: drop the legacy slot's local traces (keyring entries
        // are removed best-effort on desktop).
        removeAccountMeta(slug, "default");
        clearToken(slug, "default");
        void clearCredential(slug, "default").catch(() => undefined);
        continue;
      }
    }
    if (!survivors.includes(id)) survivors.push(id);
  }
  survivors.push(meta.accountId);
  writeAccountMeta(meta);
  writeRoster(slug, survivors);
}

/** Remove every local trace of one account (credential, meta, roster entry). */
export async function forgetAccount(providerSlug: string, accountId: string): Promise<void> {
  try {
    await clearCredential(providerSlug, accountId);
  } finally {
    clearToken(providerSlug, accountId);
    removeAccountMeta(providerSlug, accountId);
    // Keep pre-roster semantics: a null roster (never written) stays null —
    // callers fall back to ["default"], which now resolves to nothing since
    // the meta + token are gone. Writing [] would invent a roster era.
    const roster = readRoster(providerSlug);
    if (roster !== null) {
      writeRoster(
        providerSlug,
        roster.filter((id) => id !== accountId),
      );
    }
  }
}

export type LocalAccountInfo = {
  provider: string;
  account: string;
  email: string | null;
  key_suffix: string;
  status: TokenStatus;
};

export async function localAccountInfo(providerSlug: string, accountId: string): Promise<LocalAccountInfo | LocalAccountMeta | null> {  if (inTauri()) {
    try {
      const info = await tauriInvoke<LocalAccountInfo>("local_account_info", { providerSlug, accountId });
      return info;
    } catch {
      return null;
    }
  }
  const stored = getToken(providerSlug, accountId);
  if (!stored || stored.trim().length < 8) return readAccountMeta(providerSlug, accountId);
  const meta = readAccountMeta(providerSlug, accountId);
  return (
    meta ?? {
      provider: providerSlug,
      accountId,
      email: null,
      keySuffix: stored.trim().slice(-3),
      verifiedAt: null,
      modelCount: null,
    }
  );
}

const LINKED_SLUGS = ["commandcode", "opencode", "chatgpt", "antigravity", "claude"];

/**
 * Resolve one account slot: stored meta first, then live credential presence
 * (OS keyring on desktop, session storage in browser preview). Used for every
 * roster id plus the legacy "default" slot.
 */
export async function resolveAccountMeta(providerSlug: string, accountId: string): Promise<LocalAccountMeta | null> {
  const meta = readAccountMeta(providerSlug, accountId);
  if (meta) return meta;
  let status: TokenStatus;
  try {
    status = await tokenStatus(providerSlug, accountId);
  } catch {
    return null;
  }
  if (status !== "token-present") return null;
  const info = await localAccountInfo(providerSlug, accountId);
  if (info && typeof (info as LocalAccountInfo).key_suffix === "string") {
    const tauri = info as LocalAccountInfo;
    if (!tauri.key_suffix) return null;
    return {
      provider: providerSlug,
      accountId,
      email: tauri.email ?? null,
      keySuffix: tauri.key_suffix,
      verifiedAt: null,
      modelCount: null,
    };
  }
  if (info && typeof (info as LocalAccountMeta).keySuffix === "string") {
    return info as LocalAccountMeta;
  }
  return null;
}

/** All currently linked accounts across providers (used by Home). */
export async function listLinkedAccounts(): Promise<LocalAccountMeta[]> {
  const out: LocalAccountMeta[] = [];
  for (const slug of LINKED_SLUGS) {
    const roster = readRoster(slug);
    const ids = roster ?? ["default"];
    for (const id of ids) {
      try {
        const account = await resolveAccountMeta(slug, id);
        if (account && !out.some((a) => a.provider === slug && a.accountId === account.accountId)) out.push(account);
      } catch {
        // Ignore per-account failures — one bad store shouldn't hide the rest.
      }
    }
  }
  return out;
}

export async function verifyOpencodeKey(apiKey: string): Promise<OpencodeVerified> {
  const secret = apiKey.trim();
  if (secret.length < 8) {
    throw new Error("That key looks too short — paste the full Go API key.");
  }
  if (inTauri()) {
    return tauriInvoke<OpencodeVerified>("verify_opencode_key", { apiKey: secret });
  }
  // Browser preview: actually test the key against the auth-gated usage
  // endpoint. NOTE: /models is public (200 even for bad keys) so it can't
  // verify — /usage returns 401 for bad keys and 200 for good ones.
  // opencode.ai sends no CORS headers, so direct page fetches are blocked;
  // in `npm run dev` use the same-origin vite proxy, falling back to direct.
  const usageUrls = ["/opencode-usage", OPENCODE_USAGE_URL];
  let usageResp: Response | null = null;
  let lastNetworkError = false;
  for (const url of usageUrls) {
    try {
      usageResp = await fetch(url, {
        headers: { Authorization: `Bearer ${secret}`, Accept: "application/json" },
      });
      lastNetworkError = false;
      break;
    } catch {
      lastNetworkError = true;
      usageResp = null;
    }
  }
  if (!usageResp) {
    throw new Error(
      lastNetworkError
        ? "Could not reach opencode.ai to verify the key — check your connection, then try again. (Browser preview uses a local proxy; the desktop app verifies directly.)"
        : "Could not verify the key — try again.",
    );
  }
  if (usageResp.status === 401) {
    throw new Error("opencode.ai rejected the key (unauthorized) — check it and try again.");
  }
  // 403 EntitlementError = valid key, no Go subscription (verified against
  // the open-source console route). The key is still accepted as a Free
  // account so the list shows its real subscription plan.
  const subscribed = usageResp.status !== 403;
  if (subscribed && !usageResp.ok) {
    throw new Error(`opencode.ai verification failed (${usageResp.status}) — try again.`);
  }
  const usageBody = subscribed ? await usageResp.json().catch(() => null) : null;
  // Key is valid. Fetch the public model list for display only.
  let models: string[] = [];
  for (const url of ["/opencode-models", OPENCODE_MODELS_URL]) {
    try {
      const modelsResp = await fetch(url, { headers: { Accept: "application/json" } });
      if (modelsResp.ok) {
        const body = (await modelsResp.json()) as { data?: { id?: string }[] };
        const items = Array.isArray(body.data) ? body.data : [];
        models = items.map((m) => m.id ?? "").filter((id) => id.length > 0);
        break;
      }
    } catch {
      // try next URL
    }
  }
  try {
    const account = await deriveAccountId(null, secret);
    setToken("opencode", account, secret);
    return { provider: "opencode", account, models, plan: subscribed ? "Go" : "Free", quota: mapOpencodeUsage(usageBody, quotaNow()) };
  } catch {
    // ignore storage errors
    return { provider: "opencode", account: await deriveAccountId(null, secret), models, plan: subscribed ? "Go" : "Free", quota: mapOpencodeUsage(usageBody, quotaNow()) };
  }
}

/**
 * Maps the OpenCode `/zen/go/v1/usage` shape to quota rows. Verified against
 * the open-source console route: `{usage: {rolling|weekly|monthly:
 * {status: "ok"|"rate-limited", percent, resetsAt}}}`. `percent` is usage
 * consumed, so remaining = 100 - percent. Unknown shapes yield no rows —
 * never fabricated bars.
 */
export function mapOpencodeUsage(body: unknown, now: number): QuotaRow[] {
  const rows: QuotaRow[] = [];
  if (!body || typeof body !== "object") return rows;
  const usage = (body as Record<string, unknown>).usage;
  if (!usage || typeof usage !== "object") return rows;
  const rec = usage as Record<string, unknown>;
  const windows: [string, string][] = [
    ["rolling", "5-hour limit"],
    ["weekly", "Weekly limit"],
    ["monthly", "Monthly limit"],
  ];
  for (const [key, label] of windows) {
    const window = rec[key];
    if (!window || typeof window !== "object") continue;
    const w = window as Record<string, unknown>;
    if (typeof w.percent !== "number" || !Number.isFinite(w.percent)) continue;
    const reset = parseResetValue(w.resetTime ?? w.resetsAt);
    const row = makeQuotaRow(`opencode-${key}`, label, 100 - (w.percent as number), reset, now);
    if (w.status === "rate-limited") row.urgent = true;
    rows.push(row);
  }
  return rows;
}

// ---------------------------------------------------------------------------
// Redirect-based sign-in (mirrors the Tauri/Rust flows in src-tauri/src).
// In the desktop app the Rust backend opens the system browser and captures
// the localhost callback automatically. In the browser preview the same
// authorize URLs are opened in a new tab and the callback is completed
// manually (paste the callback URL) or, for Command Code, captured by the
// dev middleware — so you never have to hunt credential files yourself.
// ---------------------------------------------------------------------------

export const CODEX_AUTH_URL = "https://auth.openai.com/oauth/authorize";
export const CODEX_TOKEN_PATH = "/__proxydock/codex-exchange";
export const CODEX_CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann";
export const CODEX_CALLBACK_PORT = 1455;
export const CODEX_CALLBACK_PATH = "/auth/callback";
export const CODEX_CALLBACK_URL = `http://localhost:${CODEX_CALLBACK_PORT}${CODEX_CALLBACK_PATH}`;

export const ANTIGRAVITY_AUTH_URL = "https://accounts.google.com/o/oauth2/v2/auth";
export const ANTIGRAVITY_TOKEN_PATH = "/__proxydock/antigravity-exchange";
export const ANTIGRAVITY_CLIENT_ID =
  "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
export const ANTIGRAVITY_CALLBACK_PORT = 51121;
export const ANTIGRAVITY_CALLBACK_PATH = "/oauth-callback";
export const ANTIGRAVITY_CALLBACK_URL = `http://localhost:${ANTIGRAVITY_CALLBACK_PORT}${ANTIGRAVITY_CALLBACK_PATH}`;
export const ANTIGRAVITY_SCOPES =
  "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile https://www.googleapis.com/auth/cclog https://www.googleapis.com/auth/experimentsandconfigs";

export const COMMANDCODE_AUTH_PAGE = "https://commandcode.ai/studio/auth/cli";
/** Exact callback path the official CLI serves (`http://127.0.0.1:{port}/callback`). */
export const COMMANDCODE_CALLBACK_PATH = "/callback";
export const COMMANDCODE_WHOAMI_PATH = "/commandcode-alpha/whoami";
export const COMMANDCODE_WHOAMI_URL = "https://api.commandcode.ai/alpha/whoami";

// Claude (Anthropic) subscription OAuth — mirrors CLIProxyAPI's real
// endpoints (nothing invented): authorize at claude.ai with PKCE, the public
// client id below, and the localhost :54545 callback (same as
// `cli-proxy-api --claude-login`); code exchange at platform.claude.com.
export const CLAUDE_AUTH_URL = "https://claude.ai/oauth/authorize";
export const CLAUDE_TOKEN_PATH = "/__proxydock/claude-exchange";
export const CLAUDE_REFRESH_PATH = "/__proxydock/claude-refresh";
export const CLAUDE_USAGE_PATH = "/__proxydock/claude-usage";
export const CLAUDE_CLIENT_ID = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
export const CLAUDE_CALLBACK_PORT = 54545;
export const CLAUDE_CALLBACK_PATH = "/callback";
export const CLAUDE_CALLBACK_URL = `http://localhost:${CLAUDE_CALLBACK_PORT}${CLAUDE_CALLBACK_PATH}`;
export const CLAUDE_SCOPE =
  "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
export const CLAUDE_MODELS_URL = "https://api.anthropic.com/v1/models";

export function buildCodexAuthUrl(state: string, challenge: string, redirectUri: string): string {
  const params = new URLSearchParams({
    client_id: CODEX_CLIENT_ID,
    response_type: "code",
    redirect_uri: redirectUri,
    scope: "openid email profile offline_access",
    state,
    code_challenge: challenge,
    code_challenge_method: "S256",
    prompt: "login",
    id_token_add_organizations: "true",
    codex_cli_simplified_flow: "true",
  });
  return `${CODEX_AUTH_URL}?${params.toString()}`;
}

export function buildAntigravityAuthUrl(state: string, redirectUri: string, clientId: string = ANTIGRAVITY_CLIENT_ID, challenge?: string): string {
  const params = new URLSearchParams({
    access_type: "offline",
    client_id: clientId,
    prompt: "consent",
    redirect_uri: redirectUri,
    response_type: "code",
    scope: ANTIGRAVITY_SCOPES,
    state,
  });
  // PKCE: public clients exchange without any secret (one-click sign-in).
  if (challenge) {
    params.set("code_challenge", challenge);
    params.set("code_challenge_method", "S256");
  }
  return `${ANTIGRAVITY_AUTH_URL}?${params.toString()}`;
}

export function buildCommandCodeAuthUrl(callback: string, state: string): string {
  // Mirrors the official CLI exactly: callback + state + mode=redirect.
  const params = new URLSearchParams({ callback, state, mode: "redirect" });
  return `${COMMANDCODE_AUTH_PAGE}?${params.toString()}`;
}

export function buildClaudeAuthUrl(state: string, challenge: string, redirectUri: string): string {
  // Mirrors CLIProxyAPI's GenerateAuthURL exactly: code=true + public
  // client + S256 PKCE challenge.
  const params = new URLSearchParams({
    code: "true",
    client_id: CLAUDE_CLIENT_ID,
    response_type: "code",
    redirect_uri: redirectUri,
    scope: CLAUDE_SCOPE,
    code_challenge: challenge,
    code_challenge_method: "S256",
    state,
  });
  return `${CLAUDE_AUTH_URL}?${params.toString()}`;
}

/** 32 random bytes as base64url — the state format the official CLI uses. */
export function commandcodeState(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export async function pkcePair(): Promise<{ verifier: string; challenge: string }> {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  const verifier = btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  const challenge = btoa(String.fromCharCode(...new Uint8Array(digest)))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
  return { verifier, challenge };
}

export function randomState(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(24));
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export type OAuthCallback = { code: string; state: string };

/** Accepts a pasted callback URL (or query string) and extracts code + state. */
export function parseOAuthCallback(input: string): OAuthCallback {
  const text = input.trim();
  if (!text) throw new Error("Paste the callback URL from your browser's address bar.");
  // Bare code without state cannot be CSRF-checked — require the full URL.
  let query = "";
  try {
    if (text.includes("://") || text.startsWith("localhost") || text.startsWith("127.0.0.1")) {
      const normalized = text.includes("://") ? text : `http://${text}`;
      query = new URL(normalized).search;
    } else if (text.includes("code=")) {
      query = text.startsWith("?") ? text : `?${text}`;
    } else {
      throw new Error("That looks like just the code — paste the full callback URL so the sign-in can be verified.");
    }
  } catch (err) {
    if (err instanceof Error && err.message.startsWith("That looks like")) throw err;
    throw new Error("Could not read that URL — paste the full callback URL from the address bar.");
  }
  const params = new URLSearchParams(query);
  const error = params.get("error");
  if (error) {
    throw new Error(`Sign-in was not approved (${params.get("error_description") ?? error}). Try again.`);
  }
  const code = params.get("code") ?? "";
  const state = params.get("state") ?? "";
  if (!code) throw new Error("No authorization code in that URL — complete the browser sign-in first, then paste the URL you land on.");
  if (!state) throw new Error("No state in that URL — paste the full callback URL so the sign-in can be verified.");
  return { code, state };
}

/** Reads the email claim out of an OpenID id_token without any network call. */
export function decodeIdTokenEmail(idToken: string | null | undefined): string {
  if (!idToken) return "";
  const parts = idToken.split(".");
  if (parts.length < 2) return "";
  try {
    const payload = JSON.parse(atob(parts[1].replace(/-/g, "+").replace(/_/g, "/"))) as { email?: unknown };
    return typeof payload.email === "string" ? payload.email : "";
  } catch {
    return "";
  }
}

export type OAuthCredential = {
  access_token: string;
  refresh_token?: string | null;
  id_token?: string | null;
  email: string;
};

/** Stores an OAuth credential (preview: local+session storage so it survives tabs/restarts; desktop: OS keyring via Tauri). */
export async function storeOAuthCredential(providerSlug: string, accountId: string, credential: OAuthCredential): Promise<void> {
  if (inTauri()) {
    await tauriInvoke("set_local_token", {
      providerSlug,
      accountId,
      token: JSON.stringify({
        access_token: credential.access_token,
        refresh_token: credential.refresh_token ?? null,
        id_token: credential.id_token ?? null,
        account_id: accountId,
        email: credential.email,
        expires_in: null,
      }),
    });
    return;
  }
  const raw = JSON.stringify(credential);
  setToken(providerSlug, accountId, raw);
}

export function readPreviewOAuthCredential(providerSlug: string, accountId: string): OAuthCredential | null {
  try {
    // Single session-first priority (same as getToken) — never resurrect a
    // rotated session value from a stale local copy.
    const raw = getToken(providerSlug, accountId);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<OAuthCredential>;
    if (typeof parsed.access_token !== "string" || !parsed.access_token) return null;
    return {
      access_token: parsed.access_token,
      refresh_token: typeof parsed.refresh_token === "string" ? parsed.refresh_token : null,
      id_token: typeof parsed.id_token === "string" ? parsed.id_token : null,
      email: typeof parsed.email === "string" ? parsed.email : "",
    };
  } catch {
    return null;
  }
}

export type OAuthFlowState = { state: string; verifier?: string; redirectUri?: string };

const flowKey = (providerSlug: string) => `proxydock-oauth-flow:${providerSlug}`;

export function saveOAuthFlow(providerSlug: string, flow: OAuthFlowState): void {
  try {
    sessionStorage.setItem(flowKey(providerSlug), JSON.stringify(flow));
  } catch {
    // storage blocked — sign-in still works via the paste fallback
  }
}

export function readOAuthFlow(providerSlug: string): OAuthFlowState | null {
  try {
    const raw = sessionStorage.getItem(flowKey(providerSlug));
    if (!raw) return null;
    return JSON.parse(raw) as OAuthFlowState;
  } catch {
    return null;
  }
}

export function clearOAuthFlow(providerSlug: string): void {
  try {
    sessionStorage.removeItem(flowKey(providerSlug));
  } catch {
    // ignore
  }
}

type TokenResponse = {
  access_token?: string;
  refresh_token?: string | null;
  id_token?: string | null;
  expires_in?: number | null;
  error?: unknown;
  error_description?: unknown;
};

/** Upstream OAuth errors are sometimes a string, sometimes {message}. */
function tokenErrorMessage(data: { error?: unknown; error_description?: unknown }, fallback: string): string {
  if (typeof data.error_description === "string" && data.error_description) return data.error_description;
  const err = data.error as { message?: unknown } | string | undefined;
  if (typeof err === "string" && err) return err;
  if (err && typeof err === "object" && typeof err.message === "string" && err.message) return err.message;
  return fallback;
}

/** Exchanges a Codex authorization code using the saved flow (auto + paste paths). */
async function exchangeCodexCode(code: string): Promise<OAuthCredential> {
  const flow = readOAuthFlow("chatgpt");
  if (!flow?.verifier) throw new Error("Sign-in session expired — start the browser sign-in again.");
  let resp: Response;
  try {
    resp = await fetch(CODEX_TOKEN_PATH, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code, verifier: flow.verifier, redirect_uri: flow.redirectUri ?? CODEX_CALLBACK_URL }),
    });
  } catch {
    throw new Error("Could not reach the token endpoint — is the dev server running? Try again.");
  }
  const tokens = (await resp.json().catch(() => ({}))) as TokenResponse;
  if (!resp.ok || !tokens.access_token) {
    // Desktop note: this guard must never fire from a 200-without-token body
    // in desktop — desktop exchanges via Rust commands (secrets.rs), so this
    // preview/dev-server path is unreachable there. Kept as a guard.
    throw new Error(tokenErrorMessage(tokens, `Sign-in failed (${resp.status}). The code may have expired — try again.`));
  }
  clearOAuthFlow("chatgpt");
  return {
    access_token: tokens.access_token,
    refresh_token: tokens.refresh_token ?? null,
    id_token: tokens.id_token ?? null,
    email: decodeIdTokenEmail(tokens.id_token),
  };
}

/** Completes a Codex browser sign-in from a pasted callback URL (preview fallback). */
export async function completeCodexSignIn(callbackInput: string): Promise<OAuthCredential> {
  const { code, state } = parseOAuthCallback(callbackInput);
  const flow = readOAuthFlow("chatgpt");
  if (!flow || flow.state !== state) {
    throw new Error("That URL does not match this sign-in attempt — open the sign-in page fresh from Proxy Dock and try again.");
  }
  return exchangeCodexCode(code);
}

/** Exchanges an Antigravity authorization code using the saved flow (auto + paste paths). */
async function exchangeAntigravityCode(code: string): Promise<OAuthCredential> {
  const flow = readOAuthFlow("antigravity");
  if (!flow) throw new Error("Sign-in session expired — start the browser sign-in again.");
  let resp: Response;
  try {
    resp = await fetch(ANTIGRAVITY_TOKEN_PATH, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        code,
        redirect_uri: flow.redirectUri ?? ANTIGRAVITY_CALLBACK_URL,
        code_verifier: flow.verifier,
      }),
    });
  } catch {
    throw new Error("Could not reach the token endpoint — is the dev server running? Try again.");
  }
  const data = (await resp.json().catch(() => ({}))) as { ok?: boolean; tokens?: TokenResponse; email?: string; error?: string };
  if (!resp.ok || !data.ok || !data.tokens?.access_token) {
    // Desktop note: this guard must never fire from a 200-without-token body
    // in desktop — desktop exchanges via Rust commands (secrets.rs), so this
    // preview/dev-server path is unreachable there. Kept as a guard.
    throw new Error(data.error ?? `Sign-in failed (${resp.status}). The code may have expired — try again.`);
  }
  clearOAuthFlow("antigravity");
  return {
    access_token: data.tokens.access_token,
    refresh_token: data.tokens.refresh_token ?? null,
    id_token: data.tokens.id_token ?? null,
    email: typeof data.email === "string" ? data.email : "",
  };
}

/** Completes an Antigravity browser sign-in from a pasted callback URL (preview fallback). */
export async function completeAntigravitySignIn(callbackInput: string): Promise<OAuthCredential> {
  const { code, state } = parseOAuthCallback(callbackInput);
  const flow = readOAuthFlow("antigravity");
  if (!flow || flow.state !== state) {
    throw new Error("That URL does not match this sign-in attempt — open the sign-in page fresh from Proxy Dock and try again.");
  }
  return exchangeAntigravityCode(code);
}

/** Exchanges a Claude authorization code using the saved flow (auto + paste paths). */
async function exchangeClaudeCode(code: string): Promise<OAuthCredential> {
  const flow = readOAuthFlow("claude");
  if (!flow?.verifier) throw new Error("Sign-in session expired — start the browser sign-in again.");
  let resp: Response;
  try {
    resp = await fetch(CLAUDE_TOKEN_PATH, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code, verifier: flow.verifier, redirect_uri: flow.redirectUri ?? CLAUDE_CALLBACK_URL, state: flow.state }),
    });
  } catch {
    throw new Error("Could not reach the token endpoint — is the dev server running? Try again.");
  }
  const data = (await resp.json().catch(() => ({}))) as {
    ok?: boolean;
    tokens?: { access_token?: string; refresh_token?: string | null };
    email?: string;
    error?: string;
  };
  if (!resp.ok || !data.ok || !data.tokens?.access_token) {
    // Desktop note: this guard must never fire from a 200-without-token body
    // in desktop — desktop exchanges via Rust commands (secrets.rs), so this
    // preview/dev-server path is unreachable there. Kept as a guard.
    throw new Error(data.error ?? `Sign-in failed (${resp.status}). The code may have expired — try again.`);
  }
  clearOAuthFlow("claude");
  return {
    access_token: data.tokens.access_token,
    refresh_token: data.tokens.refresh_token ?? null,
    id_token: null,
    email: typeof data.email === "string" ? data.email : "",
  };
}

/** Completes a Claude browser sign-in from a pasted callback URL (preview fallback). */
export async function completeClaudeSignIn(callbackInput: string): Promise<OAuthCredential> {
  const { code, state } = parseOAuthCallback(callbackInput);
  const flow = readOAuthFlow("claude");
  if (!flow || flow.state !== state) {
    throw new Error("That URL does not match this sign-in attempt — open the sign-in page fresh from Proxy Dock and try again.");
  }
  return exchangeClaudeCode(code);
}

/** Completes a sign-in whose code arrived via the loopback listener (already bound to this flow's state). */
export async function completeAutoOAuth(slug: "chatgpt" | "antigravity" | "claude", code: string): Promise<OAuthCredential> {
  if (slug === "chatgpt") return exchangeCodexCode(code);
  if (slug === "antigravity") return exchangeAntigravityCode(code);
  return exchangeClaudeCode(code);
}

export type LoopbackStatus = { codexPort: number; antigravityPort: number; claudePort: number };

/** Which official callback ports the dev server managed to listen on (0 = taken). */
export async function loopbackStatus(): Promise<LoopbackStatus> {
  const resp = await fetch("/__proxydock/loopback-status");
  if (!resp.ok) throw new Error("dev server unreachable");
  return (await resp.json()) as LoopbackStatus;
}

export type PreviewOAuthStarted = { state: string; authUrl: string; auto: boolean };

/**
 * Opens the provider's own login tab for the browser preview. When the dev
 * server holds the official loopback port, the redirect lands back in the
 * app automatically (auto=true) — otherwise the caller shows the paste
 * fallback with the same auth URL.
 */
export async function startPreviewOAuth(slug: "chatgpt" | "antigravity" | "claude"): Promise<PreviewOAuthStarted> {
  let status: LoopbackStatus = { codexPort: 0, antigravityPort: 0, claudePort: 0 };
  try {
    status = await loopbackStatus();
  } catch {
    // Dev server without the OAuth middleware — paste fallback only.
  }
  if (slug === "chatgpt") {
    const { verifier, challenge } = await pkcePair();
    const state = randomState();
    if (status.codexPort > 0) {
      const redirectUri = `http://localhost:${status.codexPort}${CODEX_CALLBACK_PATH}`;
      saveOAuthFlow(slug, { state, verifier, redirectUri });
      const authUrl = buildCodexAuthUrl(state, challenge, redirectUri);
      void openExternal(authUrl);
      return { state, authUrl, auto: true };
    }
    saveOAuthFlow(slug, { state, verifier, redirectUri: CODEX_CALLBACK_URL });
    const authUrl = buildCodexAuthUrl(state, challenge, CODEX_CALLBACK_URL);
    void openExternal(authUrl);
    return { state, authUrl, auto: false };
  }
  const state = randomState();
  if (slug === "claude") {
    const { verifier, challenge } = await pkcePair();
    if (status.claudePort > 0) {
      const redirectUri = `http://localhost:${status.claudePort}${CLAUDE_CALLBACK_PATH}`;
      saveOAuthFlow(slug, { state, verifier, redirectUri });
      const authUrl = buildClaudeAuthUrl(state, challenge, redirectUri);
      void openExternal(authUrl);
      return { state, authUrl, auto: true };
    }
    saveOAuthFlow(slug, { state, verifier, redirectUri: CLAUDE_CALLBACK_URL });
    const authUrl = buildClaudeAuthUrl(state, challenge, CLAUDE_CALLBACK_URL);
    void openExternal(authUrl);
    return { state, authUrl, auto: false };
  }
  if (status.antigravityPort > 0) {
    const { verifier, challenge } = await pkcePair();
    const redirectUri = `http://localhost:${status.antigravityPort}${ANTIGRAVITY_CALLBACK_PATH}`;
    saveOAuthFlow(slug, { state, verifier, redirectUri });
    const authUrl = buildAntigravityAuthUrl(state, redirectUri, await effectiveAntigravityClientId(), challenge);
    void openExternal(authUrl);
    return { state, authUrl, auto: true };
  }
  {
    const { verifier, challenge } = await pkcePair();
    saveOAuthFlow(slug, { state, verifier, redirectUri: ANTIGRAVITY_CALLBACK_URL });
    const authUrl = buildAntigravityAuthUrl(state, ANTIGRAVITY_CALLBACK_URL, await effectiveAntigravityClientId(), challenge);
    void openExternal(authUrl);
    return { state, authUrl, auto: false };
  }
}

/** Effective Antigravity client ID for the preview flow (dev middleware env,
 * else the built-in public default). Desktop reads the vault bundle instead.
 * Never throws — falls back to the built-in ID. */
export async function effectiveAntigravityClientId(): Promise<string> {
  try {
    const resp = await fetch("/__proxydock/antigravity-client");
    if (resp.ok) {
      const data = (await resp.json().catch(() => ({}))) as { client_id?: unknown };
      if (typeof data.client_id === "string" && data.client_id.trim()) return data.client_id.trim();
    }
  } catch {
    // Preview middleware absent (production) — built-in default applies.
  }
  return ANTIGRAVITY_CLIENT_ID;
}

/**
 * Waits for the loopback listener to capture the provider redirect for this
 * flow's state. Resolves with the authorization code.
 */
export async function waitForLoopbackCode(
  slug: "chatgpt" | "antigravity" | "claude",
  state: string,
  signal: AbortSignal,
  timeoutMs = 5 * 60 * 1000,
): Promise<string> {
  const provider = slug === "chatgpt" ? "codex" : slug;
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (signal.aborted) throw new Error("Sign-in was cancelled.");
    if (Date.now() > deadline) {
      throw new Error("Still waiting for the browser — if you already approved, paste the callback URL below instead.");
    }
    await new Promise((resolve) => setTimeout(resolve, 1500));
    if (signal.aborted) throw new Error("Sign-in was cancelled.");
    let resp: Response;
    try {
      resp = await fetch(`/__proxydock/oauth-pending?provider=${provider}&state=${encodeURIComponent(state)}`);
    } catch {
      continue;
    }
    if (!resp.ok) continue;
    const data = (await resp.json().catch(() => ({}))) as { status?: string; code?: string; error?: string };
    if (data.status === "ready" && data.code) return data.code;
    if (data.status === "error") throw new Error(data.error ?? "Sign-in was not approved.");
  }
}

export type CommandCodeVerified = { provider: string; account: string; email: string; userName: string };

export type ClaudeVerified = { provider: string; account: string; models: string[] };

/**
 * Tests an Anthropic API key against the documented Models API and returns
 * the model ids for display. OAuth setup tokens (`sk-ant-oat-*`) are not
 * API keys — they are rejected with a pointer to browser sign-in, mirroring
 * the desktop verifier exactly.
 */
export async function verifyClaudeKey(apiKey: string): Promise<ClaudeVerified> {
  const secret = apiKey.trim();
  if (secret.length < 8) {
    throw new Error("That key looks too short — paste the full API key.");
  }
  if (secret.startsWith("sk-ant-oat-")) {
    throw new Error("That looks like a Claude OAuth setup token, not an API key — use browser sign-in instead.");
  }
  if (inTauri()) {
    return tauriInvoke<ClaudeVerified>("verify_claude_key", { apiKey: secret });
  }
  // Browser preview: the documented Models API needs x-api-key +
  // anthropic-version, which a page fetch may set — but api.anthropic.com
  // sends no CORS headers, so go through the same-origin vite proxy first.
  const headers = { "x-api-key": secret, "anthropic-version": "2023-06-01", Accept: "application/json" };
  let resp: Response | null = null;
  for (const url of ["/claude-api/v1/models", CLAUDE_MODELS_URL]) {
    try {
      resp = await fetch(url, { headers });
      break;
    } catch {
      resp = null;
    }
  }
  if (!resp) {
    throw new Error(
      "Could not reach api.anthropic.com to verify the key — check your connection, then try again. (Browser preview uses a local proxy; the desktop app verifies directly.)",
    );
  }
  if (resp.status === 401 || resp.status === 403) {
    throw new Error("api.anthropic.com rejected the key (unauthorized) — check it and try again.");
  }
  if (!resp.ok) {
    throw new Error(`api.anthropic.com verification failed (${resp.status}) — try again.`);
  }
  const body = (await resp.json().catch(() => ({}))) as { data?: { id?: string }[] };
  const items = Array.isArray(body.data) ? body.data : [];
  const models = items.map((m) => m.id ?? "").filter((id) => id.length > 0);
  const account = await deriveAccountId(null, secret);
  setToken("claude", account, secret);
  return { provider: "claude", account, models };
}

/** Tests a Command Code API key and returns the account identity (email). */
export async function verifyCommandCodeKey(apiKey: string): Promise<CommandCodeVerified> {
  const secret = apiKey.trim();
  if (secret.length < 8) {
    throw new Error("That key looks too short — paste the full API key.");
  }
  if (inTauri()) {
    return tauriInvoke<CommandCodeVerified>("verify_commandcode_key", { apiKey: secret });
  }
  let resp: Response;
  try {
    resp = await fetch(COMMANDCODE_WHOAMI_PATH, {
      headers: { Authorization: `Bearer ${secret}`, Accept: "application/json", "x-command-code-version": "0.24.1" },
    });
  } catch {
    throw new Error("Could not reach Command Code to verify the key — check your connection, then try again.");
  }
  if (resp.status === 401 || resp.status === 403) {
    throw new Error("Command Code rejected the key (unauthorized) — check it and try again.");
  }
  if (!resp.ok) {
    throw new Error(`Command Code verification failed (${resp.status}) — try again.`);
  }
  const body = (await resp.json().catch(() => ({}))) as { user?: { email?: string; userName?: string; name?: string } };
  const email = body.user?.email?.trim() ?? "";
  const userName = body.user?.userName?.trim() ?? body.user?.name?.trim() ?? "";
  if (!email && !userName) {
    throw new Error("Command Code accepted the key but returned no account identity.");
  }
  try {
    const account = await deriveAccountId(email || null, secret);
    setToken("commandcode", account, secret);
    return { provider: "commandcode", account, email, userName };
  } catch {
    // ignore storage errors
    return { provider: "commandcode", account: await deriveAccountId(email || null, secret), email, userName };
  }
}

export type CommandCodeCallbackPayload = {
  apiKey: string;
  userName: string;
  keyName: string;
  /** Stable account slot id (desktop only — keyring already holds the key). */
  account?: string;
  /** Display identity for the desktop flow. */
  email?: string;
};

/**
 * Opens the Command Code Studio CLI-login page and waits for Studio to POST
 * the issued key to the local callback. Works in the desktop app (Rust
 * callback server) and the browser preview (dev middleware). Throws when
 * aborted via `signal`.
 */
export async function startCommandCodeBrowserFlow(signal: AbortSignal): Promise<CommandCodeCallbackPayload> {
  if (inTauri()) {
    // NO DOUBLE-OPEN: Rust `start_commandcode_sign_in` already opens the
    // system browser via secrets.rs `open_browser` — frontend must not call
    // openExternal on the desktop path.
    const invoked = tauriInvoke<BrowserSignInResult>("start_commandcode_sign_in", {});
    const aborted: Promise<never> = new Promise((_, reject) => {
      if (signal.aborted) {
        reject(new Error("Sign-in was cancelled."));
        return;
      }
      signal.addEventListener("abort", () => reject(new Error("Sign-in was cancelled.")), { once: true });
    });
    // Abort/timeout parity with the preview path: Rust itself times out after
    // 300s, but the frontend await must also unlatch so no permanent
    // "Waiting for browser…" state survives a lost callback.
    const timedOut: Promise<never> = new Promise((_, reject) => {
      setTimeout(
        () =>
          reject(
            new Error(
              "no callback from Studio after 5 minutes — use the \"Copy your API key\" fallback on the Studio page and paste the key below",
            ),
          ),
        5 * 60 * 1000,
      );
    });
    const result = await Promise.race([invoked, aborted, timedOut]);
    // Desktop stored + verified the key in the keyring under result.account.
    return { apiKey: "", userName: result.email || result.account, keyName: "", account: result.account, email: result.email };
  }
  const state = commandcodeState();
  // Exact CLI shape: http://127.0.0.1:{port}/callback on the dev server port.
  // When the page itself runs on a default port (80/443, so location.port is
  // "") omit the colon — `http://127.0.0.1:/callback` never parses.
  const portSuffix = window.location.port ? `:${window.location.port}` : "";
  const callback = `http://127.0.0.1${portSuffix}${COMMANDCODE_CALLBACK_PATH}`;
  void openExternal(buildCommandCodeAuthUrl(callback, state));
  // 5-minute parity with the Rust 300s callback server + waitForLoopbackCode.
  const deadline = Date.now() + 5 * 60 * 1000;
  for (;;) {
    if (signal.aborted) throw new Error("Sign-in was cancelled.");
    if (Date.now() > deadline) {
      throw new Error(
        "no callback from Studio after 5 minutes — use the \"Copy your API key\" fallback on the Studio page and paste the key below",
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 2000));
    if (signal.aborted) throw new Error("Sign-in was cancelled.");
    let resp: Response;
    try {
      resp = await fetch(`/__proxydock/commandcode-pending?state=${encodeURIComponent(state)}`);
    } catch {
      continue;
    }
    if (!resp.ok) continue;
    const data = (await resp.json().catch(() => ({}))) as { status?: string; payload?: CommandCodeCallbackPayload };
    if (data.status === "ready" && data.payload?.apiKey) return data.payload;
  }
}

// ---------------------------------------------------------------------------
// Live quota (browser preview — the desktop app uses the Rust quota module
// with the same sources and mapping rules; keep the two in sync).
// Sources: wham/usage (ChatGPT), cloudcode-pa loadCodeAssist /
// fetchAvailableModels / retrieveUserQuotaSummary (Antigravity),
// /alpha/whoami+billing+usage (Command Code, mirroring the official CLI).
// ---------------------------------------------------------------------------

export type QuotaFetchResult = { plan: string | null; quota: QuotaRow[]; refreshed: boolean; modelCount?: number | null };

function quotaNow(): number {
  return Date.now();
}

function clampRowPct(value: number): number {
  return Math.max(0, Math.min(100, value));
}

/** Epoch ms / epoch seconds / RFC-3339 — same defensive parse as Rust. */
function parseResetValue(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) {
    if (value > 1_000_000_000_000) return value;
    if (value > 1_000_000_000) return value * 1000;
    return value;
  }
  if (typeof value === "string" && value) {
    const ms = Date.parse(value);
    return Number.isNaN(ms) ? null : ms;
  }
  return null;
}

function fmtReset(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getMonth() + 1)}/${pad(d.getDate())}, ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function fmtResetIn(ms: number, now: number): string {
  const mins = Math.max(0, Math.floor((ms - now) / 60000));
  if (mins < 1) return "in under a minute";
  if (mins < 60) return `in ${mins} minute${mins === 1 ? "" : "s"}`;
  const hours = Math.floor(mins / 60);
  if (hours < 48) return `in ${hours} hour${hours === 1 ? "" : "s"}`;
  const days = Math.floor(hours / 24);
  return `in ${days} day${days === 1 ? "" : "s"}`;
}

function makeQuotaRow(id: string, label: string, remaining: number | null, resetMs: number | null, now: number): QuotaRow {
  return {
    id,
    label,
    remainingPct: remaining === null ? null : clampRowPct(remaining),
    resetText: resetMs === null ? "" : fmtReset(resetMs),
    resetInText: resetMs === null ? "" : fmtResetIn(resetMs, now),
    urgent: resetMs !== null && resetMs - now < 60 * 60 * 1000,
  };
}

function firstObject(value: Record<string, unknown> | null | undefined, keys: string[]): Record<string, unknown> | null {
  if (!value || typeof value !== "object") return null;
  for (const key of keys) {
    const candidate = value[key];
    if (candidate && typeof candidate === "object" && !Array.isArray(candidate)) {
      return candidate as Record<string, unknown>;
    }
  }
  return null;
}

function numField(obj: Record<string, unknown>, key: string): number | null {
  const value = obj[key];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function strField(obj: Record<string, unknown>, key: string): string {
  const value = obj[key];
  return typeof value === "string" ? value : "";
}

/**
 * Limit window from any nesting. Accepts unambiguous direct-remaining
 * shapes first, then the classic `{used, cap}` pair. Null when nothing
 * usable is present. Mirrors the Rust mapper.
 */
function commandcodeLimitRow(id: string, label: string, window: unknown, now: number): QuotaRow | null {
  if (!window || typeof window !== "object") return null;
  const rec = window as Record<string, unknown>;
  const num = (keys: string[]): number | null => {
    for (const key of keys) {
      const value = rec[key];
      if (typeof value === "number" && Number.isFinite(value)) return value;
    }
    return null;
  };
  const resetRaw = rec.resetAt ?? rec.reset ?? rec.expiry;
  const reset = parseResetValue(resetRaw);
  const direct = num(["remainingPercent", "percentLeft", "percent_left"]);
  if (direct !== null) return makeQuotaRow(id, label, direct, reset, now);
  const remaining = num(["remaining"]);
  const capLike = num(["cap", "limit", "total"]);
  if (remaining !== null && capLike !== null && capLike > 0) {
    return makeQuotaRow(id, label, (100 * remaining) / capLike, reset, now);
  }
  const used = num(["used"]);
  const cap = num(["cap"]);
  if (used === null || cap === null || cap <= 0) return null;
  return makeQuotaRow(id, label, 100 * (1 - used / cap), reset, now);
}

/** Weekly alias kept for the fallback call site below. */
function commandcodeWeeklyRow(window: unknown, now: number): QuotaRow | null {
  return commandcodeLimitRow("window-weekly", "Weekly", window, now);
}

/**
 * Week-to-date spend from the already-fetched usage summary. Accepts an
 * explicit weekly key or a daily breakdown (summing entries dated within
 * the last 7 days). Null when the shape carries nothing weekly — the caller
 * then shows no row rather than a fabricated one. Mirrors the Rust mapper.
 */
function commandcodeWeeklySpend(summary: Record<string, unknown> | null, now: number): number | null {
  if (!summary) return null;
  for (const key of ["weeklyCost", "weekToDate", "weeklyUsage", "totalWeeklyCost", "weekSpend"]) {
    const value = summary[key];
    if (typeof value === "number" && Number.isFinite(value)) return Math.max(0, value);
  }
  for (const key of ["daily", "byDay", "days", "breakdown", "history"]) {
    const items = summary[key];
    if (!Array.isArray(items)) continue;
    let sum = 0;
    let dated = 0;
    for (const item of items) {
      if (!item || typeof item !== "object") continue;
      const rec = item as Record<string, unknown>;
      let cost: number | null = null;
      for (const costKey of ["cost", "totalCost", "amount", "spent", "value"]) {
        const value = rec[costKey];
        if (typeof value === "number" && Number.isFinite(value)) {
          cost = value;
          break;
        }
      }
      if (cost === null) continue;
      // Undated entries can't be scoped to the week — skip them rather than
      // misattributing period spend as weekly spend.
      const dayRaw = rec.date ?? rec.day ?? rec.period;
      const day = parseResetValue(dayRaw);
      if (day !== null && day <= now && now - day <= 7 * 86_400_000) {
        sum += Math.max(0, cost);
        dated += 1;
      }
    }
    if (dated > 0) return sum;
  }
  return null;
}

// --- ChatGPT / Codex (wham/usage) ------------------------------------------

function whamUsed(window: Record<string, unknown>): number | null {
  const left = numField(window, "percent_left");
  if (left !== null) return 100 - left;
  const used = numField(window, "used_percent") ?? numField(window, "usedPercent");
  return used;
}

function whamResetMs(window: Record<string, unknown>, now: number): number | null {
  const ms = numField(window, "reset_time_ms");
  if (ms !== null) return ms;
  for (const key of ["reset_at", "resetsAt"]) {
    const parsed = parseResetValue(window[key]);
    if (parsed !== null) return parsed;
  }
  const after = numField(window, "reset_after_seconds");
  return after === null ? null : now + after * 1000;
}

function whamSlotName(seconds: number | null, fallback: string): string {
  if (seconds === null) return fallback;
  return seconds >= 172800 ? "Weekly" : "5-hour";
}

function whamPlanLabel(planType: string): string {
  switch (planType.trim().toLowerCase()) {
    case "free": return "Free";
    case "go": return "Go";
    case "plus": return "Plus";
    case "pro": return "Pro";
    case "team": return "Team";
    case "enterprise": return "Enterprise";
    case "edu": return "Edu";
    default: return planType.trim();
  }
}

function whamWindowRows(
  prefix: string,
  title: string | null,
  primary: Record<string, unknown> | null,
  secondary: Record<string, unknown> | null,
  now: number,
  out: QuotaRow[],
): void {
  const usedLabels: string[] = [];
  const slots: [string, Record<string, unknown> | null, string][] = [
    ["primary", primary, "5-hour"],
    ["secondary", secondary, "Weekly"],
  ];
  for (const [slot, window, fallback] of slots) {
    if (!window) continue;
    const used = whamUsed(window);
    if (used === null) continue;
    let label = whamSlotName(numField(window, "limit_window_seconds"), fallback);
    if (usedLabels.includes(label)) label = `${label} · ${slot}`;
    usedLabels.push(label);
    out.push(
      makeQuotaRow(`${prefix}-${slot}`, title ? `${title} · ${label}` : label, 100 - used, whamResetMs(window, now), now),
    );
  }
}

export function mapWhamQuota(body: unknown, now: number): { plan: string | null; quota: QuotaRow[] } {
  const root = firstObject(body as Record<string, unknown>, ["rate_limit", "rate_limits"]);
  if (!root) throw new Error("no rate-limit data in wham response");
  const rows: QuotaRow[] = [];
  whamWindowRows(
    "window",
    null,
    firstObject(root, ["five_hour", "primary_window", "primary"]),
    firstObject(root, ["weekly", "secondary_window", "secondary"]),
    now,
    rows,
  );
  const extra = root["additional_rate_limits"];
  if (Array.isArray(extra)) {
    for (const item of extra) {
      if (!item || typeof item !== "object") continue;
      const rec = item as Record<string, unknown>;
      const id = strField(rec, "id") || "extra";
      const title = strField(rec, "title") || id;
      const p = firstObject(rec, ["primary_window", "primary"]);
      const s = firstObject(rec, ["secondary_window", "secondary"]);
      if (p || s) whamWindowRows(`extra-${id}`, title, p, s, now, rows);
    }
  }
  const review = firstObject(root, ["code_review_rate_limit"]);
  if (review) {
    const window = firstObject(review, ["primary_window", "primary"]);
    if (window) {
      const used = whamUsed(window);
      if (used !== null) {
        rows.push(makeQuotaRow("code-review", "Code review", 100 - used, whamResetMs(window, now), now));
      }
    }
  }
  if (rows.length === 0) throw new Error("no rate-limit windows in wham response");
  const rawPlan = (body as Record<string, unknown>)["plan_type"];
  const plan = typeof rawPlan === "string" && rawPlan.trim() ? whamPlanLabel(rawPlan) : null;
  return { plan, quota: rows };
}

async function whamGet(accessToken: string, accountId: string): Promise<unknown> {
  const headers: Record<string, string> = { Authorization: `Bearer ${accessToken}`, Accept: "application/json" };
  if (accountId.trim()) headers["ChatGPT-Account-Id"] = accountId.trim();
  let resp: Response;
  try {
    resp = await fetch("/chatgpt-wham/wham/usage", { headers });
  } catch {
    throw new Error("Could not reach ChatGPT to check quota — check your connection.");
  }
  if (resp.status === 401) throw new Error("unauthorized");
  if (!resp.ok) throw new Error(`ChatGPT quota check failed (${resp.status})`);
  return resp.json().catch(() => ({}));
}

async function refreshCodexToken(refreshToken: string): Promise<{ access_token: string; refresh_token?: string | null; id_token?: string | null }> {
  let resp: Response;
  try {
    resp = await fetch("/__proxydock/codex-exchange", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ refresh_token: refreshToken }),
    });
  } catch {
    throw new Error("Could not reach the token endpoint — is the dev server running?");
  }
  const data = (await resp.json().catch(() => ({}))) as { access_token?: string; refresh_token?: string | null; id_token?: string | null; error?: unknown; error_description?: unknown };
  if (!resp.ok || !data.access_token) {
    throw new Error("refresh-rejected");
  }
  return data as { access_token: string; refresh_token?: string | null; id_token?: string | null };
}

async function fetchChatGptQuota(accessToken: string, accountId: string): Promise<{ plan: string | null; quota: QuotaRow[] }> {
  const now = quotaNow();
  const body = await whamGet(accessToken, accountId);
  const { plan, quota } = mapWhamQuota(body, now);
  const details = await fetchResetDetails(accessToken, accountId);
  const extras = whamExtraRows(body, details, now);
  return { plan, quota: [...extras, ...quota] };
}

/** Banked reset inventory (best effort — undocumented shape, null when unparseable). */
async function fetchResetDetails(accessToken: string, accountId: string): Promise<unknown | null> {
  const headers: Record<string, string> = { Authorization: `Bearer ${accessToken}`, Accept: "application/json" };
  if (accountId.trim()) headers["ChatGPT-Account-Id"] = accountId.trim();
  for (const path of ["/chatgpt-wham/codex/rate-limit-reset-credits", "/chatgpt-wham/wham/rate-limit-reset-credits"]) {
    try {
      const resp = await fetch(path, { headers });
      if (resp.ok) return (await resp.json().catch(() => null)) as unknown;
    } catch {
      // try the next path
    }
  }
  return null;
}

/** Renewal + banked-reset rows, prepended ahead of the limit bars. */
export function whamExtraRows(body: unknown, details: unknown, now: number): QuotaRow[] {
  const rows: QuotaRow[] = [];
  const root = firstObject(body as Record<string, unknown>, ["rate_limit", "rate_limits"]);
  if (root) {
    const windows = [
      firstObject(root, ["five_hour", "primary_window", "primary"]),
      firstObject(root, ["weekly", "secondary_window", "secondary"]),
    ];
    for (const window of windows) {
      if (!window) continue;
      const seconds = numField(window, "limit_window_seconds");
      if (seconds !== null && seconds >= 172800) {
        const reset = whamResetMs(window, now);
        if (reset !== null) {
          rows.push(makeQuotaRow("renewal", "Renewal time", null, reset, now));
        }
        break;
      }
    }
  }
  const bodyRec = body && typeof body === "object" ? (body as Record<string, unknown>) : null;
  const resetCredits = bodyRec && typeof bodyRec.rate_limit_reset_credits === "object" && bodyRec.rate_limit_reset_credits !== null
    ? (bodyRec.rate_limit_reset_credits as Record<string, unknown>)
    : null;
  const count = resetCredits ? (numField(resetCredits, "available_count") ?? 0) : 0;
  const available = Math.max(0, Math.floor(count));
  if (available > 0) {
    rows.push({
      id: "manual-resets",
      label: "Manual resets",
      remainingPct: null,
      resetText: "",
      resetInText: `${available} available`,
      urgent: false,
    });
  }
  const detailsRec = details && typeof details === "object" ? (details as Record<string, unknown>) : null;
  const items = detailsRec
    ? (["credits", "rows", "items", "data"]
        .map((key) => detailsRec[key])
        .find((value): value is unknown[] => Array.isArray(value)) ?? [])
    : [];
  items.forEach((item, index) => {
    if (!item || typeof item !== "object") return;
    const rec = item as Record<string, unknown>;
    const creditId = ["id", "credit_id", "creditId"].map((key) => strField(rec, key)).find((v) => v) ?? "";
    const title =
      ["title", "name", "description"].map((key) => strField(rec, key)).find((v) => v.trim()) ?? `Reset ${index + 1}`;
    const expiry = ["expires_at", "expiresAt", "expiry", "expires", "grant_expiry", "expiry_time"]
      .map((key) => parseResetValue(rec[key]))
      .find((v): v is number => v !== null) ?? null;
    rows.push(makeQuotaRow(creditId ? `reset-credit:${creditId}` : `reset-credit:auto-${index}`, title, null, expiry, now));
  });
  return rows;
}

// NOTE (honesty cut, plan §0): the banked-reset spender was a dead export
// (no UI callers) and is deleted, along with the backend
// `consume_reset_credit` Tauri command (cut 2026-09-29 — same reason).

// --- Antigravity (cloudcode-pa, via dev middleware) --------------------------

type AntigravityRaw = { project: unknown; models: unknown; summary: unknown };

async function antigravityQuotaRaw(accessToken: string): Promise<AntigravityRaw> {
  let resp: Response;
  try {
    resp = await fetch("/__proxydock/antigravity-quota", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ accessToken }),
    });
  } catch {
    throw new Error("Could not reach Google to check quota — check your connection.");
  }
  const data = (await resp.json().catch(() => ({}))) as { ok?: boolean; error?: string; project?: unknown; models?: unknown; summary?: unknown };
  if (!resp.ok || !data.ok) {
    const message = data.error ?? `Google quota check failed (${resp.status})`;
    throw new Error(message);
  }
  return { project: data.project ?? null, models: data.models ?? null, summary: data.summary ?? null };
}

function antigravityTierLabel(project: unknown): string | null {
  if (!project || typeof project !== "object") return null;
  const rec = project as Record<string, unknown>;
  const pick = (tier: unknown): string => {
    if (typeof tier === "string") return tier;
    if (!tier || typeof tier !== "object") return "";
    const t = tier as Record<string, unknown>;
    const id = typeof t.id === "string" ? t.id : "";
    const name = typeof t.name === "string" ? t.name : "";
    return id || name;
  };
  const paid = pick(rec.paidTier ?? rec.paid_tier);
  const current = pick(
    rec.currentTier ?? rec.current_tier ?? rec.subscriptionTier ?? rec.subscription_tier ?? rec.tier,
  );
  const raw = paid || current || "free-tier";
  return raw.toLowerCase().includes("free") ? "Free" : raw;
}

function interestingAntigravityModel(name: string): boolean {
  const lower = name.toLowerCase();
  return lower.startsWith("gemini") || lower.startsWith("claude") || lower.startsWith("gpt") || lower.startsWith("image") || lower.startsWith("imagen");
}

export function mapAntigravityQuota(raw: AntigravityRaw, now: number): { plan: string | null; quota: QuotaRow[] } {
  const rows: QuotaRow[] = [];
  const summary = raw.summary as Record<string, unknown> | null;
  const groups = summary && Array.isArray(summary.groups) ? (summary.groups as unknown[]) : [];
  for (const group of groups) {
    if (!group || typeof group !== "object") continue;
    const rec = group as Record<string, unknown>;
    const groupName = typeof rec.displayName === "string" && rec.displayName ? rec.displayName : "Quota";
    const buckets = Array.isArray(rec.buckets) ? (rec.buckets as unknown[]) : [];
    for (const bucket of buckets) {
      if (!bucket || typeof bucket !== "object") continue;
      const b = bucket as Record<string, unknown>;
      if (typeof b.remainingFraction !== "number") continue;
      const windowName =
        (typeof b.displayName === "string" && b.displayName) ||
        (typeof b.window === "string" && b.window) ||
        (typeof b.bucketId === "string" && b.bucketId) ||
        "";
      const label = windowName ? `${groupName} · ${windowName}` : groupName;
      const slug = `${groupName} ${windowName}`.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
      rows.push(
        makeQuotaRow(
          `bucket-${slug || "quota"}`,
          label,
          (b.remainingFraction as number) * 100,
          parseResetValue(b.resetTime),
          now,
        ),
      );
    }
  }
  if (rows.length === 0) {
    const models = raw.models as Record<string, unknown> | null;
    const table = models && typeof models.models === "object" && models.models !== null ? (models.models as Record<string, unknown>) : null;
    // Collect first: upstream reuses one display name for several native
    // model ids, so colliding labels are suffixed with their native id
    // (mirrors the desktop mapper — nothing merged or hidden).
    const pending: { name: string; label: string; fraction: number; reset: unknown }[] = [];
    const pushModelRow = (name: string, info: unknown) => {
      if (!interestingAntigravityModel(name) || !info || typeof info !== "object") return;
      const rec = info as Record<string, unknown>;
      const quota = rec.quotaInfo && typeof rec.quotaInfo === "object" ? (rec.quotaInfo as Record<string, unknown>) : null;
      // Some shapes carry the fraction at the top level of the entry.
      const fraction =
        (quota && typeof quota.remainingFraction === "number" ? quota.remainingFraction : null) ??
        (typeof rec.remainingFraction === "number" ? rec.remainingFraction : null);
      if (fraction === null) return;
      const resetRaw = quota?.resetTime ?? rec.resetTime;
      const label = typeof rec.displayName === "string" && rec.displayName ? rec.displayName : name;
      pending.push({ name, label, fraction, reset: resetRaw });
    };
    if (table) {
      if (Array.isArray(table)) {
        table.forEach((info, index) => {
          const rec = (info && typeof info === "object" ? info : {}) as Record<string, unknown>;
          const name =
            (typeof rec.name === "string" && rec.name) ||
            (typeof rec.id === "string" && rec.id) ||
            (typeof rec.model === "string" && rec.model) ||
            `model-${index}`;
          pushModelRow(name, info);
        });
      } else {
        for (const [name, info] of Object.entries(table)) pushModelRow(name, info);
      }
    }
    const labelCounts = new Map<string, number>();
    for (const p of pending) labelCounts.set(p.label, (labelCounts.get(p.label) ?? 0) + 1);
    for (const p of pending) {
      const label = (labelCounts.get(p.label) ?? 0) > 1 ? `${p.label} · ${p.name}` : p.label;
      rows.push(makeQuotaRow(`model-${p.name}`, label, p.fraction * 100, parseResetValue(p.reset), now));
    }
  }
  if (rows.length === 0) throw new Error("no quota data returned — try again later.");
  return { plan: antigravityTierLabel(raw.project), quota: rows };
}

async function refreshGoogleToken(refreshToken: string): Promise<{ access_token: string; refresh_token?: string | null }> {
  let resp: Response;
  try {
    resp = await fetch("/__proxydock/google-refresh", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ refresh_token: refreshToken }),
    });
  } catch {
    throw new Error("Could not reach the token endpoint — is the dev server running?");
  }
  const data = (await resp.json().catch(() => ({}))) as { access_token?: string; refresh_token?: string | null };
  if (!resp.ok || !data.access_token) throw new Error("refresh-rejected");
  return data as { access_token: string; refresh_token?: string | null };
}

// --- Claude (api.anthropic.com/api/oauth/usage, via dev middleware) --------

export type ClaudeUsageRaw = {
  five_hour?: unknown;
  seven_day?: unknown;
  seven_day_opus?: unknown;
  seven_day_sonnet?: unknown;
};

async function claudeQuotaRaw(accessToken: string): Promise<ClaudeUsageRaw> {
  let resp: Response;
  try {
    resp = await fetch(CLAUDE_USAGE_PATH, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ accessToken }),
    });
  } catch {
    throw new Error("Could not reach api.anthropic.com to check quota — check your connection.");
  }
  const data = (await resp.json().catch(() => ({}))) as { ok?: boolean; error?: string; usage?: ClaudeUsageRaw };
  if (!resp.ok || !data.ok) {
    const message = data.error ?? `Claude quota check failed (${resp.status})`;
    throw new Error(message);
  }
  return data.usage ?? {};
}

/**
 * Maps the OAuth usage shape to quota rows. Community-verified (mirrors
 * Claude Code's `/usage`): `{five_hour|seven_day|seven_day_opus|
 * seven_day_sonnet: {utilization, resets_at}}`. `utilization` is percent
 * USED, so remaining = 100 - utilization. Null windows (unused model
 * buckets) render no row — never fabricated.
 */
export function mapClaudeUsage(body: ClaudeUsageRaw | null | undefined, now: number): QuotaRow[] {
  const rows: QuotaRow[] = [];
  if (!body || typeof body !== "object") return rows;
  const windows: [keyof ClaudeUsageRaw, string, string][] = [
    ["five_hour", "claude-five-hour", "5-hour"],
    ["seven_day", "claude-seven-day", "Weekly"],
    ["seven_day_opus", "claude-seven-day-opus", "Weekly · Opus"],
    ["seven_day_sonnet", "claude-seven-day-sonnet", "Weekly · Sonnet"],
  ];
  for (const [key, id, label] of windows) {
    const window = body[key];
    if (!window || typeof window !== "object") continue;
    const rec = window as Record<string, unknown>;
    const used = rec.utilization;
    if (typeof used !== "number" || !Number.isFinite(used)) continue;
    rows.push(makeQuotaRow(id, label, 100 - used, parseResetValue(rec.resets_at ?? rec.resetsAt), now));
  }
  return rows;
}

async function refreshClaudeToken(refreshToken: string): Promise<{ access_token: string; refresh_token?: string | null }> {
  let resp: Response;
  try {
    resp = await fetch(CLAUDE_REFRESH_PATH, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ refresh_token: refreshToken }),
    });
  } catch {
    throw new Error("Could not reach the token endpoint — is the dev server running?");
  }
  const data = (await resp.json().catch(() => ({}))) as { access_token?: string; refresh_token?: string | null };
  if (!resp.ok || !data.access_token) throw new Error("refresh-rejected");
  return data as { access_token: string; refresh_token?: string | null };
}

// --- Command Code (/alpha/*, mirroring the official CLI) ---------------------

/**
 * Plan → credit-pool mapping mirrored from the official Command Code CLI
 * (same table as Rust `quota.rs`; longest-prefix match). The `monthly`
 * number sizes the Credits QUOTA bar pool only — it is never a price and
 * never feeds cost. Labeled here so the provenance is explicit.
 */
function commandcodePlan(id: string): { name: string; monthly: number } | null {
  const key = id.trim().toLowerCase().replace(/_/g, "-");
  const table: [string, string, number][] = [
    ["individual-go", "Go", 10],
    ["individual-goat", "GOAT", 70],
    ["individual-pro-v1", "Pro", 80],
    ["individual-pro", "Pro", 30],
    ["individual-provider", "Provider", 15],
    ["individual-max", "Max", 150],
    ["individual-ultra", "Ultra", 300],
    ["teams-pro", "Teams Pro", 40],
  ];
  let best: [string, string, number] | null = null;
  for (const entry of table) {
    if (key.startsWith(entry[0]) && (!best || entry[0].length > best[0].length)) best = entry;
  }
  return best ? { name: best[1], monthly: best[2] } : null;
}

async function commandcodeGet(apiKey: string, path: string, query: Record<string, string>): Promise<unknown> {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(query)) {
    if (value) params.set(key, value);
  }
  const suffix = params.toString();
  let resp: Response;
  try {
    resp = await fetch(`/commandcode-alpha${path}${suffix ? `?${suffix}` : ""}`, {
      headers: { Authorization: `Bearer ${apiKey}`, Accept: "application/json", "x-command-code-version": "0.24.1" },
    });
  } catch {
    throw new Error("Could not reach Command Code to check quota — check your connection.");
  }
  if (resp.status === 401 || resp.status === 403) throw new Error("unauthorized");
  if (!resp.ok) throw new Error(`Command Code quota check failed (${resp.status})`);
  return resp.json().catch(() => ({}));
}

async function fetchCommandCodeQuota(apiKey: string): Promise<{ plan: string | null; quota: QuotaRow[] }> {
  const now = quotaNow();
  const whoami = (await commandcodeGet(apiKey, "/whoami", { limits: "1" }).catch((err: unknown) => {
    if (err instanceof Error && err.message === "unauthorized") {
      throw new Error("Command Code rejected the key (unauthorized) — check it and try again");
    }
    throw err;
  })) as Record<string, unknown>;
  const org = firstObject(whoami, ["org"]);
  const orgId = (org && typeof org.id === "string" ? org.id : "");
  const orgQuery: Record<string, string> = {};
  if (orgId) orgQuery.orgId = orgId;
  const [creditsRes, subscriptionRes] = await Promise.all([
    commandcodeGet(apiKey, "/billing/credits", orgQuery).catch(() => null),
    commandcodeGet(apiKey, "/billing/subscriptions", orgQuery).catch(() => null),
  ]);
  const credits = firstObject((creditsRes ?? {}) as Record<string, unknown>, ["credits"]);
  const subscription = firstObject((subscriptionRes ?? {}) as Record<string, unknown>, ["data"]);
  const planInfo =
    (subscription && typeof subscription.planId === "string" ? commandcodePlan(subscription.planId) : null) ??
    (credits && typeof credits.planId === "string" ? commandcodePlan(credits.planId) : null);
  const rows: QuotaRow[] = [];
  const windows = credits ? (credits.windowLimits as Record<string, unknown> | undefined) : undefined;
  if (windows && typeof windows === "object") {
    for (const [id, label] of [["fiveHour", "5-hour"], ["weekly", "Weekly"]] as [string, string][]) {
      const window = firstObject(windows, [id]);
      if (!window) continue;
      const used = numField(window, "used") ?? 0;
      const cap = numField(window, "cap") ?? 0;
      if (cap > 0) {
        rows.push(makeQuotaRow(`window-${id}`, label, 100 * (1 - used / cap), parseResetValue(window.resetAt), now));
      }
    }
  }
  // Fallback: the `limits=1` whoami response itself carries limit windows
  // on several plans, and some variants nest the weekly window outside
  // `windowLimits`. Each id renders at most once.
  const pushMissing = (id: string, label: string, window: unknown): void => {
    if (rows.some((row) => row.id === id)) return;
    const rendered = commandcodeLimitRow(id, label, window, now);
    if (rendered) rows.push(rendered);
  };
  for (const nest of ["limits", "windowLimits", "usage", "quotas", "rateLimits"]) {
    const group = (whoami as Record<string, unknown>)[nest];
    if (!group || typeof group !== "object") continue;
    const rec = group as Record<string, unknown>;
    const slots: [string, string, string][] = [
      ["fiveHour", "window-fiveHour", "5-hour"],
      ["five_hour", "window-fiveHour", "5-hour"],
      ["weekly", "window-weekly", "Weekly"],
      ["monthly", "window-monthly", "Monthly"],
    ];
    for (const [key, id, label] of slots) pushMissing(id, label, rec[key]);
  }
  if (!rows.some((row) => row.id === "window-weekly") && credits && typeof credits === "object") {
    const rec = credits as Record<string, unknown>;
    const limits = rec.limits;
    const alts = [
      limits && typeof limits === "object" ? (limits as Record<string, unknown>).weekly : null,
      rec.weeklyLimit,
    ];
    for (const alt of alts) {
      const weekly = commandcodeWeeklyRow(alt, now);
      if (weekly) {
        rows.push(weekly);
        break;
      }
    }
  }
  const monthly = Math.max(0, (credits && numField(credits, "monthlyCredits")) ?? 0);
  const purchased = Math.max(0, (credits && numField(credits, "purchasedCredits")) ?? 0);
  const free = Math.max(0, (credits && numField(credits, "freeCredits")) ?? 0);
  const pool = Math.max(planInfo?.monthly ?? 0, monthly) + purchased + free;
  const since = subscription && typeof subscription.currentPeriodStart === "string" ? subscription.currentPeriodStart : "";
  const summary = (await commandcodeGet(apiKey, "/usage/summary", since ? { ...orgQuery, since } : orgQuery).catch(() => null)) as Record<string, unknown> | null;
  const spent = Math.max(0, (summary && numField(summary, "totalCost")) ?? 0);
  // Week-to-date spend (bar-less info row) from the same summary body.
  const weekSpend = commandcodeWeeklySpend(summary, now);
  if (weekSpend !== null) {
    rows.push({
      id: "weekly-usage",
      label: "Weekly usage",
      remainingPct: null,
      resetText: `$${weekSpend.toFixed(2)}`,
      resetInText: "this week",
      urgent: false,
    });
  }
  if (pool > 0 && (spent > 0 || pool - spent > 0)) {
    const periodEnd = subscription ? parseResetValue(subscription.currentPeriodEnd) : null;
    let resetInText = "";
    let urgent = false;
    if (periodEnd !== null) {
      const days = Math.ceil(Math.max(0, periodEnd - now) / 86_400_000);
      resetInText = `in ${days} day${days === 1 ? "" : "s"}`;
      urgent = days < 3;
    }
    rows.push({
      id: "credits",
      label: "Credits",
      remainingPct: clampRowPct((100 * Math.max(0, pool - spent)) / pool),
      resetText: periodEnd !== null ? fmtReset(periodEnd) : "",
      resetInText,
      urgent,
    });
  }
  const plan = planInfo?.name ?? null;
  if (rows.length === 0 && !plan) {
    throw new Error("No quota data returned for this key — the plan may not expose usage.");
  }
  return { plan, quota: rows };
}

// --- Orchestrator (preview) ---------------------------------------------------

type PreviewSecret =
  | { kind: "oauth"; credential: OAuthCredential }
  | { kind: "raw"; secret: string };

function loadPreviewSecret(provider: string, accountId: string): PreviewSecret {
  const oauth = readPreviewOAuthCredential(provider, accountId);
  if (oauth) return { kind: "oauth", credential: oauth };
  const raw = getToken(provider, accountId) ?? "";
  if (raw.trim().length >= 8) return { kind: "raw", secret: raw.trim() };
  throw new Error("No saved credential found — sign in again.");
}

/**
 * Refresh live quota via the desktop backend (OS keyring). Shape matches
 * QuotaFetchResult (Rust serializes camelCase).
 */
export async function tauriRefreshQuota(providerSlug: string, accountId: string): Promise<QuotaFetchResult> {
  if (!inTauri()) throw new Error("Desktop quota refresh needs the Proxy Dock desktop app.");
  return tauriInvoke<QuotaFetchResult>("refresh_quota", { providerSlug, accountId });
}

/**
 * Refresh live quota for one linked account (browser preview). Returns plan
 * + rows to persist; refreshes OAuth tokens on 401 where possible and stores
 * the rotated credential. Throws honest, displayable errors otherwise.
 */
export async function refreshAccountQuota(provider: string, accountId: string): Promise<QuotaFetchResult> {
  const secret = loadPreviewSecret(provider, accountId);
  if (provider === "chatgpt") {
    const access = secret.kind === "oauth" ? secret.credential.access_token : secret.secret;
    const accountRef = secret.kind === "oauth" ? accountIdForWham(secret.credential) : "";
    try {
      return { ...(await fetchChatGptQuota(access, accountRef)), refreshed: false };
    } catch (err) {
      if (!(err instanceof Error) || err.message !== "unauthorized") throw err;
      if (secret.kind !== "oauth" || !secret.credential.refresh_token) {
        throw new Error("Token rejected or expired — sign in again.");
      }
      const tokens = await refreshCodexToken(secret.credential.refresh_token).catch(() => null);
      if (!tokens) throw new Error("ChatGPT session expired — sign in again.");
      const credential: OAuthCredential = {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token ?? secret.credential.refresh_token,
        id_token: tokens.id_token ?? secret.credential.id_token ?? null,
        email: secret.credential.email,
      };
      await storeOAuthCredential(provider, accountId, credential);
      try {
        return { ...(await fetchChatGptQuota(credential.access_token, accountIdForWham(credential))), refreshed: true };
      } catch {
        throw new Error("ChatGPT session expired — sign in again.");
      }
    }
  }
  if (provider === "antigravity") {
    const access = secret.kind === "oauth" ? secret.credential.access_token : secret.secret;
    try {
      const raw = await antigravityQuotaRaw(access);
      return { ...mapAntigravityQuota(raw, quotaNow()), refreshed: false };
    } catch (err) {
      const forbidden = err instanceof Error && /forbidden|unauthorized|rejected|401/i.test(err.message);
      if (!forbidden || secret.kind !== "oauth" || !secret.credential.refresh_token) {
        throw err instanceof Error ? err : new Error("Google quota check failed.");
      }
      const tokens = await refreshGoogleToken(secret.credential.refresh_token).catch(() => null);
      if (!tokens) throw new Error("Google session expired — sign in again.");
      const credential: OAuthCredential = {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token ?? secret.credential.refresh_token,
        id_token: secret.credential.id_token ?? null,
        email: secret.credential.email,
      };
      await storeOAuthCredential(provider, accountId, credential);
      try {
        const raw = await antigravityQuotaRaw(credential.access_token);
        return { ...mapAntigravityQuota(raw, quotaNow()), refreshed: true };
      } catch {
        throw new Error("Google session expired — sign in again.");
      }
    }
  }
  if (provider === "commandcode") {
    const key = secret.kind === "oauth" ? secret.credential.access_token : secret.secret;
    return { ...(await fetchCommandCodeQuota(key)), refreshed: false };
  }
  if (provider === "opencode") {
    // No documented quota endpoint beyond the usage check — re-verify the
    // key (also refreshes plan, quota rows, and the model count). Never
    // deletes anything on failure.
    if (secret.kind !== "raw") throw new Error("No saved key found — sign in again.");
    const verified = await verifyOpencodeKey(secret.secret);
    return { plan: verified.plan, quota: verified.quota, refreshed: false, modelCount: verified.models.length };
  }
  if (provider === "claude") {
    // OAuth: usage endpoint with one refresh on auth failure (mirrors the
    // desktop quota module). Raw API keys expose no usage endpoint —
    // re-verify via the Models API (model count, no rows). Raw `sk-ant-oat-*`
    // setup tokens are OAuth bearers without refresh: usage tried directly.
    // Never deletes anything on failure.
    if (secret.kind === "oauth") {
      try {
        const raw = await claudeQuotaRaw(secret.credential.access_token);
        return { plan: null, quota: mapClaudeUsage(raw, quotaNow()), refreshed: false };
      } catch (err) {
        const authFailure = err instanceof Error && /unauthorized|forbidden|rejected|expired|401|403/i.test(err.message);
        if (!authFailure || !secret.credential.refresh_token) {
          throw err instanceof Error ? err : new Error("Claude quota check failed.");
        }
        const tokens = await refreshClaudeToken(secret.credential.refresh_token).catch(() => null);
        if (!tokens) throw new Error("Claude session expired — sign in again.");
        const credential: OAuthCredential = {
          access_token: tokens.access_token,
          refresh_token: tokens.refresh_token ?? secret.credential.refresh_token,
          id_token: null,
          email: secret.credential.email,
        };
        await storeOAuthCredential(provider, accountId, credential);
        try {
          const raw = await claudeQuotaRaw(credential.access_token);
          return { plan: null, quota: mapClaudeUsage(raw, quotaNow()), refreshed: true };
        } catch {
          throw new Error("Claude session expired — sign in again.");
        }
      }
    }
    if (secret.secret.trim().startsWith("sk-ant-oat-")) {
      try {
        const raw = await claudeQuotaRaw(secret.secret.trim());
        return { plan: null, quota: mapClaudeUsage(raw, quotaNow()), refreshed: false };
      } catch (err) {
        if (err instanceof Error && /unauthorized|forbidden|rejected|expired|401|403/i.test(err.message)) {
          throw new Error("Claude session expired — sign in again.");
        }
        throw err;
      }
    }
    const verified = await verifyClaudeKey(secret.secret);
    return { plan: null, quota: [], refreshed: false, modelCount: verified.models.length };
  }
  throw new Error("Quota is not available for this provider.");
}

/**
 * Refresh live quota for one account and persist the result to its meta.
 * Shared by the provider pages and Home. Refresh NEVER deletes accounts —
 * failures only throw honest, displayable errors; re-signing restores.
 *
 * Single path (plan §2C/§3A): `POST
 * /api/accounts/:provider/:id/quota/refresh` — same-origin in production
 * (Axum serves the UI), via the vite `/api` proxy in preview. The legacy
 * local paths (Tauri command / preview fetchers below) run ONLY when the
 * gateway itself is unreachable (fetch throws), e.g. preview without the
 * desktop app running.
 *
 * `verifiedAt` is verification time, NOT last-refresh time: refresh
 * preserves it and only fills it when previously unset.
 */
export async function refreshQuotaForAccount(account: LocalAccountMeta): Promise<LocalAccountMeta> {
  let result: QuotaFetchResult;
  try {
    // Desktop webview has no same-origin gateway: prepend the absolute base
    // resolved from `gateway_info` (null in preview → relative, unchanged).
    const apiBase = inTauri() ? await resolveGatewayBase().catch(() => null) : null;
    const quotaPath = `/api/accounts/${encodeURIComponent(account.provider)}/${encodeURIComponent(account.accountId)}/quota/refresh`;
    const resp = await fetch(apiBase ? `${apiBase}${quotaPath}` : quotaPath,
      { method: "POST", headers: { Accept: "application/json", ...(await gatewayAuthHeader()) } },
    );
    if (!resp.ok) {
      const data = (await resp.json().catch(() => null)) as { error?: { message?: string; type?: string } } | null;
      // Same wrong-key state as apiRequest (this raw fetch bypasses it).
      if (resp.status === 401 && data?.error?.type === "proxy_dock_unauthorized") throw new GatewayKeyError();
      const message =
        data && typeof data.error?.message === "string" && data.error.message ? data.error.message : null;
      if (message) throw new Error(message);
      // Non-JSON 5xx = vite proxy with no gateway behind it → fall back.
      if (resp.status >= 500) throw new GatewayUnreachableError();
      throw new Error(`Quota refresh failed (${resp.status}).`);
    }
    result = (await resp.json()) as QuotaFetchResult;
  } catch (err) {
    if (!isGatewayUnreachable(err)) throw err;
    // Gateway unreachable — legacy local refresh (preview without desktop).
    result = inTauri()
      ? await tauriRefreshQuota(account.provider, account.accountId)
      : await refreshAccountQuota(account.provider, account.accountId);
  }
  const quota = (result.quota ?? []).filter(
    (row): row is QuotaRow => !!row && typeof row.id === "string" && typeof row.label === "string",
  );
  const updated: LocalAccountMeta = {
    ...account,
    plan: result.plan ?? account.plan ?? null,
    quota,
    verifiedAt: account.verifiedAt ?? new Date().toISOString(),
    modelCount: result.modelCount ?? account.modelCount ?? null,
  };
  upsertAccountMeta(updated);
  return updated;
}

/** ChatGPT account id for wham: id_token claims (3-level fallback), else "". */
function accountIdForWham(credential: OAuthCredential): string {
  if (!credential.id_token) return "";
  const parts = credential.id_token.split(".");
  if (parts.length < 2) return "";
  try {
    const payload = JSON.parse(atob(parts[1].replace(/-/g, "+").replace(/_/g, "/"))) as Record<string, unknown>;
    if (typeof payload.chatgpt_account_id === "string" && payload.chatgpt_account_id) {
      return payload.chatgpt_account_id;
    }
    const namespaced = payload["https://api.openai.com/auth"];
    if (namespaced && typeof namespaced === "object") {
      const id = (namespaced as Record<string, unknown>).chatgpt_account_id;
      if (typeof id === "string" && id) return id;
    }
    const orgs = payload.organizations;
    if (Array.isArray(orgs) && orgs.length > 0) {
      const first = orgs[0] as Record<string, unknown>;
      if (first && typeof first.id === "string" && first.id) return first.id;
    }
  } catch {
    // fall through to ""
  }
  return "";
}
