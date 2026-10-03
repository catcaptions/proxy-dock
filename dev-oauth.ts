/**
 * Dev-only OAuth helpers for the browser preview (`npm run dev`).
 *
 * The desktop (Tauri) app performs all of this in Rust with the OS keyring;
 * this middleware exists only because browsers cannot do it directly:
 *  - auth.openai.com / Google token endpoints have no CORS headers, so the
 *    page cannot exchange codes itself (same-origin middleware does it).
 *  - The Antigravity client secret must never ship in the frontend bundle,
 *    so the exchange happens here, server-side.
 *  - Command Code Studio POSTs the issued API key to a localhost callback;
 *    this middleware captures it (with the CORS headers Studio requires)
 *    and the page polls for it.
 *
 * Nothing here is used in production builds.
 */
import type { IncomingMessage, ServerResponse } from "node:http";
import http from "node:http";
import type { Plugin } from "vite";

const CODEX_TOKEN_URL = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann";
const GOOGLE_TOKEN_URL = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL = "https://www.googleapis.com/oauth2/v2/userinfo?alt=json";
// Same public client the Tauri backend uses (mirrors the agy CLI flow).
const ANTIGRAVITY_CLIENT_ID = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
// User-owned override via PROXYDOCK_ANTIGRAVITY_CLIENT_ID (else the default).
function antigravityClientId(): string {
  const raw =
    process.env.PROXYDOCK_ANTIGRAVITY_CLIENT_ID ?? process.env.PROXYHUB_ANTIGRAVITY_CLIENT_ID ?? "";
  return raw.trim() || ANTIGRAVITY_CLIENT_ID;
}
// No shipped default — set PROXYDOCK_ANTIGRAVITY_SECRET (dev) or paste it in
// Settings (desktop vault). Endpoints below 500 tersely when it is absent.
function antigravityClientSecret(): string | null {
  const raw =
    process.env.PROXYDOCK_ANTIGRAVITY_SECRET ?? process.env.PROXYHUB_ANTIGRAVITY_SECRET ?? "";
  const trimmed = raw.trim();
  return trimmed.length > 0 ? trimmed : null;
}
// Claude subscription OAuth — same endpoints the Tauri backend uses
// (mirrors CLIProxyAPI's internal/auth/claude, nothing invented).
const CLAUDE_TOKEN_URL = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_PROFILE_URL = "https://api.anthropic.com/api/oauth/profile";
const CLAUDE_USAGE_URL = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_CLIENT_ID = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_SCOPE =
  "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";

type PendingPayload = {
  apiKey: string;
  userName: string;
  keyName: string;
  receivedAt: number;
};
const pending = new Map<string, PendingPayload>();

function sweepPending() {
  const now = Date.now();
  for (const [state, payload] of pending) {
    if (now - payload.receivedAt > 10 * 60 * 1000) pending.delete(state);
  }
}

function readRaw(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

function readJson(req: IncomingMessage): Promise<unknown> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      try {
        resolve(JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}"));
      } catch (err) {
        reject(err);
      }
    });
    req.on("error", reject);
  });
}

function sendJson(res: ServerResponse, status: number, body: unknown) {
  res.statusCode = status;
  res.setHeader("Content-Type", "application/json");
  res.end(JSON.stringify(body));
}

// --- OAuth loopback capture -------------------------------------------------
// The provider redirects the browser tab straight back to localhost, exactly
// like the desktop app and the official CLIs do — so there is nothing to
// copy-paste. These extra listeners use the official callback ports/paths:
// Codex :1455 (fallback :1457, same as the Codex CLI), Antigravity :51121.
// A top-level navigation is not subject to CORS/PNA, so no special headers
// are needed here. If a port is already taken, that flow stays on manual
// paste and the page is told via /__proxydock/loopback-status.

type OAuthProvider = "codex" | "antigravity" | "claude";

type CapturedCode = {
  code?: string;
  error?: string;
  errorDescription?: string;
  receivedAt: number;
};
const capturedCodes = new Map<string, CapturedCode>();

const loopbackStatus = { codexPort: 0, antigravityPort: 0, claudePort: 0 };

function sweepCapturedCodes() {
  const now = Date.now();
  for (const [key, entry] of capturedCodes) {
    if (now - entry.receivedAt > 10 * 60 * 1000) capturedCodes.delete(key);
  }
}

