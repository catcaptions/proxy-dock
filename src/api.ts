/**
 * Control API client (plan §2/§3A). All `/api/*` data calls go through the
 * internal `apiRequest`: on desktop (Tauri) it resolves the absolute gateway
 * base via the `gateway_info` IPC command (cached for the session, honoring
 * the ephemeral-port fallback) and fetches `base + path`; in the browser
 * preview it fetches the relative `path` exactly as before (the vite preview
 * proxies `/api` to the gateway, §0 gate). No secrets ever flow here — the
 * API is presence-only (keySuffix/censored identity); tokens stay in the OS
 * keyring / preview storage handled by auth.ts.
 *
 * Gateway auth: every call carries `Authorization: Bearer <Proxy Dock key>`
 * (the §0 bearer gate 401s without it). The key resolves via Tauri IPC on
 * desktop (cached in memory only, never localStorage) and from sessionStorage
 * in the browser preview (dev only). A 401 with the `proxy_dock_unauthorized`
 * envelope becomes a `GatewayKeyError` — one terse wrong-key state whose
 * re-enter affordance is the Settings key block.
 */

export type ApiError = { message: string; type?: string; code?: number };

/**
 * Marker for "the gateway rejected the Proxy Dock key" — 401 with the
 * `proxy_dock_unauthorized` envelope. Callers use `isGatewayKeyError` to
 * render the one terse wrong-key state (re-enter in Settings) instead of a
 * generic request failure.
 */
export class GatewayKeyError extends Error {
  constructor() {
    super("Wrong key — re-enter it in Settings.");
    this.name = "GatewayKeyError";
  }
}

export function isGatewayKeyError(err: unknown): boolean {
  return err instanceof GatewayKeyError;
}

/**
 * Marker for "no gateway behind this call" — direct fetch failure OR a
 * non-JSON 5xx (the vite `/api` proxy answers 500 HTML when nothing listens
 * on 11434). Callers use `isGatewayUnreachable` to decide between an honest
 * offline state / legacy fallback vs surfacing a real gateway error.
 */
export class GatewayUnreachableError extends Error {
  constructor() {
    super("Gateway unreachable — is the gateway (npm run gateway) or desktop app running?");
    this.name = "GatewayUnreachableError";
  }
}

export function isGatewayUnreachable(err: unknown): boolean {
  return err instanceof GatewayUnreachableError;
}

declare global {
  interface Window {
    __TAURI__?: unknown;
    __TAURI_INTERNALS__?: unknown;
  }
}

/** Desktop runtime check — mirrors `isTauri()` in auth.ts (kept local to
 * avoid an auth ↔ api import cycle; same two flags). */
function isTauriRuntime(): boolean {
  return (
    typeof window !== "undefined" &&
    (window.__TAURI__ !== undefined || window.__TAURI_INTERNALS__ !== undefined)
  );
}

/** Session-cached `gateway_info` base: absolute URL including the
 * ephemeral-port fallback, or null when IPC is unavailable/fails. Shared by
 * `apiRequest`, `gatewayBase`, and the quota-refresh call in auth.ts so
 * fetching and display always agree; the pending promise itself is cached
 * so concurrent calls issue one invoke. Exported for auth.ts only. */
let gatewayBasePromise: Promise<string | null> | null = null;

export function resolveGatewayBase(): Promise<string | null> {
  if (gatewayBasePromise) return gatewayBasePromise;
  gatewayBasePromise = (async () => {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const info = (await invoke("gateway_info")) as { base?: string | null };
      if (typeof info.base === "string" && info.base) return info.base;
    } catch {
      // Not in Tauri (browser preview) or IPC unavailable — fall through.
    }
    return null;
  })();
  return gatewayBasePromise;
}