function loopbackPage(ok: boolean): string {
  return (
    "<!doctype html><html><head><meta charset=\"utf-8\"><title>Proxy Dock sign-in</title></head>" +
    "<body style=\"font-family:system-ui,sans-serif;padding:40px;text-align:center\">" +
    (ok
      ? "<h1>Sign-in successful</h1><p>Return to Proxy Dock — your account is being linked.</p>"
      : "<h1>Sign-in failed</h1><p>Return to Proxy Dock for details.</p>") +
    "<script>try{window.close()}catch(e){}</script></body></html>"
  );
}

function startLoopback(provider: OAuthProvider, ports: number[], path: string, onPort: (port: number) => void) {
  const tryPort = (index: number): void => {
    if (index >= ports.length) return; // all taken — page falls back to paste
    const server = http.createServer((req, res) => {
      const url = new URL(req.url ?? "/", "http://127.0.0.1");
      if (url.pathname !== path) {
        res.statusCode = 404;
        res.end("not found");
        return;
      }
      const state = url.searchParams.get("state") ?? undefined;
      const entry: CapturedCode = {
        code: url.searchParams.get("code") ?? undefined,
        error: url.searchParams.get("error") ?? undefined,
        errorDescription: url.searchParams.get("error_description") ?? undefined,
        receivedAt: Date.now(),
      };
      if (state) {
        sweepCapturedCodes();
        capturedCodes.set(`${provider}:${state}`, entry);
      }
      res.statusCode = 200;
      res.setHeader("Content-Type", "text/html; charset=utf-8");
      res.end(loopbackPage(!entry.error && !!entry.code));
    });
    server.on("error", (err: NodeJS.ErrnoException) => {
      if (err && err.code === "EADDRINUSE") tryPort(index + 1);
      // Any other error: leave this flow unavailable, page falls back to paste.
    });
    server.listen(ports[index], "127.0.0.1", () => {
      onPort(ports[index]);
    });
  };
  tryPort(0);
}

export function proxyDockDevOAuth(): Plugin {
  return {
    name: "proxydock-dev-oauth",
    configureServer(server) {
      startLoopback("codex", [1455, 1457], "/auth/callback", (port) => {
        loopbackStatus.codexPort = port;
      });
      startLoopback("antigravity", [51121], "/oauth-callback", (port) => {
        loopbackStatus.antigravityPort = port;
      });
      // Claude's official callback (same port/path as `claude setup-token`).
      // A separate listener: no conflict with the vite-port /callback that
      // Command Code Studio uses.
      startLoopback("claude", [54545], "/callback", (port) => {
        loopbackStatus.claudePort = port;
      });
      server.middlewares.use(async (req, res, next) => {
        const url = new URL(req.url ?? "/", "http://127.0.0.1");

        // --- Loopback availability + captured-code poll for OAuth providers.
        if (url.pathname === "/__proxydock/loopback-status" && req.method === "GET") {
          sendJson(res, 200, { codexPort: loopbackStatus.codexPort, antigravityPort: loopbackStatus.antigravityPort, claudePort: loopbackStatus.claudePort });
          return;
        }

        // --- Effective Antigravity client ID (env override or default).
        if (url.pathname === "/__proxydock/antigravity-client" && req.method === "GET") {
          sendJson(res, 200, { client_id: antigravityClientId() });
          return;
        }
        if (url.pathname === "/__proxydock/oauth-pending" && req.method === "GET") {
          const provider = url.searchParams.get("provider") ?? "";
          const state = url.searchParams.get("state") ?? "";
          const entry = capturedCodes.get(`${provider}:${state}`);
          if (!entry) {
            sendJson(res, 200, { status: "waiting" });
            return;
          }
          capturedCodes.delete(`${provider}:${state}`);
          if (entry.error || !entry.code) {
            sendJson(res, 200, { status: "error", error: entry.errorDescription ?? entry.error ?? "Sign-in was not approved." });
            return;
          }
          sendJson(res, 200, { status: "ready", code: entry.code });
          return;
        }

        // --- Codex (ChatGPT) code exchange: public client, PKCE, no secret.
        // Also handles the refresh_token grant for quota refreshes.
        if (url.pathname === "/__proxydock/codex-exchange" && req.method === "POST") {
          try {
            const body = (await readJson(req)) as { code?: string; verifier?: string; redirect_uri?: string; refresh_token?: string };
            let params: URLSearchParams;
            if (typeof body.refresh_token === "string" && body.refresh_token) {
              params = new URLSearchParams({
                grant_type: "refresh_token",
                client_id: CODEX_CLIENT_ID,
                refresh_token: body.refresh_token,
              });
            } else {
              if (!body.code || !body.verifier || !body.redirect_uri) {
                sendJson(res, 400, { ok: false, error: "missing code, verifier, or redirect_uri" });
                return;
              }
              params = new URLSearchParams({
                grant_type: "authorization_code",
                client_id: CODEX_CLIENT_ID,
                code: body.code,
                redirect_uri: body.redirect_uri,
                code_verifier: body.verifier,
              });
            }
            const upstream = await fetch(CODEX_TOKEN_URL, {
              method: "POST",
              headers: { "Content-Type": "application/x-www-form-urlencoded", Accept: "application/json" },
              body: params,
            });
            const data = (await upstream.json().catch(() => ({}))) as Record<string, unknown>;
            sendJson(res, upstream.status, data);
          } catch (err) {
            sendJson(res, 502, { ok: false, error: err instanceof Error ? err.message : "exchange failed" });
          }
          return;
        }

        // --- Antigravity code exchange: secret stays server-side here.
        if (url.pathname === "/__proxydock/antigravity-exchange" && req.method === "POST") {
          try {
            const body = (await readJson(req)) as { code?: string; redirect_uri?: string };
            if (!body.code || !body.redirect_uri) {
              sendJson(res, 400, { ok: false, error: "missing code or redirect_uri" });
              return;
            }
            const exchangeSecret = antigravityClientSecret();
            if (!exchangeSecret) {
              sendJson(res, 500, { ok: false, error: "Antigravity OAuth needs PROXYDOCK_ANTIGRAVITY_SECRET" });
              return;
            }
            const upstream = await fetch(GOOGLE_TOKEN_URL, {
              method: "POST",
              headers: { "Content-Type": "application/x-www-form-urlencoded" },
              body: new URLSearchParams({
                code: body.code,
                client_id: antigravityClientId(),
                client_secret: exchangeSecret,
                redirect_uri: body.redirect_uri,
                grant_type: "authorization_code",
              }),
            });
            const tokens = (await upstream.json().catch(() => ({}))) as {
              access_token?: string;
              error?: string;
              error_description?: string;
            };
            if (!upstream.ok || !tokens.access_token) {
              sendJson(res, upstream.status, {
                ok: false,
                error: tokens.error_description ?? tokens.error ?? `token exchange failed (${upstream.status})`,
              });
              return;
            }
            let email = "";
            try {
              const infoResp = await fetch(GOOGLE_USERINFO_URL, {
                headers: { Authorization: `Bearer ${tokens.access_token}` },
              });
              if (infoResp.ok) {
                const info = (await infoResp.json()) as { email?: string };
                email = typeof info.email === "string" ? info.email : "";
              }
            } catch {
              // email stays empty; tokens are still usable
            }
            sendJson(res, 200, { ok: true, tokens, email });
          } catch (err) {
            sendJson(res, 502, { ok: false, error: err instanceof Error ? err.message : "exchange failed" });
          }
          return;
        }

        // --- Claude code exchange: public client, PKCE, no secret.
        // Mirrors the Tauri backend (CLIProxyAPI's exchange shape): ordered
        // JSON body + axios headers, then the profile lookup for the email.
        if (url.pathname === "/__proxydock/claude-exchange" && req.method === "POST") {
          try {
            const body = (await readJson(req)) as { code?: string; verifier?: string; redirect_uri?: string; state?: string };
            if (!body.code || !body.verifier || !body.redirect_uri) {
              sendJson(res, 400, { ok: false, error: "missing code, verifier, or redirect_uri" });
              return;
            }
            // A `#state` fragment appended to the callback code takes
            // precedence (same rule as the backend).
            let code = body.code;
            let state = typeof body.state === "string" ? body.state : "";
            const hash = code.indexOf("#");
            if (hash >= 0) {
              const fragment = code.slice(hash + 1);
              code = code.slice(0, hash);
              if (fragment) state = fragment;
            }
            // Field order mirrors native CLI traffic (string keys keep
            // insertion order).
            const payload: Record<string, string> = {};
            payload.grant_type = "authorization_code";
            payload.code = code;
            payload.redirect_uri = body.redirect_uri;
            payload.client_id = CLAUDE_CLIENT_ID;
            payload.code_verifier = body.verifier;
            payload.state = state;
            const upstream = await fetch(CLAUDE_TOKEN_URL, {
              method: "POST",
              headers: {
                Accept: "application/json, text/plain, */*",
                "Content-Type": "application/json",
                "User-Agent": "axios/1.15.2",
              },
              body: JSON.stringify(payload),
            });
            const tokens = (await upstream.json().catch(() => ({}))) as {
              access_token?: string;
              refresh_token?: string | null;
              error?: string;
              error_description?: string;
            };
            if (!upstream.ok || !tokens.access_token) {
              sendJson(res, upstream.status, {
                ok: false,
                error: tokens.error_description ?? tokens.error ?? `token exchange failed (${upstream.status})`,
              });
              return;
            }
            let email = "";
            let accountUuid = "";
            try {
              const profileResp = await fetch(CLAUDE_PROFILE_URL, {
                headers: {
                  Authorization: `Bearer ${tokens.access_token}`,
                  Accept: "application/json, text/plain, */*",
                  "User-Agent": "axios/1.15.2",
                  "Cache-Control": "no-cache",
                },
              });
              if (profileResp.ok) {
                const profile = (await profileResp.json()) as {
                  account?: { uuid?: string; email?: string };
                };
                email = typeof profile.account?.email === "string" ? profile.account.email : "";
                accountUuid = typeof profile.account?.uuid === "string" ? profile.account.uuid : "";
              }
            } catch {
              // email stays empty; tokens are still usable
            }
            sendJson(res, 200, { ok: true, tokens, email, accountUuid });
          } catch (err) {
            sendJson(res, 502, { ok: false, error: err instanceof Error ? err.message : "exchange failed" });
          }
          return;
        }

        // --- Claude token refresh: same token URL, refresh grant.
        if (url.pathname === "/__proxydock/claude-refresh" && req.method === "POST") {
          try {
            const body = (await readJson(req)) as { refresh_token?: string };
            if (!body.refresh_token) {
              sendJson(res, 400, { ok: false, error: "missing refresh_token" });
              return;
            }
            const upstream = await fetch(CLAUDE_TOKEN_URL, {
              method: "POST",
              headers: {
                Accept: "application/json, text/plain, */*",
                "Content-Type": "application/json",
                "User-Agent": "axios/1.15.2",
              },
              body: JSON.stringify({
                client_id: CLAUDE_CLIENT_ID,
                grant_type: "refresh_token",
                refresh_token: body.refresh_token,
                scope: CLAUDE_SCOPE,
              }),
            });
            const data = (await upstream.json().catch(() => ({}))) as Record<string, unknown>;
            sendJson(res, upstream.status, data);
          } catch (err) {
            sendJson(res, 502, { ok: false, error: err instanceof Error ? err.message : "refresh failed" });
          }
          return;
        }

        // --- Claude quota: the OAuth usage endpoint needs a claude-code
        // User-Agent (browsers forbid setting it), so the page goes through
        // here. Returns the raw upstream body for the page to map.
        if (url.pathname === "/__proxydock/claude-usage" && req.method === "POST") {
          try {
            const body = (await readJson(req)) as { accessToken?: string };
            const token = typeof body.accessToken === "string" ? body.accessToken.trim() : "";
            if (!token) {
              sendJson(res, 400, { ok: false, error: "missing accessToken" });
              return;
            }
            const upstream = await fetch(CLAUDE_USAGE_URL, {
              headers: {
                Authorization: `Bearer ${token}`,
                "anthropic-beta": "oauth-2025-04-20",
                "User-Agent": "claude-code/2.1.77",
                Accept: "application/json",
              },
            });
            if (upstream.status === 401 || upstream.status === 403) {
              sendJson(res, upstream.status, { ok: false, error: "unauthorized" });
              return;
            }
            if (upstream.status === 429) {
              sendJson(res, 429, { ok: false, error: "claude usage is rate-limited right now — try again in a few minutes" });
              return;
            }
            if (!upstream.ok) {
              sendJson(res, upstream.status, { ok: false, error: `Claude quota check failed (${upstream.status})` });
              return;
            }
            const usage = (await upstream.json().catch(() => null)) as unknown;
            sendJson(res, 200, { ok: true, usage });
          } catch (err) {
            sendJson(res, 502, { ok: false, error: err instanceof Error ? err.message : "quota fetch failed" });
          }
          return;
        }

        // --- Antigravity quota: server-side host fallback (prod -> daily ->
        // sandbox), returns raw upstream bodies for the page to map.
        // Mirrors the proven antigravity-usage tool exactly: baseUrl first,
        // `User-Agent: antigravity`, full IDE metadata. (Generic UAs and
        // metadata subsets get rejected by these internal endpoints.)
        if (url.pathname === "/__proxydock/antigravity-quota" && req.method === "POST") {
          const CC_HOSTS = [
            "https://cloudcode-pa.googleapis.com",
            "https://daily-cloudcode-pa.googleapis.com",
            "https://daily-cloudcode-pa.sandbox.googleapis.com",
          ];
          const CC_METADATA = { ideType: "ANTIGRAVITY", platform: "PLATFORM_UNSPECIFIED", pluginType: "GEMINI" };
          try {
            const body = (await readJson(req)) as { accessToken?: string };
            const token = typeof body.accessToken === "string" ? body.accessToken.trim() : "";
            if (!token) {
              sendJson(res, 400, { ok: false, error: "missing accessToken" });
              return;
            }
            const post = async (path: string, payload: unknown): Promise<{ status: number; json: unknown }> => {
              let lastStatus = 502;
              for (const host of CC_HOSTS) {
                let r: Response | null = null;
                try {
                  r = await fetch(`${host}${path}`, {
                    method: "POST",
                    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json", "User-Agent": "antigravity" },
                    body: JSON.stringify(payload),
                  });
                } catch {
                  continue;
                }
                // Auth failures are terminal (same credential everywhere);
                // anything else (incl. regional 400s) falls through to the
                // next host.
                if (r.status === 401 || r.status === 403) return { status: r.status, json: null };
                if (r.ok) return { status: r.status, json: await r.json().catch(() => null) };
                lastStatus = r.status;
                continue;
              }
              return { status: lastStatus, json: null };
            };
            const project = await post("/v1internal:loadCodeAssist", { metadata: CC_METADATA });
            const projectId =
              project.status === 200 && project.json && typeof project.json === "object"
                ? (project.json as { cloudaicompanionProject?: string }).cloudaicompanionProject ?? null
                : null;
            const payload = projectId ? { project: projectId } : {};
            const models = await post("/v1internal:fetchAvailableModels", payload);
            const summary = await post("/v1internal:retrieveUserQuotaSummary", payload);
            const statuses = [project.status, models.status, summary.status];
            const detail = `project: ${project.status}, models: ${models.status}, summary: ${summary.status}`;
            // Auth failures dominate: 401/403 means the credential itself is bad.
            if (statuses.includes(403)) {
              sendJson(res, 403, { ok: false, error: `forbidden [${detail}]` });
              return;
            }
            if (statuses.includes(401)) {
              sendJson(res, 401, { ok: false, error: `unauthorized [${detail}]` });
              return;
            }
            if (models.status !== 200 && summary.status !== 200 && project.status !== 200) {
              sendJson(res, 502, { ok: false, error: `Google quota check failed [${detail}]` });
              return;
            }
            sendJson(res, 200, { ok: true, project: project.status === 200 ? project.json : null, models: models.status === 200 ? models.json : null, summary: summary.status === 200 ? summary.json : null });
          } catch (err) {
            sendJson(res, 502, { ok: false, error: err instanceof Error ? err.message : "quota fetch failed" });
          }
          return;
        }

        // --- Google token refresh: secret stays server-side here.
        if (url.pathname === "/__proxydock/google-refresh" && req.method === "POST") {
          try {
            const body = (await readJson(req)) as { refresh_token?: string };
            if (!body.refresh_token) {
              sendJson(res, 400, { ok: false, error: "missing refresh_token" });
              return;
            }
            const refreshSecret = antigravityClientSecret();
            if (!refreshSecret) {
              sendJson(res, 500, { ok: false, error: "Antigravity OAuth needs PROXYDOCK_ANTIGRAVITY_SECRET" });
              return;
            }
            const upstream = await fetch(GOOGLE_TOKEN_URL, {
              method: "POST",
              headers: { "Content-Type": "application/x-www-form-urlencoded" },
              body: new URLSearchParams({
                refresh_token: body.refresh_token,
                client_id: antigravityClientId(),
                client_secret: refreshSecret,
                grant_type: "refresh_token",
              }),
            });
            const data = (await upstream.json().catch(() => ({}))) as Record<string, unknown>;
            sendJson(res, upstream.status, data);
          } catch (err) {
            sendJson(res, 502, { ok: false, error: err instanceof Error ? err.message : "refresh failed" });
          }
          return;
        }

        // --- Command Code Studio callback (mirrors the official CLI server).
        // The CLI serves exactly POST /callback (JSON or form bodies with
        // apiKey+state+userId+userName+keyName) and GET /callback/complete.
        const CC_ALLOWED_ORIGINS = ["http://localhost:3000", "https://staging.commandcode.ai", "https://commandcode.ai"];
        const ccCors = (origin: string | undefined, pnaRequested: boolean) => {
          res.setHeader("Access-Control-Allow-Origin", origin && CC_ALLOWED_ORIGINS.includes(origin) ? origin : CC_ALLOWED_ORIGINS[0]);
          res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
          res.setHeader("Access-Control-Allow-Headers", "Content-Type");
          if (pnaRequested) res.setHeader("Access-Control-Allow-Private-Network", "true");
        };
        if (url.pathname === "/callback" || url.pathname === "/callback/complete") {
          const origin = req.headers.origin;
          const pna = req.headers["access-control-request-private-network"] === "true";
          if (req.method === "OPTIONS") {
            res.statusCode = 204;
            ccCors(origin, pna);
            res.end();
            return;
          }
          if (url.pathname === "/callback/complete" && req.method === "GET") {
            ccCors(origin, pna);
            res.statusCode = 200;
            res.setHeader("Content-Type", "text/html; charset=utf-8");
            res.end(loopbackPage(true));
            return;
          }
          if (url.pathname === "/callback" && req.method === "POST") {
            ccCors(origin, pna);
            const contentType = (req.headers["content-type"] ?? "").split(";")[0].trim().toLowerCase();
            let fields: Record<string, string> = {};
            try {
              if (contentType === "application/x-www-form-urlencoded") {
                const raw = await readRaw(req);
                for (const [key, value] of new URLSearchParams(raw)) fields[key] = value;
              } else {
                const body = (await readJson(req)) as Record<string, unknown>;
                for (const [key, value] of Object.entries(body)) {
                  if (typeof value === "string") fields[key] = value;
                }
              }
            } catch {
              res.statusCode = 400;
              sendJson(res, 400, { success: false, error: "Invalid request body" });
              return;
            }
            if (fields.error) {
              sendJson(res, 200, { success: true });
              return;
            }
            const { apiKey = "", state = "", userName = "", keyName = "" } = fields;
            const userId = fields.userId ?? "";
            if (!apiKey.trim() || !state || !userId || !userName || !keyName) {
              res.statusCode = 400;
              sendJson(res, 400, { success: false, error: "Missing required fields" });
              return;
            }
            sweepPending();
            pending.set(state, { apiKey: apiKey.trim(), userName, keyName, receivedAt: Date.now() });
            if (contentType === "application/x-www-form-urlencoded") {
              // Browser form flow: send the tab to the landing page like the CLI.
              res.statusCode = 303;
              res.setHeader("Location", `/callback/complete?state=${encodeURIComponent(state)}`);
              res.end();
              return;
            }
            sendJson(res, 200, { success: true });
            return;
          }
          res.statusCode = url.pathname === "/callback" ? 405 : 404;
          ccCors(origin, pna);
          res.end();
          return;
        }

        // --- Command Code pending poll (page long-polls after opening Studio).
        if (url.pathname === "/__proxydock/commandcode-pending" && req.method === "GET") {
          const state = url.searchParams.get("state") ?? "";
          const payload = pending.get(state);
          if (!payload) {
            sendJson(res, 200, { status: "waiting" });
            return;
          }
          pending.delete(state);
          sendJson(res, 200, {
            status: "ready",
            payload: { apiKey: payload.apiKey, userName: payload.userName, keyName: payload.keyName },
          });
          return;
        }

        next();
      });
    },
  };
}