async function apiRequest<T>(path: string, init?: RequestInit): Promise<T> {
  let url = path;
  if (isTauriRuntime()) {
    const base = await resolveGatewayBase();
    if (base) url = `${base}${path}`;
  }
  const auth = await gatewayAuthHeader();
  let resp: Response;
  try {
    resp = await fetch(url, {
      ...init,
      headers: { Accept: "application/json", "Content-Type": "application/json", ...auth, ...(init?.headers ?? {}) },
    });
  } catch (err: unknown) {
    if (err instanceof DOMException && err.name === "AbortError") throw err;
    throw new GatewayUnreachableError();
  }
  if (!resp.ok) {
    const data = (await resp.json().catch(() => null)) as { error?: ApiError } | null;
    if (resp.status === 401 && data?.error?.type === "proxy_dock_unauthorized") {
      // Wrong/missing Proxy Dock key. Preview drops the stored key so
      // Settings offers the Set input again; desktop drops the memory cache
      // so the next call refetches the authoritative IPC value.
      if (isTauriRuntime()) clearCachedGatewayKey();
      else clearPreviewGatewayKey();
      throw new GatewayKeyError();
    }
    const message =
      data && typeof data.error?.message === "string" && data.error.message ? data.error.message : null;
    if (message) throw new Error(message);
    // No API-shaped error body (e.g. vite proxy HTML on ECONNREFUSED):
    // the gateway isn't there, not a real gateway error.
    if (resp.status >= 500) throw new GatewayUnreachableError();
    throw new Error(`Request failed (${resp.status}).`);
  }
  return (await resp.json()) as T;
}

/**
 * In-flight GET dedup (plan §3.5): identical `since+group_by` paths share one
 * promise while pending. Aborts are per-caller (the shared fetch keeps its
 * first signal); late joiners that lose the race just ignore AbortError.
 */
const inflightGets = new Map<string, Promise<unknown>>();

export const apiGet = <T,>(path: string, init?: RequestInit): Promise<T> => {
  const key = `GET ${path}`;
  const existing = inflightGets.get(key);
  if (existing) return existing as Promise<T>;
  const p: Promise<T> = apiRequest<T>(path, init).finally(() => {
    if (inflightGets.get(key) === p) inflightGets.delete(key);
  });
  inflightGets.set(key, p);
  return p;
};
export const apiPost = <T,>(path: string, body?: unknown, init?: RequestInit): Promise<T> =>
  apiRequest<T>(path, { ...init, method: "POST", body: body === undefined ? undefined : JSON.stringify(body) });
export const apiPatch = <T,>(path: string, body: unknown, init?: RequestInit): Promise<T> =>
  apiRequest<T>(path, { ...init, method: "PATCH", body: JSON.stringify(body) });
export const apiPut = <T,>(path: string, body: unknown, init?: RequestInit): Promise<T> =>
  apiRequest<T>(path, { ...init, method: "PUT", body: JSON.stringify(body) });
export const apiDelete = <T,>(path: string, init?: RequestInit): Promise<T> =>
  apiRequest<T>(path, { ...init, method: "DELETE" });

/**
 * Proxy Dock key (gateway bearer) resolution. Desktop: Tauri IPC
 * `get_gateway_key`, cached in memory only — never localStorage, never disk.
 * Browser preview (dev only): sessionStorage, so one tab session shares the
 * pasted key without persisting it. Never throws (null when unavailable).
 */
const PREVIEW_KEY_STORE = "proxydock-gateway-key";

let gatewayKeyPromise: Promise<string | null> | null = null;

export function resolveGatewayKey(): Promise<string | null> {
  if (gatewayKeyPromise) return gatewayKeyPromise;
  gatewayKeyPromise = (async () => {
    if (isTauriRuntime()) {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const info = (await invoke("get_gateway_key")) as { key?: unknown };
        if (typeof info.key === "string" && info.key) return info.key;
      } catch {
        // IPC unavailable — unauthenticated call, the gate 401s honestly.
      }
      return null;
    }
    try {
      return sessionStorage.getItem(PREVIEW_KEY_STORE);
    } catch {
      return null;
    }
  })();
  return gatewayKeyPromise;
}

/** Drop the cached key so the next call refetches (after rotate/set/401). */
export function clearCachedGatewayKey(): void {
  gatewayKeyPromise = null;
}

/** Preview-only (dev): keep the pasted key for this tab session. */
export function setPreviewGatewayKey(key: string): void {
  clearCachedGatewayKey();
  try {
    sessionStorage.setItem(PREVIEW_KEY_STORE, key);
  } catch {
    // ignore storage errors (private mode etc.)
  }
}

/** Preview-only (dev): forget the pasted key (e.g. after a 401). */
export function clearPreviewGatewayKey(): void {
  clearCachedGatewayKey();
  try {
    sessionStorage.removeItem(PREVIEW_KEY_STORE);
  } catch {
    // ignore
  }
}

/** Bearer header for raw gateway fetches outside `apiRequest`. */
export async function gatewayAuthHeader(): Promise<Record<string, string>> {
  const key = await resolveGatewayKey();
  return key ? { Authorization: `Bearer ${key}` } : {};
}

export type ImportedCredential = {
  provider: string;
  account: string;
  email: string | null;
  plan: string | null;
  verified: boolean;
};

/**
 * Best-effort mirror of a locally saved credential into the gateway's own
 * store (OS keyring + account row + catalog warm-up). Needed because preview
 * sign-ins live in browser storage, which the gateway process cannot see
 * (it reads the OS keyring). Never throws — returns false when the gateway
 * is unreachable or rejects; the local save already succeeded, so the
 * preview keeps working from local data either way.
 */
export async function mirrorCredentialToGateway(provider: string, secret: string): Promise<boolean> {
  try {
    await apiPost<ImportedCredential>(`/api/accounts/${encodeURIComponent(provider)}/credentials`, { secret });
    return true;
  } catch {
    return false;
  }
}

/** Gateway-discovered base URL for display (Settings). Tauri IPC first
 * (authoritative incl. ephemeral-port fallback — the same session-cached
 * value the `/api/*` fetch path uses), same-origin fallback, honest unknown
 * when neither answers. */
export async function gatewayBase(): Promise<string | null> {
  if (isTauriRuntime()) {
    return resolveGatewayBase();
  }
  try {
    const resp = await fetch("/api/health");
    if (resp.ok) return window.location.origin;
  } catch {
    // Unreachable.
  }
  return null;
}

export type UsageGroup = {
  key: string;
  requests: number;
  prompt_tokens: number;
  completion_tokens: number;
  estimated_cost: number | null;
  estimated: boolean;
  /** Cache-read leg (subset of prompt_tokens). Absent on older gateways. */
  cached_tokens?: number;
  /** Cache-creation leg (subset of prompt_tokens). Absent on older gateways. */
  cache_creation_tokens?: number;
  /** What the cached leg would have cost at full input rates. Absent on older gateways. */
  cache_savings?: number;
};

export type UsageSummary = {
  requests: number;
  prompt_tokens: number;
  completion_tokens: number;
  estimated_cost: number | null;
  estimated: boolean;
  groups?: UsageGroup[];
  /** Cache-read leg (subset of prompt_tokens). Absent on older gateways. */
  cached_tokens?: number;
  /** Cache-creation leg (subset of prompt_tokens). Absent on older gateways. */
  cache_creation_tokens?: number;
  /** What the cached leg would have cost at full input rates. Absent on older gateways. */
  cache_savings?: number;
};

export type UsageEvent = {
  id: number;
  provider: string;
  account: string | null;
  model: string;
  prompt_tokens: number;
  completion_tokens: number;
  estimated_cost: number | null;
  estimated: boolean;
  created_at: string;
};

export type RoutingAccount = {
  accountId: string;
  label: string;
  priority: number;
  enabled: boolean;
  plan: string | null;
  hasToken: boolean;
  isDefault: boolean;
};

export type CatalogModel = {
  nativeId: string;
  displayName: string | null;
  endpoints: string[];
  contextLength: number | null;
  /** `live` = provider-listed; `observed` = proven by a successful request. */
  source: "live" | "observed";
};

export type CatalogResponse = {
  provider: string;
  models: CatalogModel[];
  updated_at: string | null;
  stale: boolean;
  reason: string | null;
};

export type QuotaHistoryObs = {
  observed_at: string;
  remaining: unknown;
  reset_at: string | null;
};

export type ExportBundle = {
  version: 1;
  exported_at: string;
  routing: Record<string, RoutingAccount[]>;
  pricing: { provider: string; model: string; price: { prompt_per_1k: number; completion_per_1k: number }; source: string }[];
  /** Local roster/metas (preview storage). Secrets are NEVER included —
   * only keySuffix/email/plan/quota display data. */
  accounts: { provider: string; accountId: string; email: string | null; keySuffix: string; plan: string | null }[];
};
