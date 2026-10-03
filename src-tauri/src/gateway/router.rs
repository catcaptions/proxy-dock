use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{sse::{Event, KeepAlive, Sse}, IntoResponse, Json, Response};
use axum::routing::{get, patch, post, put};
use axum::Router;
use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tokio::sync::{mpsc, Semaphore};
use tokio_stream::wrappers::ReceiverStream;

use crate::adapters::antigravity as ag;
use crate::adapters::chatgpt as cg;
use crate::adapters::claude as cl;
use crate::adapters::commandcode as cc;
use crate::adapters::opencode as oc;
use crate::adapters::{rewrite_model, serving_client as shared_client, sse_payload, usage_details, AdapterError};
use crate::gateway::split_provider_model;
use crate::{catalog, db, gateway_key, secrets, SHARED_PORT_CANDIDATE};

/// Per-account in-flight caps: commandcode (Go bridge) is subscription
/// sensitive, default covers the rest. Reject-fast with 429, no queueing.
const INFLIGHT_CAP_COMMANDCODE: usize = 2;
const INFLIGHT_CAP_DEFAULT: usize = 8;
/// Bounded SSE fan-out: `send().await` backpressures instead of dropping.
const SSE_CHANNEL_CAP: usize = 128;
/// Retention newest-wins caps (background prune, never on request path).
const RETENTION_GATEWAY_EVENTS: i64 = 10000;
const RETENTION_USAGE_EVENTS: i64 = 50000;
const RETENTION_QUOTA_OBS: i64 = 10000;
const RETENTION_PRICING: i64 = 5000;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub port: u16,
    /// Built-SPA directory for same-origin UI serving (production). `None`
    /// in dev/preview (vite serves the UI; `/api` is reached via its proxy).
    pub dist_dir: Option<PathBuf>,
    /// Providers with a catalog refresh currently running (manual
    /// `refresh=1`, boot warm, sign-in warm). A concurrent refresh for a
    /// listed provider serves cache immediately (stale) instead of
    /// stampeding upstream with duplicate live fetches on every page mount.
    pub catalog_refreshing: Arc<Mutex<HashSet<String>>>,
    /// Per-account in-flight permits `(slug, account_id) -> semaphore`.
    pub inflight: Arc<Mutex<HashMap<(String, String), Arc<Semaphore>>>>,
}

fn inflight_cap_for(slug: &str) -> usize {
    if slug == "commandcode" { INFLIGHT_CAP_COMMANDCODE } else { INFLIGHT_CAP_DEFAULT }
}

fn inflight_semaphore(state: &AppState, slug: &str, account_id: &str) -> Arc<Semaphore> {
    let key = (slug.to_string(), account_id.to_string());
    let mut map = state.inflight.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(key)
        .or_insert_with(|| Arc::new(Semaphore::new(inflight_cap_for(slug))))
        .clone()
}

/// Reject-fast permit: `None` means the account is at cap (caller 429s).
fn try_acquire_inflight(state: &AppState, slug: &str, account_id: &str) -> Option<tokio::sync::OwnedSemaphorePermit> {
    inflight_semaphore(state, slug, account_id).try_acquire_owned().ok()
}

fn account_busy_response(slug: &str) -> Response {
    let request_id = format!("req_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let body = serde_json::json!({
        "error": {"message": "account busy, retry shortly", "type": "proxy_dock_account_busy", "code": 429, "requestId": request_id, "provider": slug}
    });
    ([("retry-after", "2")], (StatusCode::TOO_MANY_REQUESTS, Json(body))).into_response()
}

fn is_sse_response(resp: &Response) -> bool {
    resp.headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.contains("text/event-stream"))
        .unwrap_or(false)
}

/// Redacted upstream snippet for logs/DB: truncated, never secrets (callers
/// only pass upstream error text, never tokens).
fn redact_snippet(text: &str) -> String {
    text.chars().take(500).collect()
}

async fn sse_send(tx: &mpsc::Sender<Result<Event, std::convert::Infallible>>, event: Event) -> bool {
    if tx.is_closed() {
        return false;
    }
    tx.send(Ok(event)).await.is_ok()
}

/// Shared bound-port slot so the UI can discover the gateway base URL
/// (the listener falls back to an ephemeral port when 11434 is busy —
/// hardcoding the candidate in the frontend would be circular).
#[derive(Clone, Default)]
pub struct GatewayPort(pub Arc<Mutex<Option<u16>>>);

/// Bound gateway address for the UI. The SPA reads this via Tauri IPC
/// (same-origin HTTP also works in production since Axum serves the UI).
#[tauri::command]
pub fn gateway_info(port: tauri::State<'_, GatewayPort>) -> serde_json::Value {
    let port = port.0.lock().ok().and_then(|p| *p);
    serde_json::json!({
        "port": port,
        "base": port.map(|p| format!("http://127.0.0.1:{p}")),
    })
}

#[derive(Serialize)]
struct Health {
    ok: bool,
    port: u16,
    version: &'static str,
}

#[derive(Serialize)]
struct ModelList {
    object: &'static str,
    data: Vec<ModelEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
    stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Serialize)]
struct ModelEntry {
    id: String,
    object: &'static str,
    owned_by: String,
}

#[derive(Serialize)]
struct ProviderInfo {
    slug: String,
    label: String,
}

#[derive(Serialize)]
struct AccountInfo {
    id: String,
    provider_slug: String,
    credential: Option<String>,
    plan: Option<String>,
    token: secrets::TokenStatus,
}

/// The client model field; full OpenAI-style bodies are accepted and passed
/// through (only `model` is validated at the gateway layer).
fn extract_model(body: &serde_json::Value) -> Option<String> {
    body.get("model").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Canonical error envelope: `{error:{message, type:<proxy_dock_*>,
/// code:<numeric HTTP status>, requestId, provider?}}` with `provider` nested
/// inside `error` (never beside it). `type` always carries the machine-readable
/// `proxy_dock_*` code; `code` always echoes the HTTP status.
fn openai_error(status: StatusCode, message: String, err_type: &str) -> axum::response::Response {
    openai_error_full(status, message, err_type, None)
}

fn openai_error_full(
    status: StatusCode,
    message: String,
    err_type: &str,
    provider: Option<&str>,
) -> axum::response::Response {
    let request_id = format!("req_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let mut error = serde_json::json!({
        "message": message,
        "type": err_type,
        "code": status.as_u16(),
        "requestId": request_id,
    });
    if let Some(slug) = provider {
        error["provider"] = serde_json::Value::String(slug.to_string());
    }
    (status, Json(serde_json::json!({"error": error}))).into_response()
}

pub fn router(state: AppState) -> Router {
    // Provider paths share one slug-capturing route table (`:slug` feeds the
    // handlers' `Path(slug)` extractor). A static `nest()` per slug was tried
    // before: it strips the prefix without capturing, so every provider
    // endpoint 500'd on extractor failure. This shape keeps native ids on
    // provider paths and `slug/native` ids on the unified paths.
    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(unified_models_handler))
        .route("/v1/chat/completions", post(unified_chat))
        .route("/v1/responses", post(unified_responses))
        .route("/v1/messages", post(unified_messages))
        .route("/:slug/v1/models", get(provider_models))
        .route("/:slug/v1/chat/completions", post(provider_chat))
        .route("/:slug/v1/responses", post(provider_responses))
        .route("/:slug/v1/messages", post(provider_messages))
        .route("/api/health", get(health))
        .route("/api/providers", get(list_providers))
        .route("/api/accounts", get(list_accounts))
        .route("/api/accounts/:provider/:id", patch(patch_account).delete(delete_account))
        .route("/api/accounts/:provider/credentials", post(import_credential))
        .route("/api/accounts/:provider/:id/quota/history", get(quota_history))
        .route("/api/accounts/:provider/:id/quota/refresh", post(quota_refresh))
        .route("/api/routing/accounts", get(routing_accounts))
        .route("/api/pricing/:provider/:model", put(put_pricing))
        .route("/api/pricing/sync", post(sync_pricing_endpoint))
        .route("/api/models", get(api_models))
        .route("/api/events", get(api_events))
        .route("/api/usage/summary", get(usage_summary))
        .route("/api/usage/events", get(usage_events));
    // Unknown slugs fall through to provider_models' honest 404
    // (proxy_dock_unknown_provider), never the SPA fallback.
    app.layer(axum::middleware::from_fn(gate_middleware))
        .fallback(spa_fallback)
        .with_state(state)
}

// ---------------------------------------------------------------------------
// §0 gate: bearer check + loopback CORS + mutating-verb origin check.
// ---------------------------------------------------------------------------

/// Paths exempt from the bearer check: liveness probes only (port + version,
/// no data). Everything else under `/api`, `/v1`, and `/:slug/v1` needs
/// `Authorization: Bearer <Proxy Dock key>`.
fn bearer_exempt(path: &str) -> bool {
    path == "/health" || path == "/api/health"
}

/// `true` when `path` is a protected gateway surface: unified `/v1/*`,
/// provider `/:slug/v1/*`, or control `/api/*`.
fn bearer_protected(path: &str) -> bool {
    if path == "/api" || path.starts_with("/api/") {
        return true;
    }
    if path == "/v1" || path.starts_with("/v1/") {
        return true;
    }
    // Provider model traffic only (`/:slug/v1/...`); unknown-slug or
    // non-model paths fall through to the honest 404s below, unauthenticated.
    if let Some(rest) = path.strip_prefix('/') {
        if let Some((slug, tail)) = rest.split_once('/') {
            if crate::PROVIDER_SLUGS.contains(&slug) && (tail == "v1" || tail.starts_with("v1/")) {
                return true;
            }
        }
    }
    false
}

fn unauthorized_response() -> Response {
    // Terse on purpose; the key itself never appears in error bodies.
    openai_error(StatusCode::UNAUTHORIZED, "wrong or missing gateway key".to_string(), "proxy_dock_unauthorized")
}

/// Origins allowed to call the gateway cross-origin: vite preview, the Tauri
/// webview, and the gateway itself (ephemeral-port fallback). Anything else
/// (e.g. a DNS-rebinding page on attacker.example) gets no CORS headers, and
/// mutating `/api` verbs from such origins are rejected outright.
fn origin_allowed(origin: &str) -> bool {
    if origin.starts_with("tauri://localhost") {
        return true;
    }
    let hostport = origin.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("");
    if hostport.is_empty() {
        return false;
    }
    // Strip a single :port suffix (hosts here are never bare-IPv6 except ::1).
    let host = if hostport.starts_with('[') {
        hostport.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    matches!(host, "localhost" | "127.0.0.1" | "[::1" | "tauri.localhost")
}

/// Loopback gate middleware: bearer check first (`Authorization: Bearer
/// <Proxy Dock key>`, timing-safe compare, fail-closed when no key is
/// configured), then CORS echo + preflight for allowlisted origins; mutating
/// `/api` verbs additionally require an absent or allowlisted Origin/Referer
/// (absent = curl / non-browser client on loopback). `/health` (both paths)
/// and OPTIONS preflight stay open. Never logs secrets.
async fn gate_middleware(req: axum::http::Request<Body>, next: Next) -> Response {
    let origin = req
        .headers()
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let referer = req
        .headers()
        .get("referer")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    if method == Method::OPTIONS {
        let mut resp = Response::new(Body::empty());
        *resp.status_mut() = StatusCode::NO_CONTENT;
        if origin_allowed(&origin) {
            resp.headers_mut().insert(
                "access-control-allow-origin",
                origin.parse().unwrap_or("*".parse().unwrap()),
            );
            resp.headers_mut().insert("access-control-allow-methods", "GET, POST, PATCH, PUT, DELETE, OPTIONS".parse().unwrap());
            resp.headers_mut().insert(
                "access-control-allow-headers",
                "Content-Type, Authorization, X-Client-Request-Id".parse().unwrap(),
            );
            resp.headers_mut().insert("access-control-max-age", "600".parse().unwrap());
            resp.headers_mut().insert("vary", "Origin".parse().unwrap());
        }
        return resp;
    }

    let mutating = matches!(method, Method::POST | Method::PATCH | Method::PUT | Method::DELETE) && path.starts_with("/api/");
    // Bearer check runs before the origin gate (which stays as
    // defense-in-depth behind it) but after OPTIONS preflight, which is
    // unauthenticated by design.
    if bearer_protected(&path) && !bearer_exempt(&path) {
        let authorized = gateway_key::bearer_authorized(req.headers(), gateway_key::read_effective().as_deref());
        if !authorized {
            tracing::warn!(path = %path, "rejected gateway call with wrong or missing key");
            return unauthorized_response();
        }
    }
    if mutating {
        // Referer carries the page origin on most navigations; check both.
        let foreign = (!origin.is_empty() && !origin_allowed(&origin))
            || (!referer.is_empty() && !origin_allowed(&referer));
        if foreign {
            tracing::warn!(path = %path, "rejected cross-origin mutating /api call");
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({"error": {"message": "cross-origin mutating calls are not allowed", "type": "proxy_dock_forbidden_origin", "code": 403}})),
            )
                .into_response();
        }
    }

    let mut resp = next.run(req).await;
    if origin_allowed(&origin) {
        resp.headers_mut().insert(
            "access-control-allow-origin",
            origin.parse().unwrap_or("*".parse().unwrap()),
        );
        resp.headers_mut().insert("vary", "Origin".parse().unwrap());
    }
    resp
}

/// Same-origin SPA fallback (production): serves the bundled `dist/` files so
/// the UI and the Control API share one origin (no CORS needed). API, model,
/// and health paths never fall through here — unknown ones get JSON 404s.
async fn spa_fallback(State(state): State<AppState>, req: axum::http::Request<Body>) -> Response {
    let path = req.uri().path().to_string();
    if path == "/api" || path.starts_with("/api/") || path == "/v1" || path.starts_with("/v1/") || path == "/health" {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": {"message": format!("unknown path '{path}'"), "type": "proxy_dock_unknown_path", "code": 404}})),
        )
            .into_response();
    }
    for slug in crate::PROVIDER_SLUGS {
        if path == format!("/{slug}") || path.starts_with(&format!("/{slug}/")) {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": {"message": format!("unknown path '{path}'"), "type": "proxy_dock_unknown_path", "code": 404}})),
            )
                .into_response();
        }
    }
    let Some(dist) = state.dist_dir.clone() else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": {"message": "no UI bundled (dev mode) — open the vite preview instead", "type": "proxy_dock_no_ui", "code": 404}})),
        )
            .into_response();
    };
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return (StatusCode::METHOD_NOT_ALLOWED, Json(serde_json::json!({"error": {"message": "method not allowed", "type": "proxy_dock_method_not_allowed", "code": 405}}))).into_response();
    }
    let rel = path.trim_start_matches('/');
    let candidate = dist.join(rel);
    // No traversal escapes: the resolved file must stay under dist/.
    let file_ok = candidate.is_file()
        && candidate
            .canonicalize()
            .ok()
            .zip(dist.canonicalize().ok())
            .map(|(f, d)| f.starts_with(&d))
            .unwrap_or(false);
    let target = if file_ok { candidate } else { dist.join("index.html") };
    match tokio::fs::read(&target).await {
        Ok(bytes) => {
            let ctype = content_type_for(target.extension().and_then(|e| e.to_str()).unwrap_or(""));
            ([("content-type", ctype)], bytes).into_response()
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": {"message": format!("unknown path '{path}'"), "type": "proxy_dock_unknown_path", "code": 404}})),
        )
            .into_response(),
    }
}

fn content_type_for(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

pub async fn serve(app: AppHandle) -> Result<(), std::io::Error> {
    let pool = db::pool(&app).await.map_err(std::io::Error::other)?;
    app.manage(pool.clone());
    let listener = bind_with_fallback(SHARED_PORT_CANDIDATE).await?;
    let port = listener.local_addr()?.port();
    // Publish the bound port (candidate or ephemeral fallback) for the UI —
    // hardcoding the candidate in the frontend would be circular.
    if let Some(slot) = app.try_state::<GatewayPort>() {
        if let Ok(mut guard) = slot.0.lock() {
            *guard = Some(port);
        }
    }
    // Production UI bundle: Tauri resources first, dev `dist/` fallback.
    let dist_dir = app
        .path()
        .resource_dir()
        .ok()
        .filter(|d| d.join("index.html").is_file())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|c| c.join("dist"))
                .filter(|d| d.join("index.html").is_file())
        });
    tracing::info!(port, has_ui = dist_dir.is_some(), "proxy-dock gateway listening");
    serve_on_listener(pool, listener, port, dist_dir).await
}

/// Shared serve path for the desktop app and the headless gateway binary:
/// retention task + Axum on an already-bound listener.
pub async fn serve_on_listener(
    pool: SqlitePool,
    listener: tokio::net::TcpListener,
    port: u16,
    dist_dir: Option<std::path::PathBuf>,
) -> Result<(), std::io::Error> {
    // Unified Proxy Dock key: generate-on-first-run so the bearer gate below
    // is never fail-open-by-accident. Best-effort (a broken vault fails
    // closed per-request with 401, never with an unauthenticated gateway).
    if let Err(err) = gateway_key::ensure() {
        tracing::error!(error = %err, "gateway key unavailable — protected routes will 401 until the vault works");
    }
    // Retention: newest-wins per table + 30d age bound (OR); background,
    // never on the request path.
    let prune_pool = pool.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let _ = sqlx::query(
                "DELETE FROM gateway_events WHERE id NOT IN (SELECT id FROM gateway_events ORDER BY id DESC LIMIT ?) OR created_at <= datetime('now', '-30 days')",
            )
            .bind(RETENTION_GATEWAY_EVENTS)
            .execute(&prune_pool)
            .await;
            let _ = sqlx::query(
                "DELETE FROM usage_events WHERE id NOT IN (SELECT id FROM usage_events ORDER BY id DESC LIMIT ?)",
            )
            .bind(RETENTION_USAGE_EVENTS)
            .execute(&prune_pool)
            .await;
            let _ = sqlx::query(
                "DELETE FROM quota_observations WHERE id NOT IN (SELECT id FROM quota_observations ORDER BY id DESC LIMIT ?)",
            )
            .bind(RETENTION_QUOTA_OBS)
            .execute(&prune_pool)
            .await;
            let _ = sqlx::query(
                "DELETE FROM pricing_snapshots WHERE id NOT IN (SELECT id FROM pricing_snapshots ORDER BY id DESC LIMIT ?)",
            )
            .bind(RETENTION_PRICING)
            .execute(&prune_pool)
            .await;
        }
    });
    // Pricing sync (background): fill missing per-model rates from Models.dev
    // so costs show up; manual/official snapshots always win. Off the request
    // path entirely — failures only log.
    crate::pricing::spawn_sync(&pool);
    // Catalog boot-warm (background, once per process): fill EMPTY caches for
    // linked accounts so first UI paint doesn't serialize behind live
    // upstream chains (one multi-round-trip fetch per provider otherwise).
    // Non-empty caches are left alone; manual Refresh is unaffected.
    {
        let warm_pool = pool.clone();
        tokio::spawn(async move {
            for slug in crate::PROVIDER_SLUGS {
                let cached = catalog::read_cache(&warm_pool, slug).await;
                if !cached.0.is_empty() {
                    continue;
                }
                let candidates = candidate_accounts(&warm_pool, slug).await;
                let Some(cand) = candidates.iter().find(|c| secrets::has_local_token(slug, &c.id)) else {
                    continue;
                };
                if catalog::refresh_catalog(&warm_pool, slug, &cand.id).await.is_ok() {
                    crate::pricing::spawn_sync(&warm_pool);
                }
            }
        });
    }
    let state = AppState { pool, port, dist_dir, catalog_refreshing: Default::default(), inflight: Default::default() };
    axum::serve(listener, router(state)).await
}

/// Bind the loopback candidate, falling back to an ephemeral port when busy.
/// Public for the headless gateway binary (same package, separate crate).
pub async fn bind_with_fallback(candidate: u16) -> Result<tokio::net::TcpListener, std::io::Error> {
    match tokio::net::TcpListener::bind(("127.0.0.1", candidate)).await {
        Ok(listener) => Ok(listener),
        Err(err) => {
            tracing::warn!(port = candidate, error = %err, "preferred port busy, binding ephemeral port");
            tokio::net::TcpListener::bind(("127.0.0.1", 0)).await
        }
    }
}

async fn health(State(state): State<AppState>) -> Json<Health> {
    Json(Health { ok: true, port: state.port, version: env!("CARGO_PKG_VERSION") })
}

/// Unified catalog from `model_cache` (live 1A fetches, never
/// `BUILTIN_MODELS`). Empty only with an honest `reason`.
async fn unified_models_handler(State(state): State<AppState>) -> Json<ModelList> {
    let mut data = Vec::new();
    let mut newest: Option<String> = None;
    let mut any_stale = true;
    for slug in crate::gateway::PROVIDER_ORDER {
        // Self-healing catalog (same rule as the provider endpoint).
        let mut cached = catalog::read_cache(&state.pool, slug).await;
        if cached.0.is_empty() {
            let candidates = candidate_accounts(&state.pool, slug).await;
            if let Some(cand) = candidates.iter().find(|c| secrets::has_local_token(slug, &c.id)) {
                if catalog::refresh_catalog(&state.pool, slug, &cand.id).await.is_ok() {
                    cached = catalog::read_cache(&state.pool, slug).await;
                }
            }
        }
        let (entries, updated_at, stale) = cached;
        if !entries.is_empty() {
            any_stale = any_stale && stale;
        }
        if updated_at.as_ref().map(|u| newest.as_ref().map(|n| u > n).unwrap_or(true)).unwrap_or(false) {
            newest = updated_at;
        }
        for entry in entries {
            data.push(ModelEntry {
                owned_by: slug.to_string(),
                id: format!("{slug}/{}", entry.native_id),
                object: "model",
            });
        }
    }
    let stale = data.is_empty() || any_stale;
    let reason = if data.is_empty() {
        Some("No models yet — sign in first; the catalog refreshes automatically once a credential exists.".to_string())
    } else {
        None
    };
    Json(ModelList { object: "list", data, updated_at: newest, stale, reason })
}

/// Provider catalog from `model_cache` (native ids, no provider prefix).
async fn provider_models(State(state): State<AppState>, Path(slug): Path<String>) -> impl IntoResponse {
    if !crate::PROVIDER_SLUGS.contains(&slug.as_str()) {
        return openai_error_full(
            StatusCode::NOT_FOUND,
            format!("unknown provider '{slug}'"),
            "proxy_dock_unknown_provider",
            None,
        );
    }
    // Self-healing catalog: empty cache + saved credential → live refresh
    // inline (bounded by the 30s upstream timeouts). Stale caches still
    // serve immediately with stale:true; refresh stays manual.
    let mut cached = catalog::read_cache(&state.pool, &slug).await;
    if cached.0.is_empty() {
        let candidates = candidate_accounts(&state.pool, &slug).await;
        if let Some(cand) = candidates.iter().find(|c| secrets::has_local_token(&slug, &c.id)) {
            if catalog::refresh_catalog(&state.pool, &slug, &cand.id).await.is_ok() {
                cached = catalog::read_cache(&state.pool, &slug).await;
            }
        }
    }
    let (entries, updated_at, stale) = cached;
    let data = entries
        .into_iter()
        .map(|entry| ModelEntry { owned_by: slug.clone(), id: entry.native_id, object: "model" })
        .collect::<Vec<_>>();
    let reason = if data.is_empty() {
        let candidates = candidate_accounts(&state.pool, &slug).await;
        let has_token = candidates.iter().any(|c| secrets::has_local_token(&slug, &c.id));
        Some(if has_token {
            "No models yet — the live fetch just failed. Try again shortly.".to_string()
        } else {
            "No models yet — sign in first.".to_string()
        })
    } else {
        None
    };
    (
        StatusCode::OK,
        Json(ModelList { object: "list", data, updated_at, stale, reason }),
    )
        .into_response()
}

/// Client request id (`X-Client-Request-Id`), truncated for logs. Never a
/// secret; echoed in tracing + the events table only.
fn client_request_id(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get("x-client-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(64).collect::<String>())
        .filter(|s| !s.is_empty())
}

async fn unified_chat(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(model) = extract_model(&body) else {
        return openai_error(StatusCode::BAD_REQUEST, "model must be '<provider-slug>/<native-model-id>'".to_string(), "proxy_dock_bad_model");
    };
    let Some((slug, native)) = split_provider_model(&model) else {
        return openai_error(StatusCode::BAD_REQUEST, "model must be '<provider-slug>/<native-model-id>'".to_string(), "proxy_dock_bad_model");
    };
    if let Err(resp) = require_chat_messages(&body) {
        return resp;
    }
    let req_id = client_request_id(&headers);
    let (slug_o, native_o, echo_o, body_o) = (slug.to_string(), native.to_string(), model.clone(), body.clone());
    let st = state.clone();
    serve_with_event_log(&state, slug_o.clone(), native_o.clone(), "chat/completions", echo_o.clone(), req_id, || async move {
        route_chat(&st, &slug_o, &native_o, &body_o, &echo_o).await
    })
    .await
}

async fn provider_chat(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(model) = extract_model(&body) else {
        return openai_error(StatusCode::BAD_REQUEST, "chat request must include a model".to_string(), "proxy_dock_bad_model");
    };
    if let Err(resp) = require_chat_messages(&body) {
        return resp;
    }
    // Provider endpoints use native model IDs (no provider prefix).
    let req_id = client_request_id(&headers);
    let (slug_o, model_o, body_o) = (slug, model, body);
    let st = state.clone();
    serve_with_event_log(&state, slug_o.clone(), model_o.clone(), "chat/completions", model_o.clone(), req_id, || async move {
        route_chat(&st, &slug_o, &model_o, &body_o, &model_o).await
    })
    .await
}

/// Chat-route validation (1C): `messages` must be a non-empty array. Unknown
/// or empty models are rejected before any account is touched.
fn require_chat_messages(body: &serde_json::Value) -> Result<(), axum::response::Response> {
    match body.get("messages").and_then(|v| v.as_array()) {
        Some(items) if !items.is_empty() => Ok(()),
        _ => Err(openai_error(
            StatusCode::BAD_REQUEST,
            "chat request must include a non-empty messages array".to_string(),
            "proxy_dock_bad_model",
        )),
    }
}

/// Native wires (1B): `/v1/responses`, `/v1/messages` and the provider-path
/// equivalents. Bodies pass through untouched on the serving wire; models
/// sent to the wrong wire get an honest 400 naming the right one.
async fn unified_responses(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    serve_native_request(state, headers, body, "responses").await
}

async fn unified_messages(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    serve_native_request(state, headers, body, "messages").await
}

async fn provider_responses(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    serve_native_request_for(state, slug, headers, body, "responses").await
}

async fn provider_messages(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    serve_native_request_for(state, slug, headers, body, "messages").await
}

async fn serve_native_request(
    state: AppState,
    headers: axum::http::HeaderMap,
    body: serde_json::Value,
    wire: &'static str,
) -> axum::response::Response {
    let Some(model) = extract_model(&body) else {
        return openai_error(StatusCode::BAD_REQUEST, "request must include a model".to_string(), "proxy_dock_bad_model");
    };
    let Some((slug, native)) = split_provider_model(&model) else {
        return openai_error(StatusCode::BAD_REQUEST, "model must be '<provider-slug>/<native-model-id>'".to_string(), "proxy_dock_bad_model");
    };
    let req_id = client_request_id(&headers);
    let (slug_o, native_o, echo_o, body_o) = (slug.to_string(), native.to_string(), model.clone(), body.clone());
    let st = state.clone();
    serve_with_event_log(&state, slug_o.clone(), native_o.clone(), wire, echo_o.clone(), req_id, || async move {
        route_native(&st, &slug_o, &native_o, wire, &body_o, &echo_o).await
    })
    .await
}

async fn serve_native_request_for(
    state: AppState,
    slug: String,
    headers: axum::http::HeaderMap,
    body: serde_json::Value,
    wire: &'static str,
) -> axum::response::Response {
    let Some(model) = extract_model(&body) else {
        return openai_error(StatusCode::BAD_REQUEST, "request must include a model".to_string(), "proxy_dock_bad_model");
    };
    let req_id = client_request_id(&headers);
    // Provider endpoints use native model IDs (no provider prefix).
    let (slug_o, model_o, body_o) = (slug, model, body);
    let st = state.clone();
    serve_with_event_log(&state, slug_o.clone(), model_o.clone(), wire, model_o.clone(), req_id, || async move {
        route_native(&st, &slug_o, &model_o, wire, &body_o, &model_o).await
    })
    .await
}

/// Shared send path with cross-plan account rolling: candidates are tried in
/// routing-policy order; each account uses its own plan's transport
/// (Command Code Go rides the conversion bridge, other Command Code plans
/// ride the Provider API, OpenCode Go rides Zen, ChatGPT rides Codex
/// Responses, Antigravity rides generateContent, Claude rides Messages).
/// Retryable pre-stream failures (429/5xx) roll to the next account;
/// failures (429/5xx) roll to the next account; auth/validation failures
/// stop immediately.
async fn route_chat(
    state: &AppState,
    slug: &str,
    native_model: &str,
    body: &serde_json::Value,
    echo_model: &str,
) -> axum::response::Response {
    if !crate::PROVIDER_SLUGS.contains(&slug) {
        return openai_error(StatusCode::NOT_FOUND, format!("unknown provider '{slug}'"), "proxy_dock_unknown_provider");
    }
    let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let candidates = candidate_accounts(&state.pool, slug).await;
    let mut failed: u32 = 0;
    let mut skipped: u32 = 0;
    let mut busy: u32 = 0;
    let mut first_err: Option<AdapterError> = None;
    for cand in &candidates {
        // Reject-fast per-account cap, no queueing.
        let permit = match try_acquire_inflight(state, slug, &cand.id) {
            Some(p) => p,
            None => {
                busy += 1;
                continue;
            }
        };
        match attempt_candidate(state, slug, cand, native_model, body, echo_model, stream, Some(permit)).await {
            Attempt::Done(resp) => return resp,
            Attempt::Next(err) => match err {
                Some(err) => {
                    failed += 1;
                    if first_err.is_none() {
                        first_err = Some(err);
                    }
                }
                None => {
                    skipped += 1;
                }
            },
            Attempt::Stop(resp) => return resp,
        }
    }
    if failed > 0 || skipped > 0 {
        return all_failed_response(slug, failed, skipped, busy, first_err);
    }
    if busy > 0 {
        return account_busy_response(slug);
    }
    gateway_not_ready(&state.pool, slug).await
}

fn all_failed_response(
    slug: &str,
    failed: u32,
    skipped: u32,
    busy: u32,
    first_err: Option<AdapterError>,
) -> axum::response::Response {
    let suffix = if busy > 0 { format!(", {busy} busy") } else { String::new() };
    // Thought-only bridge empties surface as an honest rollable 502.
    if let Some(err) = &first_err {
        if err.message.contains("upstream returned no content") {
            let message = format!("all {failed} failed, {skipped} skipped{suffix} on {slug}; last: {}", redact_snippet(&err.message));
            return openai_error_full(StatusCode::BAD_GATEWAY, message, "proxy_dock_bridge_empty", Some(slug));
        }
    }
    let first_retry_after = first_err.as_ref().and_then(|err| err.retry_after);
    let (status, message) = match first_err {
        Some(err) if err.status == 429 => (
            StatusCode::TOO_MANY_REQUESTS,
            format!("all {failed} failed, {skipped} skipped{suffix} on {slug}; last: {}", redact_snippet(&err.message)),
        ),
        Some(err) => (
            StatusCode::BAD_GATEWAY,
            format!("all {failed} failed, {skipped} skipped{suffix} on {slug}; last: {}", redact_snippet(&err.message)),
        ),
        None => (
            StatusCode::BAD_GATEWAY,
            format!("all {skipped} skipped{suffix} on {slug} (no routable plan)"),
        ),
    };
    // Forward a Retry-After on 429s: prefer the upstream value captured in
    // the adapter error, else a terse default so callers back off.
    if status == StatusCode::TOO_MANY_REQUESTS {
        let request_id = format!("req_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let body = serde_json::json!({
            "error": {"message": message, "type": "proxy_dock_all_accounts_failed", "code": 429, "requestId": request_id, "provider": slug}
        });
        let retry_after = first_retry_after.map(|s| s.to_string()).unwrap_or_else(|| "2".to_string());
        return ([("retry-after", retry_after)], (status, Json(body))).into_response();
    }
    openai_error_full(status, message, "proxy_dock_all_accounts_failed", Some(slug))
}

/// Request wrapper (1C/2F): measures latency, resolves the default account
/// for attribution, runs the send, then records one `gateway_events` row and
/// a structured trace line. Account ids are hash-prefixed in logs (never raw
/// emails); error bodies are NOT captured here — `outcome`/`status`/`latency`
/// power the activity panels, per-request messages stay upstream snippets.
/// Streams (`text/event-stream`) skip the headers-time log: the stream task
/// logs the completion outcome (`ok` vs `truncated`) with a redacted snippet.
async fn serve_with_event_log<F, Fut>(
    state: &AppState,
    slug: String,
    native_model: String,
    route: &'static str,
    echo_model: String,
    client_req_id: Option<String>,
    run: F,
) -> axum::response::Response
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = axum::response::Response>,
{
    let start = std::time::Instant::now();
    let account = default_account_for(&state.pool, &slug).await;
    let resp = run().await;
    if is_sse_response(&resp) {
        tracing::info!(
            slug = %slug,
            model = %echo_model,
            route = %route,
            status = 200,
            account = %account_prefix(account.as_deref()),
            client_req = %client_req_id.as_deref().unwrap_or("-"),
            "gateway stream started"
        );
        return resp;
    }
    let status = resp.status().as_u16();
    let latency_ms = start.elapsed().as_millis() as i64;
    let outcome = if status < 400 { "ok" } else { "error" };
    log_event(
        &state.pool,
        &slug,
        account.as_deref(),
        &native_model,
        route,
        outcome,
        status,
        latency_ms,
        None,
    )
    .await;
    // Proven-servable catalog: successful serves record the model as
    // `observed`, so providers without a listable catalog (ChatGPT/Codex)
    // still surface models that actually work. Failures never record.
    if outcome == "ok" {
        if let Some(wire) = wire_for_route(route) {
            catalog::note_observed(&state.pool, &slug, &native_model, wire).await;
        }
    }
    tracing::info!(
        slug = %slug,
        model = %echo_model,
        route = %route,
        status = status,
        latency_ms = latency_ms,
        account = %account_prefix(account.as_deref()),
        client_req = %client_req_id.as_deref().unwrap_or("-"),
        outcome = %outcome,
        "gateway request"
    );
    resp
}

/// Serving route -> catalog wire for observed-model provenance.
fn wire_for_route(route: &str) -> Option<&'static str> {
    match route {
        "chat/completions" => Some("chat"),
        "responses" => Some("responses"),
        "messages" => Some("messages"),
        _ => None,
    }
}

/// Hash-prefix for account attribution in logs: first 8 hex of sha256, never
/// the raw email/key id.
fn account_prefix(account: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    match account {
        Some(id) if !id.is_empty() => {
            let mut hasher = Sha256::new();
            hasher.update(id.as_bytes());
            let digest = hasher.finalize();
            digest[..4].iter().map(|b| format!("{b:02x}")).collect()
        }
        _ => "-".to_string(),
    }
}

async fn log_event(
    pool: &SqlitePool,
    slug: &str,
    account_id: Option<&str>,
    model: &str,
    route: &str,
    outcome: &str,
    status: u16,
    latency_ms: i64,
    error_snippet: Option<&str>,
) {
    let snippet = error_snippet.map(|s| s.chars().take(500).collect::<String>());
    let _ = sqlx::query(
        "INSERT INTO gateway_events (provider_slug, account_id, model, route, outcome, status, latency_ms, error_snippet) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(slug)
    .bind(account_id.unwrap_or(""))
    .bind(model)
    .bind(route)
    .bind(outcome)
    .bind(status as i64)
    .bind(latency_ms)
    .bind(snippet)
    .execute(pool)
    .await;
}

/// Native-wire send path (1B): same rolling as chat, but the body goes to the
/// serving wire untouched. Go-plan Command Code keys have no Provider API, so
/// they get an honest 400 naming the bridge wire instead of a pointless
/// upstream 403. Antigravity has no native wires (chat translation only).
async fn route_native(
    state: &AppState,
    slug: &str,
    native_model: &str,
    wire: &str,
    body: &serde_json::Value,
    echo_model: &str,
) -> axum::response::Response {
    if !crate::PROVIDER_SLUGS.contains(&slug) {
        return openai_error(StatusCode::NOT_FOUND, format!("unknown provider '{slug}'"), "proxy_dock_unknown_provider");
    }
    if wire != "responses" && wire != "messages" {
        return openai_error(StatusCode::NOT_FOUND, format!("unknown wire '{wire}'"), "proxy_dock_unknown_path");
    }
    let stream = body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
    let candidates = candidate_accounts(&state.pool, slug).await;
    let mut failed: u32 = 0;
    let mut skipped: u32 = 0;
    let mut busy: u32 = 0;
    let mut first_err: Option<AdapterError> = None;
    for cand in &candidates {
        let _permit = match try_acquire_inflight(state, slug, &cand.id) {
            Some(p) => p,
            None => {
                busy += 1;
                continue;
            }
        };
        match native_attempt(state, slug, cand, native_model, wire, body, echo_model, stream).await {
            Attempt::Done(resp) => return resp,
            Attempt::Next(err) => match err {
                Some(err) => {
                    failed += 1;
                    if first_err.is_none() {
                        first_err = Some(err);
                    }
                }
                None => {
                    skipped += 1;
                }
            },
            Attempt::Stop(resp) => return resp,
        }
    }
    if failed > 0 || skipped > 0 {
        return all_failed_response(slug, failed, skipped, busy, first_err);
    }
    if busy > 0 {
        return account_busy_response(slug);
    }
    gateway_not_ready(&state.pool, slug).await
}

async fn native_attempt(
    state: &AppState,
    slug: &str,
    cand: &Candidate,
    native_model: &str,
    wire: &str,
    body: &serde_json::Value,
    echo_model: &str,
    stream: bool,
) -> Attempt {
    let secret = match secrets::read_raw_token_async(slug.to_string(), cand.id.clone()).await {
        Ok(secret) if !secret.trim().is_empty() && !cc::is_placeholder_token(&secret) => secret,
        _ => return Attempt::Next(None),
    };
    let path = if wire == "responses" { "/responses" } else { "/messages" };
    match slug {
        "commandcode" => {
            if cc::is_go_plan(cand.plan.as_deref()) {
                return Attempt::Stop(openai_error(
                    StatusCode::BAD_REQUEST,
                    format!("Go-plan keys serve chat via the /alpha/generate bridge only — {wire} has no Go transport (use /v1/chat/completions)"),
                    "proxy_dock_wrong_wire",
                ));
            }
            if !cc::is_provider_routable(cand.plan.as_deref()) {
                return Attempt::Next(None);
            }
            native_passthrough(state, slug, cand, &secret, "commandcode", path, native_model, body, echo_model, stream, "commandcode_provider_error").await
        }
        "opencode" => {
            if !oc::is_servable_plan(cand.plan.as_deref()) {
                return Attempt::Next(None);
            }
            native_passthrough(state, slug, cand, &secret, "opencode", path, native_model, body, echo_model, stream, "opencode_upstream_error").await
        }
        "chatgpt" => {
            if wire != "responses" {
                return Attempt::Stop(openai_error(
                    StatusCode::BAD_REQUEST,
                    "ChatGPT Codex serves the Responses wire only — use /v1/responses".to_string(),
                    "proxy_dock_wrong_wire",
                ));
            }
            let Some(cred) = cg::parse_serving_cred(&secret) else {
                return Attempt::Next(None);
            };
            native_passthrough_codex(state, slug, cand, &secret, &cred, native_model, body, echo_model, stream).await
        }
        "antigravity" => Attempt::Stop(openai_error(
            StatusCode::BAD_REQUEST,
            "Antigravity serves chat via the generateContent translation only — use /v1/chat/completions".to_string(),
            "proxy_dock_wrong_wire",
        )),
        "claude" => {
            if wire != "messages" {
                return Attempt::Stop(openai_error(
                    StatusCode::BAD_REQUEST,
                    "Claude serves the Messages wire only — use /v1/messages".to_string(),
                    "proxy_dock_wrong_wire",
                ));
            }
            let Some(cred) = cl::parse_serving_cred(&secret) else {
                return Attempt::Next(None);
            };
            native_passthrough_claude(state, slug, cand, &secret, &cred, native_model, body, echo_model, stream).await
        }
        _ => Attempt::Stop(adapter_pending(slug)),
    }
}

/// Generic native passthrough for Bearer-key providers (opencode,
/// commandcode): raw body to `{base}{path}`, usage mapped from the
/// Responses/Messages `input_tokens`/`output_tokens` shape.
async fn native_passthrough(
    state: &AppState,
    slug: &str,
    cand: &Candidate,
    secret: &str,
    provider: &str,
    path: &str,
    native_model: &str,
    body: &serde_json::Value,
    echo_model: &str,
    stream: bool,
    err_type: &str,
) -> Attempt {
    let client = shared_client();
    let result = if provider == "opencode" {
        oc::post_native(&client, secret, path, body, stream).await
    } else {
        cc::post_provider_native(&client, secret, path, body, stream).await
    };
    match result {
        Err(err) if err.retryable => Attempt::Next(Some(err)),
        Err(err) => Attempt::Stop(adapter_error_response(&err, err_type)),
        Ok(upstream) => {
            if stream {
                let route: &'static str = if path == "/responses" { "responses" } else { "messages" };
                Attempt::Done(passthrough_stream_response_with_route(state, slug, &cand.id, native_model, echo_model, upstream, route))
            } else {
                Attempt::Done(native_json_response(state, slug, &cand.id, native_model, echo_model, upstream).await)
            }
        }
    }
}

/// Native Responses passthrough for Codex (chatgpt), with one OAuth refresh
/// on 401. Usage comes from the Responses `usage` block when present.
async fn native_passthrough_codex(
    state: &AppState,
    slug: &str,
    cand: &Candidate,
    secret: &str,
    cred: &cg::ServingCred,
    native_model: &str,
    body: &serde_json::Value,
    echo_model: &str,
    stream: bool,
) -> Attempt {
    let client = cg::serving_client();
    let upstream = match cg::post_responses_raw(&client, cred, body, stream).await {
        Ok(resp) => resp,
        Err(err) if err.status == 401 => {
            let Some(refresh) = cred.refresh_token.clone().filter(|r| !r.is_empty()) else {
                return Attempt::Stop(adapter_error_response(&err, "codex_upstream_error"));
            };
            let (access, rotated) = match cg::refresh_access_token(&refresh).await {
                Ok(tokens) => tokens,
                Err(_) => return Attempt::Stop(adapter_error_response(&err, "codex_upstream_error")),
            };
            persist_refreshed_oauth(slug, &cand.id, secret, &access, rotated.as_deref()).await;
            let retried = cg::ServingCred { access_token: access, account_id: cred.account_id.clone(), refresh_token: rotated.or(cred.refresh_token.clone()) };
            match cg::post_responses_raw(&client, &retried, body, stream).await {
                Ok(resp) => resp,
                Err(err2) if err2.retryable => return Attempt::Next(Some(err2)),
                Err(err2) => return Attempt::Stop(adapter_error_response(&err2, "codex_upstream_error")),
            }
        }
        Err(err) if err.retryable => return Attempt::Next(Some(err)),
        Err(err) => return Attempt::Stop(adapter_error_response(&err, "codex_upstream_error")),
    };
    if stream {
        Attempt::Done(passthrough_stream_response_with_route(state, slug, &cand.id, native_model, echo_model, upstream, "responses"))
    } else {
        Attempt::Done(native_json_response(state, slug, &cand.id, native_model, echo_model, upstream).await)
    }
}

/// Non-streaming native response: parse upstream JSON, rewrite `model` to the
/// gateway echo id, record usage. The upstream `usage` object passes through
/// untouched — `record_usage` normalizes every known shape (including cache
/// legs), so nothing is dropped here.
async fn native_json_response(
    state: &AppState,
    slug: &str,
    account_id: &str,
    native_model: &str,
    echo_model: &str,
    upstream: reqwest::Response,
) -> axum::response::Response {
    let mut body: serde_json::Value = match upstream.json().await {
        Ok(body) => body,
        Err(err) => return openai_error(StatusCode::BAD_GATEWAY, format!("upstream parse failed: {err}"), "proxy_dock_upstream_parse"),
    };
    let usage = body.get("usage").cloned().unwrap_or_else(|| serde_json::json!({"prompt_tokens": 0, "completion_tokens": 0}));
    // Only OpenAI-shaped bodies (choices) carry a top-level `model`.
    // Native Messages/Responses shapes have none — don't invent one.
    if body.get("choices").is_some() {
        if let Some(obj) = body.as_object_mut() {
            obj.insert("model".to_string(), serde_json::Value::String(echo_model.to_string()));
        }
    }
    record_usage(&state.pool, slug, account_id, native_model, &usage).await;
    (StatusCode::OK, Json(body)).into_response()
}

/// One account's attempt: success, roll-to-next, or terminal.
enum Attempt {
    Done(axum::response::Response),
    Next(Option<AdapterError>),
    Stop(axum::response::Response),
}

fn to_adapter(err: cc::BridgeError) -> AdapterError {
    AdapterError { message: err.message, status: err.status, retryable: err.retryable, retry_after: None }
}

#[derive(Debug, Clone)]
struct Candidate {
    id: String,
    plan: Option<String>,
}

/// Canonical rolling order, shared by the gateway and `GET
/// /api/routing/accounts`: routing-policy priority first, then account
/// priority; lower number wins; ties break on account id (email sort), so the
/// default (first row) is deterministic. Disabled policy rows are excluded.
/// Secrets are never read here — only the keyring presence check in the
/// caller decides eligibility.
pub const CANDIDATE_ORDER_BY: &str = "COALESCE(r.priority, a.priority, 1), a.id";

/// Rolling order: routing-policy priority first, then account priority.
/// Disabled policy rows are excluded. Secrets are never read here — only the
/// keyring presence check in the caller decides eligibility. Unverified
/// pastes (migration 011 `verified = 0`) are excluded until a quota refresh
/// validates them; pre-011 DBs without the column keep the old behavior.
async fn candidate_accounts(pool: &SqlitePool, slug: &str) -> Vec<Candidate> {
    let filtered = sqlx::query_as(&format!(
        "SELECT a.id, a.plan FROM accounts a LEFT JOIN routing_policy r ON r.provider_slug = a.provider_slug AND r.account_id = a.id WHERE a.provider_slug = ? AND COALESCE(r.enabled, 1) = 1 AND COALESCE(a.verified, 1) = 1 ORDER BY {CANDIDATE_ORDER_BY}"
    ))
    .bind(slug)
    .fetch_all(pool)
    .await;
    let rows: Vec<(String, Option<String>)> = match filtered {
        Ok(rows) => rows,
        Err(_) => sqlx::query_as(&format!(
            "SELECT a.id, a.plan FROM accounts a LEFT JOIN routing_policy r ON r.provider_slug = a.provider_slug AND r.account_id = a.id WHERE a.provider_slug = ? AND COALESCE(r.enabled, 1) = 1 ORDER BY {CANDIDATE_ORDER_BY}"
        ))
        .bind(slug)
        .fetch_all(pool)
        .await
        .unwrap_or_default(),
    };
    rows.into_iter().map(|(id, plan)| Candidate { id, plan }).collect()
}

async fn attempt_candidate(
    state: &AppState,
    slug: &str,
    cand: &Candidate,
    native_model: &str,
    body: &serde_json::Value,
    echo_model: &str,
    stream: bool,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> Attempt {
    // Read-once per candidate, off the runtime via spawn_blocking.
    let secret = match secrets::read_raw_token_async(slug.to_string(), cand.id.clone()).await {
        Ok(secret) if !secret.trim().is_empty() && !cc::is_placeholder_token(&secret) => secret,
        // Missing or placeholder credentials are never forwarded upstream.
        _ => return Attempt::Next(None),
    };
    match slug {
        "commandcode" => {
            // Go is the special case: CLI-style `/alpha/generate` bridge.
            // Everything else rides the documented Provider API.
            if cc::has_unsupported_parts(body) {
                return Attempt::Stop(openai_error(
                    StatusCode::BAD_REQUEST,
                    "commandcode serving supports text messages only — image/file parts have no verified mapping".to_string(),
                    "proxy_dock_unsupported_content",
                ));
            }
            if cc::is_go_plan(cand.plan.as_deref()) {
                if !cc::go_bridge_enabled() {
                    return Attempt::Stop(openai_error(
                        StatusCode::NOT_IMPLEMENTED,
                        "Command Code Go bridge is disabled (PROXYDOCK_GO_BRIDGE=0)".to_string(),
                        "proxy_dock_adapter_disabled",
                    ));
                }
                return bridge_attempt(state, slug, cand, &secret, native_model, body, echo_model, stream, permit).await;
            }
            if !cc::is_provider_routable(cand.plan.as_deref()) {
                // Free/empty plans have no serving transport — skip and roll.
                return Attempt::Next(None);
            }
            provider_attempt(state, slug, cand, &secret, native_model, body, echo_model, stream).await
        }
        "opencode" => {
            if !oc::is_servable_plan(cand.plan.as_deref()) {
                // Valid key without a Go subscription — no serving transport.
                return Attempt::Next(None);
            }
            opencode_attempt(state, slug, cand, &secret, native_model, body, echo_model, stream).await
        }
        "chatgpt" => codex_attempt(state, slug, cand, &secret, native_model, body, echo_model, stream).await,
        "antigravity" => antigravity_attempt(state, slug, cand, &secret, native_model, body, echo_model, stream).await,
        "claude" => claude_attempt(state, slug, cand, &secret, native_model, body, echo_model, stream).await,
        _ => Attempt::Stop(adapter_pending(slug)),
    }
}

fn adapter_pending(slug: &str) -> axum::response::Response {
    openai_error(
        StatusCode::NOT_IMPLEMENTED,
        secrets::not_configured_response(slug)
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("adapter is not configured")
            .to_string(),
        "proxy_dock_not_configured",
    )
}

/// One Go-leg attempt over the conversion bridge.
async fn bridge_attempt(
    state: &AppState,
    slug: &str,
    cand: &Candidate,
    secret: &str,
    native_model: &str,
    body: &serde_json::Value,
    echo_model: &str,
    stream: bool,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> Attempt {
    let ctx = cc::AlphaContext::local();
    let thread_id = uuid::Uuid::new_v4().to_string();
    let mut request = body.clone();
    if let Some(obj) = request.as_object_mut() {
        obj.insert("model".to_string(), serde_json::Value::String(native_model.to_string()));
    }
    let payload = cc::build_alpha_payload(&request, &ctx, &thread_id);
    let effective_max = payload.get("params").and_then(|p| p.get("max_tokens")).and_then(|v| v.as_u64()).unwrap_or(0);
    tracing::trace!(max_tokens = effective_max, "bridge attempt");
    let client = cc::bridge_client();
    if stream {
        // Pre-stream: only HTTP status is known. Retryable failures roll;
        // terminal ones stop. Mid-stream failures terminate the stream
        // (mirrors the Python bridge — a started stream can't roll).
        match cc::open_alpha_stream(&client, secret, &payload, &thread_id, &ctx.working_dir).await {
            Err(err) if err.retryable => Attempt::Next(Some(to_adapter(err))),
            Err(err) => Attempt::Stop(bridge_error_response(&err)),
            Ok(upstream) => bridge_stream_response(state, slug, &cand.id, native_model, echo_model, upstream, permit).await,
        }
    } else {
        match cc::generate_once(&client, secret, &payload, &thread_id, &ctx.working_dir).await {
            Ok(result) => {
                let completion_id = cc::new_completion_id();
                let created = cc::unix_now();
                match result.to_openai(&completion_id, echo_model, created) {
                    Ok(value) => {
                        record_usage(&state.pool, slug, &cand.id, native_model, &result.usage).await;
                        Attempt::Done((StatusCode::OK, Json(value)).into_response())
                    }
                    Err(empty) => Attempt::Next(Some(to_adapter(empty))),
                }
            }
            Err(err) if err.retryable => Attempt::Next(Some(to_adapter(err))),
            Err(err) => Attempt::Stop(bridge_error_response(&err)),
        }
    }
}

/// One Provider-API attempt (Command Code non-Go plans): OpenAI passthrough.
async fn provider_attempt(
    state: &AppState,
    slug: &str,
    cand: &Candidate,
    secret: &str,
    native_model: &str,
    body: &serde_json::Value,
    echo_model: &str,
    stream: bool,
) -> Attempt {
    let client = shared_client();
    match cc::post_provider_chat(&client, secret, native_model, body, stream).await {
        Err(err) if err.retryable => Attempt::Next(Some(err)),
        Err(err) => Attempt::Stop(adapter_error_response(&err, "commandcode_provider_error")),
        Ok(upstream) => {
            if stream {
                Attempt::Done(passthrough_stream_response(state, slug, &cand.id, native_model, echo_model, upstream))
            } else {
                Attempt::Done(passthrough_json_response(state, slug, &cand.id, native_model, echo_model, upstream).await)
            }
        }
    }
}

/// One OpenCode Zen Go attempt: OpenAI passthrough.
async fn opencode_attempt(
    state: &AppState,
    slug: &str,
    cand: &Candidate,
    secret: &str,
    native_model: &str,
    body: &serde_json::Value,
    echo_model: &str,
    stream: bool,
) -> Attempt {
    let client = shared_client();
    match oc::post_chat(&client, secret, native_model, body, stream).await {
        Err(err) if err.retryable => Attempt::Next(Some(err)),
        Err(err) => Attempt::Stop(adapter_error_response(&err, "opencode_upstream_error")),
        Ok(upstream) => {
            if stream {
                Attempt::Done(passthrough_stream_response(state, slug, &cand.id, native_model, echo_model, upstream))
            } else {
                Attempt::Done(passthrough_json_response(state, slug, &cand.id, native_model, echo_model, upstream).await)
            }
        }
    }
}

/// One Codex attempt (ChatGPT OAuth): chat -> Responses -> chat, with one
/// OAuth refresh on 401 before giving up on this account.
async fn codex_attempt(
    state: &AppState,
    slug: &str,
    cand: &Candidate,
    secret: &str,
    native_model: &str,
    body: &serde_json::Value,
    echo_model: &str,
    stream: bool,
) -> Attempt {
    let Some(cred) = cg::parse_serving_cred(secret) else {
        return Attempt::Next(None);
    };
    let client = cg::serving_client();
    let first = cg::post_responses(&client, &cred, native_model, body, stream).await;
    let upstream = match first {
        Ok(resp) => resp,
        Err(err) if err.status == 401 => {
            let Some(refresh) = cred.refresh_token.clone().filter(|r| !r.is_empty()) else {
                return Attempt::Stop(adapter_error_response(&err, "codex_upstream_error"));
            };
            let (access, rotated) = match cg::refresh_access_token(&refresh).await {
                Ok(tokens) => tokens,
                Err(_) => return Attempt::Stop(adapter_error_response(&err, "codex_upstream_error")),
            };
            persist_refreshed_oauth(slug, &cand.id, secret, &access, rotated.as_deref()).await;
            let retried = cg::ServingCred { access_token: access, account_id: cred.account_id.clone(), refresh_token: rotated.or(cred.refresh_token.clone()) };
            match cg::post_responses(&client, &retried, native_model, body, stream).await {
                Ok(resp) => resp,
                Err(err2) if err2.retryable => return Attempt::Next(Some(err2)),
                Err(err2) => return Attempt::Stop(adapter_error_response(&err2, "codex_upstream_error")),
            }
        }
        Err(err) if err.retryable => return Attempt::Next(Some(err)),
        Err(err) => return Attempt::Stop(adapter_error_response(&err, "codex_upstream_error")),
    };
    if stream {
        Attempt::Done(codex_stream_response(state, slug, &cand.id, native_model, echo_model, upstream))
    } else {
        Attempt::Done(codex_json_response(state, slug, &cand.id, native_model, echo_model, upstream).await)
    }
}

/// One Antigravity attempt (Google OAuth): project discovery + generate, with
/// one OAuth refresh on 401 before giving up on this account.
async fn antigravity_attempt(
    state: &AppState,
    slug: &str,
    cand: &Candidate,
    secret: &str,
    native_model: &str,
    body: &serde_json::Value,
    echo_model: &str,
    stream: bool,
) -> Attempt {
    let Some(cred) = ag::parse_serving_cred(secret) else {
        return Attempt::Next(None);
    };
    // Image/file parts have no proven Gemini mapping: honest 400, never
    // silently dropped (see ag::has_unsupported_parts).
    if ag::has_unsupported_parts(body) {
        return Attempt::Stop(openai_error(
            StatusCode::BAD_REQUEST,
            "antigravity serving supports text messages only — image/file parts have no verified mapping".to_string(),
            "proxy_dock_unsupported_content",
        ));
    }
    let client = ag::serving_client();
    let project = match ag::discover_project(&client, &cred.access_token).await {
        Ok(project) => project,
        Err(err) if err.status == 401 => {
            let Some(refresh) = cred.refresh_token.clone().filter(|r| !r.is_empty()) else {
                return Attempt::Stop(adapter_error_response(&err, "antigravity_upstream_error"));
            };
            let (access, rotated) = match ag::refresh_access_token(&refresh).await {
                Ok(tokens) => tokens,
                Err(_) => return Attempt::Stop(adapter_error_response(&err, "antigravity_upstream_error")),
            };
            persist_refreshed_oauth(slug, &cand.id, secret, &access, rotated.as_deref()).await;
            match ag::discover_project(&client, &access).await {
                Ok(project) => {
                    match ag::post_generate(&client, &access, &project, native_model, body, stream).await {
                        Ok(resp) => {
                            return if stream {
                                Attempt::Done(antigravity_stream_response(state, slug, &cand.id, native_model, echo_model, resp))
                            } else {
                                Attempt::Done(antigravity_json_response(state, slug, &cand.id, native_model, echo_model, resp).await)
                            };
                        }
                        Err(err2) if err2.retryable => return Attempt::Next(Some(err2)),
                        Err(err2) => return Attempt::Stop(adapter_error_response(&err2, "antigravity_upstream_error")),
                    }
                }
                Err(err2) if err2.retryable => return Attempt::Next(Some(err2)),
                Err(err2) => return Attempt::Stop(adapter_error_response(&err2, "antigravity_upstream_error")),
            }
        }
        Err(err) if err.retryable => return Attempt::Next(Some(err)),
        Err(err) => return Attempt::Stop(adapter_error_response(&err, "antigravity_upstream_error")),
    };
    let upstream = match ag::post_generate(&client, &cred.access_token, &project, native_model, body, stream).await {
        Ok(resp) => resp,
        Err(err) if err.status == 401 => {
            let Some(refresh) = cred.refresh_token.clone().filter(|r| !r.is_empty()) else {
                return Attempt::Stop(adapter_error_response(&err, "antigravity_upstream_error"));
            };
            let (access, rotated) = match ag::refresh_access_token(&refresh).await {
                Ok(tokens) => tokens,
                Err(_) => return Attempt::Stop(adapter_error_response(&err, "antigravity_upstream_error")),
            };
            persist_refreshed_oauth(slug, &cand.id, secret, &access, rotated.as_deref()).await;
            let project = ag::discover_project(&client, &access).await.unwrap_or(project);
            match ag::post_generate(&client, &access, &project, native_model, body, stream).await {
                Ok(resp) => resp,
                Err(err2) if err2.retryable => return Attempt::Next(Some(err2)),
                Err(err2) => return Attempt::Stop(adapter_error_response(&err2, "antigravity_upstream_error")),
            }
        }
        Err(err) if err.retryable => return Attempt::Next(Some(err)),
        Err(err) => return Attempt::Stop(adapter_error_response(&err, "antigravity_upstream_error")),
    };
    if stream {
        Attempt::Done(antigravity_stream_response(state, slug, &cand.id, native_model, echo_model, upstream))
    } else {
        Attempt::Done(antigravity_json_response(state, slug, &cand.id, native_model, echo_model, upstream).await)
    }
}

/// One Claude attempt (API key or OAuth subscription): chat -> Messages ->
/// chat, with one OAuth refresh on 401 before giving up on this account.
/// Text-only v1: image/file parts get an honest 400 (see
/// `cl::has_unsupported_parts`).
async fn claude_attempt(
    state: &AppState,
    slug: &str,
    cand: &Candidate,
    secret: &str,
    native_model: &str,
    body: &serde_json::Value,
    echo_model: &str,
    stream: bool,
) -> Attempt {
    let Some(cred) = cl::parse_serving_cred(secret) else {
        return Attempt::Next(None);
    };
    if cl::has_unsupported_parts(body) {
        return Attempt::Stop(openai_error(
            StatusCode::BAD_REQUEST,
            "claude serving supports text messages only — image/file parts have no verified mapping".to_string(),
            "proxy_dock_unsupported_content",
        ));
    }
    let client = cl::serving_client();
    let first = cl::post_messages(&client, &cred, native_model, body, stream).await;
    let upstream = match first {
        Ok(resp) => resp,
        Err(err) if err.status == 401 && cred.kind == cl::CredKind::OAuth => {
            let Some(refresh) = cred.refresh_token.clone().filter(|r| !r.is_empty()) else {
                return Attempt::Stop(adapter_error_response(&err, "claude_upstream_error"));
            };
            let (access, rotated) = match cl::refresh_access_token(&refresh).await {
                Ok(tokens) => tokens,
                Err(_) => return Attempt::Stop(adapter_error_response(&err, "claude_upstream_error")),
            };
            persist_refreshed_oauth(slug, &cand.id, secret, &access, rotated.as_deref()).await;
            let retried = cl::ServingCred { access_token: access, refresh_token: rotated.or(cred.refresh_token.clone()), kind: cl::CredKind::OAuth };
            match cl::post_messages(&client, &retried, native_model, body, stream).await {
                Ok(resp) => resp,
                Err(err2) if err2.retryable => return Attempt::Next(Some(err2)),
                Err(err2) => return Attempt::Stop(adapter_error_response(&err2, "claude_upstream_error")),
            }
        }
        Err(err) if err.retryable => return Attempt::Next(Some(err)),
        Err(err) => return Attempt::Stop(adapter_error_response(&err, "claude_upstream_error")),
    };
    if stream {
        Attempt::Done(claude_stream_response(state, slug, &cand.id, native_model, echo_model, upstream))
    } else {
        Attempt::Done(claude_json_response(state, slug, &cand.id, native_model, echo_model, upstream).await)
    }
}

/// Native Messages passthrough for Claude, with one OAuth refresh on 401.
/// Usage comes from the Messages `usage` block when present.
async fn native_passthrough_claude(
    state: &AppState,
    slug: &str,
    cand: &Candidate,
    secret: &str,
    cred: &cl::ServingCred,
    native_model: &str,
    body: &serde_json::Value,
    echo_model: &str,
    stream: bool,
) -> Attempt {
    let client = cl::serving_client();
    let upstream = match cl::post_messages_raw(&client, cred, body, stream).await {
        Ok(resp) => resp,
        Err(err) if err.status == 401 && cred.kind == cl::CredKind::OAuth => {
            let Some(refresh) = cred.refresh_token.clone().filter(|r| !r.is_empty()) else {
                return Attempt::Stop(adapter_error_response(&err, "claude_upstream_error"));
            };
            let (access, rotated) = match cl::refresh_access_token(&refresh).await {
                Ok(tokens) => tokens,
                Err(_) => return Attempt::Stop(adapter_error_response(&err, "claude_upstream_error")),
            };
            persist_refreshed_oauth(slug, &cand.id, secret, &access, rotated.as_deref()).await;
            let retried = cl::ServingCred { access_token: access, refresh_token: rotated.or(cred.refresh_token.clone()), kind: cl::CredKind::OAuth };
            match cl::post_messages_raw(&client, &retried, body, stream).await {
                Ok(resp) => resp,
                Err(err2) if err2.retryable => return Attempt::Next(Some(err2)),
                Err(err2) => return Attempt::Stop(adapter_error_response(&err2, "claude_upstream_error")),
            }
        }
        Err(err) if err.retryable => return Attempt::Next(Some(err)),
        Err(err) => return Attempt::Stop(adapter_error_response(&err, "claude_upstream_error")),
    };
    if stream {
        Attempt::Done(passthrough_stream_response_with_route(state, slug, &cand.id, native_model, echo_model, upstream, "messages"))
    } else {
        Attempt::Done(native_json_response(state, slug, &cand.id, native_model, echo_model, upstream).await)
    }
}

fn adapter_error_response(err: &AdapterError, err_type: &str) -> axum::response::Response {
    let status = StatusCode::from_u16(err.status).unwrap_or(StatusCode::BAD_GATEWAY);
    openai_error(status, err.message.clone(), err_type)
}

/// Persist a refreshed OAuth credential (access + rotated refresh) back to
/// the keyring without touching routing metadata. Failures are best-effort:
/// the current request already has a working token.
async fn persist_refreshed_oauth(slug: &str, account_id: &str, old_secret: &str, access_token: &str, refresh_token: Option<&str>) {
    // Route through the shared merge so id_token bytes are dropped on
    // rotation instead of accumulating in the vault.
    let payload = crate::oauth::refreshed_payload(old_secret, access_token, refresh_token);
    let Some(payload) = payload else { return };
    let _ = secrets::store_raw_token(slug, account_id, &payload);
}

fn bridge_error_response(err: &cc::BridgeError) -> axum::response::Response {
    let status = StatusCode::from_u16(err.status).unwrap_or(StatusCode::BAD_GATEWAY);
    openai_error(status, err.message.clone(), "commandcode_bridge_error")
}

/// Persist gateway-observed token usage. Missing upstream usage maps to zero
/// (mirrors the bridge). `prompt_tokens` stays TOTAL input; the cache legs
/// (`cached_tokens`, `cache_creation_tokens`) are subsets of it, normalized
/// from every known upstream shape. `estimated_cost` is filled from pricing
/// when a rate exists, else NULL (NULL = unknown, never $0.00 presented as
/// fact). Costs are always estimates unless reconciled. `cost_source` records
/// whether the rate came from an exact (provider, model) row or a bare-slug
/// fallback so later exact prices can overwrite fallback-filled rows.
async fn record_usage(pool: &SqlitePool, provider_slug: &str, account_id: &str, model: &str, usage: &serde_json::Value) {
    let (prompt, completion, cached, creation) = usage_details(Some(usage));
    let (prompt, completion, cached, creation) = (prompt as i64, completion as i64, cached as i64, creation as i64);
    let (priced, source) = estimated_cost_for(pool, provider_slug, model, prompt, completion, cached, creation).await;
    let (cost, savings) = priced.map(|(c, s)| (Some(c), s)).unwrap_or((None, 0.0));
    let _ = sqlx::query(
        "INSERT INTO usage_events (provider_slug, account_id, model, prompt_tokens, completion_tokens, cached_tokens, cache_creation_tokens, estimated_cost, cache_savings, cost_source) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(provider_slug)
    .bind(account_id)
    .bind(model)
    .bind(prompt)
    .bind(completion)
    .bind(cached)
    .bind(creation)
    .bind(cost)
    .bind(savings)
    .bind(source)
    .execute(pool)
    .await;
}

/// Pricing lookup for a served model. An exact (provider, model) snapshot
/// wins; otherwise the bare model slug (after the last `/`, case-insensitive)
/// matches the latest snapshot across providers — the same model served via
/// antigravity or commandcode is the same pricing. Returns the (cost,
/// savings) pair with its provenance (`exact` | `bare`), or `(None, None)`
/// when unpriced.
/// `price_json` shape: `{"prompt_per_1k": f64, "completion_per_1k": f64}` plus
/// optional `cache_read_per_1k` / `cache_write_per_1k`; absent cache rates
/// fall back to the input rate (t3code's `?? input`), so costs match the old
/// two-rate math exactly until real cache rates land in snapshots.
async fn estimated_cost_for(
    pool: &SqlitePool,
    provider_slug: &str,
    model: &str,
    prompt: i64,
    completion: i64,
    cached: i64,
    creation: i64,
) -> (Option<(f64, f64)>, Option<String>) {
    if let Some(priced) = priced_cost(pool, provider_slug, model, prompt, completion, cached, creation, true).await {
        return (Some(priced), Some("exact".to_string()));
    }
    if let Some(priced) = priced_cost(pool, provider_slug, model, prompt, completion, cached, creation, false).await {
        return (Some(priced), Some("bare".to_string()));
    }
    (None, None)
}

async fn priced_cost(
    pool: &SqlitePool,
    provider_slug: &str,
    model: &str,
    prompt: i64,
    completion: i64,
    cached: i64,
    creation: i64,
    exact: bool,
) -> Option<(f64, f64)> {
    let row: Option<(String,)> = if exact {
        sqlx::query_as(
            "SELECT price_json FROM pricing_snapshots WHERE provider_slug = ? AND model = ? ORDER BY id DESC LIMIT 1",
        )
        .bind(provider_slug)
        .bind(model)
        .fetch_optional(pool)
        .await
        .unwrap_or(None)
    } else {
        // Ranked: official/manual sources win over recency (see
        // pricing::BARE_RANK_ORDER_BY).
        sqlx::query_as(
            &format!(
                "SELECT price_json FROM pricing_snapshots WHERE lower(substr(model, instr(model, '/') + 1)) = lower(substr(?1, instr(?1, '/') + 1)) ORDER BY {} LIMIT 1",
                crate::pricing::BARE_RANK_ORDER_BY
            ),
        )
        .bind(model)
        .fetch_optional(pool)
        .await
        .unwrap_or(None)
    };
    let json = row.map(|(j,)| j)?;
    let parsed: serde_json::Value = serde_json::from_str(&json).ok()?;
    let prompt_rate = parsed.get("prompt_per_1k").and_then(|v| v.as_f64())?;
    let completion_rate = parsed.get("completion_per_1k").and_then(|v| v.as_f64())?;
    if !prompt_rate.is_finite() || !completion_rate.is_finite() || prompt_rate < 0.0 || completion_rate < 0.0 {
        return None;
    }
    // Optional cache legs; invalid values fall back to the input rate.
    let valid_rate = |key: &str| {
        parsed
            .get(key)
            .and_then(|v| v.as_f64())
            .filter(|r| r.is_finite() && *r >= 0.0)
            .unwrap_or(prompt_rate)
    };
    let read_rate = valid_rate("cache_read_per_1k");
    let write_rate = valid_rate("cache_write_per_1k");
    let uncached = (prompt - cached - creation).max(0) as f64;
    let cost = uncached / 1000.0 * prompt_rate
        + cached as f64 / 1000.0 * read_rate
        + creation as f64 / 1000.0 * write_rate
        + completion as f64 / 1000.0 * completion_rate;
    let savings = (cached as f64 / 1000.0 * (prompt_rate - read_rate)).max(0.0);
    Some((cost, savings))
}

/// Normalize a client time bound (RFC-3339, space-separated UTC, or epoch
/// millis/seconds) to UTC `'YYYY-MM-DD HH:MM:SS'` for bound-param comparison
/// against TEXT columns. Returns `None` when unparseable (callers 400 rather
/// than mis-filtering). Epoch values below 1_000_000_000 (pre-2001-09-09) are
/// rejected as unparseable — send RFC-3339 instead.
pub fn normalize_bound(raw: &str) -> Option<String> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    // Epoch seconds / millis.
    if let Ok(num) = text.parse::<i64>() {
        let ms = if num > 1_000_000_000_000 { num } else if num > 1_000_000_000 { num * 1000 } else { return None };
        return chrono::DateTime::from_timestamp_millis(ms)
            .map(|dt| dt.with_timezone(&chrono::Utc).format("%Y-%m-%d %H:%M:%S").to_string());
    }
    // RFC-3339 (handles Z and numeric offsets).
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(dt.with_timezone(&chrono::Utc).format("%Y-%m-%d %H:%M:%S").to_string());
    }
    // Already space-separated UTC: validate shape strictly.
    if text.len() == 19
        && chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").is_ok()
    {
        return Some(text.to_string());
    }
    None
}

/// Read upstream until the first parsed SSE event. Preamble lines are
/// drained; remaining bytes stay in `buffer` for the forward task. EOF,
/// transport errors, preamble overflow, and stalls map to retryable 502s so
/// the caller rolls/retries instead of hanging on a dead stream.
async fn peek_first_event(
    upstream: &mut reqwest::Response,
    buffer: &mut Vec<u8>,
) -> Result<cc::CcEvent, cc::BridgeError> {
    const PEEK_TIMEOUT_SECS: u64 = 180;
    const PEEK_MAX_BYTES: usize = 1024 * 1024;
    let mut seen: usize = 0;
    let read = async {
        loop {
            while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=pos).collect();
                seen += line.len();
                let text = String::from_utf8_lossy(&line);
                if let Some(event) = cc::parse_cc_line(&text) {
                    return Ok(event);
                }
                if seen > PEEK_MAX_BYTES {
                    return Err(cc::BridgeError::retryable("upstream preamble too large", 502));
                }
            }
            match upstream.chunk().await {
                Ok(Some(bytes)) => buffer.extend_from_slice(&bytes),
                Ok(None) => {
                    return Err(cc::BridgeError::retryable("upstream closed before first event", 502));
                }
                Err(err) => {
                    return Err(cc::BridgeError::retryable(format!("upstream read failed: {err}"), 502));
                }
            }
        }
    };
    match tokio::time::timeout(std::time::Duration::from_secs(PEEK_TIMEOUT_SECS), read).await {
        Ok(out) => out,
        Err(_) => Err(cc::BridgeError::retryable("upstream first event timeout", 502)),
    }
}

/// SSE streaming response: peeks at the first upstream event BEFORE
/// committing to 200, so pre-content failures surface as rollable HTTP errors
/// (the caller retries and the user still gets a response) instead of a dead
/// stream. Post-content errors emit as content + finish (content delivered).
async fn bridge_stream_response(
    state: &AppState,
    slug: &str,
    account_id: &str,
    native_model: &str,
    echo_model: &str,
    mut upstream: reqwest::Response,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> Attempt {
    let completion_id = cc::new_completion_id();
    let created = cc::unix_now();
    let model = echo_model.to_string();
    let pool = state.pool.clone();
    let slug_o = slug.to_string();
    let account_o = account_id.to_string();
    let native_o = native_model.to_string();
    let start = std::time::Instant::now();
    let mut buffer: Vec<u8> = Vec::new();
    let first = match peek_first_event(&mut upstream, &mut buffer).await {
        Err(err) if err.retryable => return Attempt::Next(Some(to_adapter(err))),
        Err(err) => return Attempt::Stop(bridge_error_response(&err)),
        Ok(event) => event,
    };
    if let Some(err) = cc::bridge_inline_error(&first) {
        return if err.retryable {
            Attempt::Next(Some(to_adapter(err)))
        } else {
            Attempt::Stop(bridge_error_response(&err))
        };
    }
    let mut acc = cc::CompletionResult {
        finish_reason: "stop".to_string(),
        usage: cc::map_usage_to_openai(0, 0),
        ..Default::default()
    };
    let mut sse_state = cc::SseState::default();
    let first_lines = match cc::translate_stream_event(&completion_id, &model, created, &first, &mut sse_state, &mut acc) {
        Err(err) => {
            return if err.retryable {
                Attempt::Next(Some(to_adapter(err)))
            } else {
                Attempt::Stop(bridge_error_response(&err))
            };
        }
        Ok(lines) => lines,
    };
    let (tx, rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(SSE_CHANNEL_CAP);
    tokio::spawn(async move {
        let _permit = permit;
        if !sse_send(&tx, Event::default().data(cc::sse_role_chunk(&completion_id, &model, created))).await {
            return;
        }
        let mut content_started = false;
        for line in first_lines {
            if !sse_send(&tx, Event::default().data(line)).await {
                return;
            }
            content_started = true;
        }
        let mut err_snippet: Option<String> = None;
        // Emit finish + DONE, record usage, log completion outcome.
        async fn finish_and_record(
            tx: &mpsc::Sender<Result<Event, std::convert::Infallible>>,
            pool: &SqlitePool,
            slug: &str,
            account_id: &str,
            native_model: &str,
            completion_id: &str,
            model: &str,
            created: u64,
            acc: &cc::CompletionResult,
            err_snippet: Option<String>,
            content_started: bool,
            latency_ms: i64,
        ) {
            sse_send(tx, Event::default().data(cc::sse_finish_chunk(completion_id, model, created, acc))).await;
            sse_send(tx, Event::default().data("[DONE]")).await;
            record_usage(pool, slug, account_id, native_model, &acc.usage).await;
            let outcome = if err_snippet.is_some() || !content_started { "truncated" } else { "ok" };
            log_event(pool, slug, Some(account_id), native_model, "chat/completions", outcome, 200, latency_ms, err_snippet.as_deref()).await;
            tracing::info!(slug = %slug, outcome = %outcome, latency_ms = latency_ms, "gateway stream done");
        }
        loop {
            if tx.is_closed() {
                return;
            }
            let chunk = match upstream.chunk().await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => break,
                Err(err) => {
                    err_snippet = Some(redact_snippet(&format!("stream read failed: {err}")));
                    break;
                }
            };
            buffer.extend_from_slice(&chunk);
            while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&line);
                let Some(event) = cc::parse_cc_line(&text) else { continue };
                if let Some(err) = cc::bridge_inline_error(&event) {
                    let snippet = redact_snippet(&err.message);
                    let detail = serde_json::json!({
                        "id": completion_id, "object": "chat.completion.chunk", "created": created, "model": model,
                        "choices": [{"index": 0, "delta": {"content": format!("[upstream error] {}", err.message)}, "finish_reason": null}],
                    });
                    sse_send(&tx, Event::default().data(detail.to_string())).await;
                    err_snippet = Some(snippet);
                    let latency_ms = start.elapsed().as_millis() as i64;
                    finish_and_record(&tx, &pool, &slug_o, &account_o, &native_o, &completion_id, &model, created, &acc, err_snippet, true, latency_ms).await;
                    return;
                }
                match cc::translate_stream_event(&completion_id, &model, created, &event, &mut sse_state, &mut acc) {
                    Err(err) => {
                        let snippet = redact_snippet(&err.message);
                        let detail = serde_json::json!({
                            "id": completion_id, "object": "chat.completion.chunk", "created": created, "model": model,
                            "choices": [{"index": 0, "delta": {"content": format!("[upstream error] {}", err.message)}, "finish_reason": null}],
                        });
                        sse_send(&tx, Event::default().data(detail.to_string())).await;
                        err_snippet = Some(snippet);
                        let latency_ms = start.elapsed().as_millis() as i64;
                        finish_and_record(&tx, &pool, &slug_o, &account_o, &native_o, &completion_id, &model, created, &acc, err_snippet, true, latency_ms).await;
                        return;
                    }
                    Ok(lines) => {
                        for line in lines {
                            if !sse_send(&tx, Event::default().data(line)).await {
                                return;
                            }
                            content_started = true;
                        }
                    }
                }
            }
        }
        // Thought-only guard: never close 200-empty.
        if acc.is_empty_content() {
            let detail = serde_json::json!({
                "id": completion_id, "object": "chat.completion.chunk", "created": created, "model": model,
                "choices": [{"index": 0, "delta": {"content": "[upstream error] upstream returned no content"}, "finish_reason": null}],
            });
            sse_send(&tx, Event::default().data(detail.to_string())).await;
            if err_snippet.is_none() {
                err_snippet = Some("upstream returned no content".to_string());
            }
            content_started = true;
        }
        let latency_ms = start.elapsed().as_millis() as i64;
        finish_and_record(&tx, &pool, &slug_o, &account_o, &native_o, &completion_id, &model, created, &acc, err_snippet, content_started, latency_ms).await;
    });
    Attempt::Done(Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()).into_response())
}

/// Non-streaming OpenAI passthrough: parse upstream JSON, rewrite `model` to
/// the gateway echo id, record usage, return. Upstream shapes pass through
/// untouched otherwise (choices, finish_reason, usage).
async fn passthrough_json_response(
    state: &AppState,
    slug: &str,
    account_id: &str,
    native_model: &str,
    echo_model: &str,
    upstream: reqwest::Response,
) -> axum::response::Response {
    let mut body: serde_json::Value = match upstream.json().await {
        Ok(body) => body,
        Err(err) => return openai_error(StatusCode::BAD_GATEWAY, format!("upstream parse failed: {err}"), "proxy_dock_upstream_parse"),
    };
    let usage = body.get("usage").cloned().unwrap_or_else(|| serde_json::json!({"prompt_tokens": 0, "completion_tokens": 0}));
    if let Some(obj) = body.as_object_mut() {
        obj.insert("model".to_string(), serde_json::Value::String(echo_model.to_string()));
    }
    record_usage(&state.pool, slug, account_id, native_model, &usage).await;
    (StatusCode::OK, Json(body)).into_response()
}

/// Streaming OpenAI passthrough (OpenCode Zen Go, Command Code Provider):
/// forwards upstream `data:` payloads unchanged, rewrites the `model` field
/// per chunk to the gateway echo id, captures `usage` from the final chunk,
/// and records it. Always ends with a terminal usage chunk + `[DONE]`.
fn passthrough_stream_response(
    state: &AppState,
    slug: &str,
    account_id: &str,
    native_model: &str,
    echo_model: &str,
    upstream: reqwest::Response,
) -> axum::response::Response {
    passthrough_stream_response_with_route(state, slug, account_id, native_model, echo_model, upstream, "chat/completions")
}

fn passthrough_stream_response_with_route(
    state: &AppState,
    slug: &str,
    account_id: &str,
    native_model: &str,
    echo_model: &str,
    upstream: reqwest::Response,
    route: &'static str,
) -> axum::response::Response {
    let (tx, rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(SSE_CHANNEL_CAP);
    let model = echo_model.to_string();
    let pool = state.pool.clone();
    let slug_o = slug.to_string();
    let account_o = account_id.to_string();
    let native_o = native_model.to_string();
    let start = std::time::Instant::now();
    tokio::spawn(async move {
        let mut prompt_tokens: u64 = 0;
        let mut completion_tokens: u64 = 0;
        let mut cached_tokens: u64 = 0;
        let mut cache_creation_tokens: u64 = 0;
        let mut buffer: Vec<u8> = Vec::new();
        let mut upstream = upstream;
        let mut forwarded: u32 = 0;
        let mut err_snippet: Option<String> = None;
        loop {
            if tx.is_closed() {
                return;
            }
            let chunk = match upstream.chunk().await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => break,
                Err(err) => {
                    err_snippet = Some(redact_snippet(&format!("stream read failed: {err}")));
                    break;
                }
            };
            buffer.extend_from_slice(&chunk);
            while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&line).to_string();
                let Some(payload) = sse_payload(&text) else { continue };
                if payload == "[DONE]" {
                    continue;
                }
                let mut value: serde_json::Value = match serde_json::from_str(payload) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                if let Some(usage) = value.get("usage") {
                    let (prompt, completion, cached, creation) = usage_details(Some(usage));
                    prompt_tokens = prompt;
                    completion_tokens = completion;
                    cached_tokens = cached;
                    cache_creation_tokens = creation;
                }
                rewrite_model(&mut value, &model);
                if !sse_send(&tx, Event::default().data(value.to_string())).await {
                    return;
                }
                forwarded += 1;
            }
        }
        let usage = serde_json::json!({
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
            "prompt_tokens_details": {"cached_tokens": cached_tokens},
            "cache_read_input_tokens": cached_tokens,
            "cache_creation_input_tokens": cache_creation_tokens,
        });
        // Terminal usage chunk + DONE, always (synthesized when omitted).
        sse_send(&tx, Event::default().data(
            serde_json::json!({"id": cc::new_completion_id(), "object": "chat.completion.chunk", "created": cc::unix_now(), "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage}).to_string(),
        )).await;
        sse_send(&tx, Event::default().data("[DONE]")).await;
        record_usage(&pool, &slug_o, &account_o, &native_o, &usage).await;
        let outcome = if err_snippet.is_some() || forwarded == 0 { "truncated" } else { "ok" };
        let latency_ms = start.elapsed().as_millis() as i64;
        log_event(&pool, &slug_o, Some(&account_o), &native_o, route, outcome, 200, latency_ms, err_snippet.as_deref()).await;
        tracing::info!(slug = %slug_o, outcome = %outcome, latency_ms = latency_ms, "gateway stream done");
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()).into_response()
}

/// Non-streaming Codex: parse the Responses body, translate to
/// `chat.completion`, record Codex usage.
async fn codex_json_response(
    state: &AppState,
    slug: &str,
    account_id: &str,
    native_model: &str,
    echo_model: &str,
    upstream: reqwest::Response,
) -> axum::response::Response {
    let body: serde_json::Value = match upstream.json().await {
        Ok(body) => body,
        Err(err) => return openai_error(StatusCode::BAD_GATEWAY, format!("codex parse failed: {err}"), "codex_upstream_error"),
    };
    let completion_id = cc::new_completion_id();
    let created = cc::unix_now();
    let chat = cg::responses_to_chat_completion(&body, &completion_id, echo_model, created);
    let usage = chat.get("usage").cloned().unwrap_or_else(|| serde_json::json!({"prompt_tokens": 0, "completion_tokens": 0}));
    record_usage(&state.pool, slug, account_id, native_model, &usage).await;
    (StatusCode::OK, Json(chat)).into_response()
}

/// Streaming Codex: translate Responses SSE events to chat chunks.
fn codex_stream_response(
    state: &AppState,
    slug: &str,
    account_id: &str,
    native_model: &str,
    echo_model: &str,
    upstream: reqwest::Response,
) -> axum::response::Response {
    let (tx, rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(SSE_CHANNEL_CAP);
    let completion_id = cc::new_completion_id();
    let created = cc::unix_now();
    let model = echo_model.to_string();
    let pool = state.pool.clone();
    let slug_o = slug.to_string();
    let account_o = account_id.to_string();
    let native_o = native_model.to_string();
    let start = std::time::Instant::now();
    tokio::spawn(async move {
        if !sse_send(&tx, Event::default().data(cc::sse_role_chunk(&completion_id, &model, created))).await {
            return;
        }
        let mut sse_state = cg::CodexSseState::default();
        let mut buffer: Vec<u8> = Vec::new();
        let mut upstream = upstream;
        let mut content_started = false;
        let mut err_snippet: Option<String> = None;
        loop {
            if tx.is_closed() {
                return;
            }
            let chunk = match upstream.chunk().await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => break,
                Err(err) => {
                    err_snippet = Some(redact_snippet(&format!("stream read failed: {err}")));
                    break;
                }
            };
            buffer.extend_from_slice(&chunk);
            while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&line).to_string();
                let Some(payload) = sse_payload(&text) else { continue };
                if payload == "[DONE]" {
                    continue;
                }
                let value: serde_json::Value = match serde_json::from_str(payload) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let (lines, done) = match cg::translate_responses_data_checked(&completion_id, &model, created, &value, &mut sse_state) {
                    Ok(out) => out,
                    Err(msg) => {
                        // Surface upstream stream errors (failed/incomplete)
                        // instead of a fabricated clean stop.
                        err_snippet = Some(redact_snippet(&msg));
                        break;
                    }
                };
                for line in lines {
                    if !sse_send(&tx, Event::default().data(line)).await {
                        return;
                    }
                    content_started = true;
                }
                if done {
                    let usage = cg::codex_usage(&sse_state);
                    sse_send(&tx, Event::default().data(
                        serde_json::json!({"id": completion_id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage}).to_string(),
                    )).await;
                    sse_send(&tx, Event::default().data("[DONE]")).await;
                    record_usage(&pool, &slug_o, &account_o, &native_o, &usage).await;
                    let latency_ms = start.elapsed().as_millis() as i64;
                    log_event(&pool, &slug_o, Some(&account_o), &native_o, "chat/completions", "ok", 200, latency_ms, None).await;
                    tracing::info!(slug = %slug_o, outcome = "ok", latency_ms = latency_ms, "gateway stream done");
                    return;
                }
            }
        }
        // Always close with terminal usage + DONE (synthesized when omitted).
        let usage = cg::codex_usage(&sse_state);
        sse_send(&tx, Event::default().data(
            serde_json::json!({"id": completion_id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage}).to_string(),
        )).await;
        sse_send(&tx, Event::default().data("[DONE]")).await;
        record_usage(&pool, &slug_o, &account_o, &native_o, &usage).await;
        let outcome = if err_snippet.is_some() || !content_started { "truncated" } else { "ok" };
        let latency_ms = start.elapsed().as_millis() as i64;
        log_event(&pool, &slug_o, Some(&account_o), &native_o, "chat/completions", outcome, 200, latency_ms, err_snippet.as_deref()).await;
        tracing::info!(slug = %slug_o, outcome = %outcome, latency_ms = latency_ms, "gateway stream done");
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()).into_response()
}

/// Non-streaming Claude: parse the Messages body, translate to
/// `chat.completion`, record Messages usage.
async fn claude_json_response(
    state: &AppState,
    slug: &str,
    account_id: &str,
    native_model: &str,
    echo_model: &str,
    upstream: reqwest::Response,
) -> axum::response::Response {
    let body: serde_json::Value = match upstream.json().await {
        Ok(body) => body,
        Err(err) => return openai_error(StatusCode::BAD_GATEWAY, format!("claude parse failed: {err}"), "claude_upstream_error"),
    };
    let completion_id = cc::new_completion_id();
    let created = cc::unix_now();
    let chat = cl::messages_to_chat_completion(&body, &completion_id, echo_model, created);
    let usage = chat.get("usage").cloned().unwrap_or_else(|| serde_json::json!({"prompt_tokens": 0, "completion_tokens": 0}));
    record_usage(&state.pool, slug, account_id, native_model, &usage).await;
    (StatusCode::OK, Json(chat)).into_response()
}

/// Streaming Claude: translate Messages SSE events to chat chunks.
fn claude_stream_response(
    state: &AppState,
    slug: &str,
    account_id: &str,
    native_model: &str,
    echo_model: &str,
    upstream: reqwest::Response,
) -> axum::response::Response {
    let (tx, rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(SSE_CHANNEL_CAP);
    let completion_id = cc::new_completion_id();
    let created = cc::unix_now();
    let model = echo_model.to_string();
    let pool = state.pool.clone();
    let slug_o = slug.to_string();
    let account_o = account_id.to_string();
    let native_o = native_model.to_string();
    let start = std::time::Instant::now();
    tokio::spawn(async move {
        if !sse_send(&tx, Event::default().data(cc::sse_role_chunk(&completion_id, &model, created))).await {
            return;
        }
        let mut sse_state = cl::ClaudeSseState::default();
        let mut buffer: Vec<u8> = Vec::new();
        let mut upstream = upstream;
        let mut content_started = false;
        let mut err_snippet: Option<String> = None;
        loop {
            if tx.is_closed() {
                return;
            }
            let chunk = match upstream.chunk().await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => break,
                Err(err) => {
                    err_snippet = Some(redact_snippet(&format!("stream read failed: {err}")));
                    break;
                }
            };
            buffer.extend_from_slice(&chunk);
            while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&line).to_string();
                let Some(payload) = sse_payload(&text) else { continue };
                if payload == "[DONE]" {
                    continue;
                }
                let value: serde_json::Value = match serde_json::from_str(payload) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let (lines, done) = match cl::translate_messages_data_checked(&completion_id, &model, created, &value, &mut sse_state) {
                    Ok(out) => out,
                    Err(msg) => {
                        // Surface upstream stream errors (e.g. overloaded)
                        // instead of a fabricated clean stop.
                        err_snippet = Some(redact_snippet(&msg));
                        break;
                    }
                };
                for line in lines {
                    if !sse_send(&tx, Event::default().data(line)).await {
                        return;
                    }
                    content_started = true;
                }
                if done {
                    let usage = cl::claude_usage(&sse_state);
                    sse_send(&tx, Event::default().data(
                        serde_json::json!({"id": completion_id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage}).to_string(),
                    )).await;
                    sse_send(&tx, Event::default().data("[DONE]")).await;
                    record_usage(&pool, &slug_o, &account_o, &native_o, &usage).await;
                    let latency_ms = start.elapsed().as_millis() as i64;
                    log_event(&pool, &slug_o, Some(&account_o), &native_o, "chat/completions", "ok", 200, latency_ms, None).await;
                    tracing::info!(slug = %slug_o, outcome = "ok", latency_ms = latency_ms, "gateway stream done");
                    return;
                }
            }
        }
        // Always close with terminal usage + DONE (synthesized when omitted).
        let usage = cl::claude_usage(&sse_state);
        sse_send(&tx, Event::default().data(
            serde_json::json!({"id": completion_id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": usage}).to_string(),
        )).await;
        sse_send(&tx, Event::default().data("[DONE]")).await;
        record_usage(&pool, &slug_o, &account_o, &native_o, &usage).await;
        let outcome = if err_snippet.is_some() || !content_started { "truncated" } else { "ok" };
        let latency_ms = start.elapsed().as_millis() as i64;
        log_event(&pool, &slug_o, Some(&account_o), &native_o, "chat/completions", outcome, 200, latency_ms, err_snippet.as_deref()).await;
        tracing::info!(slug = %slug_o, outcome = %outcome, latency_ms = latency_ms, "gateway stream done");
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()).into_response()
}

/// Non-streaming Antigravity: unwrap the envelope, translate to
/// `chat.completion`, record Gemini usage.
async fn antigravity_json_response(
    state: &AppState,
    slug: &str,
    account_id: &str,
    native_model: &str,
    echo_model: &str,
    upstream: reqwest::Response,
) -> axum::response::Response {
    let body: serde_json::Value = match upstream.json().await {
        Ok(body) => body,
        Err(err) => return openai_error(StatusCode::BAD_GATEWAY, format!("antigravity parse failed: {err}"), "antigravity_upstream_error"),
    };
    let completion_id = cc::new_completion_id();
    let created = cc::unix_now();
    let chat = ag::gemini_to_chat_completion(&body, &completion_id, echo_model, created);
    let usage = chat.get("usage").cloned().unwrap_or_else(|| serde_json::json!({"prompt_tokens": 0, "completion_tokens": 0}));
    record_usage(&state.pool, slug, account_id, native_model, &usage).await;
    (StatusCode::OK, Json(chat)).into_response()
}

/// Streaming Antigravity: translate `streamGenerateContent` SSE envelopes to
/// chat chunks, accumulating Gemini usage across events.
fn antigravity_stream_response(
    state: &AppState,
    slug: &str,
    account_id: &str,
    native_model: &str,
    echo_model: &str,
    upstream: reqwest::Response,
) -> axum::response::Response {
    let (tx, rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(SSE_CHANNEL_CAP);
    let completion_id = cc::new_completion_id();
    let created = cc::unix_now();
    let model = echo_model.to_string();
    let pool = state.pool.clone();
    let slug_o = slug.to_string();
    let account_o = account_id.to_string();
    let native_o = native_model.to_string();
    let start = std::time::Instant::now();
    tokio::spawn(async move {
        if !sse_send(&tx, Event::default().data(cc::sse_role_chunk(&completion_id, &model, created))).await {
            return;
        }
        let mut prompt_tokens: u64 = 0;
        let mut completion_tokens: u64 = 0;
        let mut cached_tokens: u64 = 0;
        let mut buffer: Vec<u8> = Vec::new();
        let mut upstream = upstream;
        let mut content_started = false;
        let mut err_snippet: Option<String> = None;
        // functionCall streaming: ids are synthesized per event (Gemini
        // carries no client call id); each event's calls share one index
        // each, in arrival order.
        let mut tool_index: u32 = 0;
        loop {
            if tx.is_closed() {
                return;
            }
            let chunk = match upstream.chunk().await {
                Ok(Some(bytes)) => bytes,
                Ok(None) => break,
                Err(err) => {
                    err_snippet = Some(redact_snippet(&format!("stream read failed: {err}")));
                    break;
                }
            };
            buffer.extend_from_slice(&chunk);
            while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&line).to_string();
                let Some(payload) = sse_payload(&text) else { continue };
                if payload == "[DONE]" {
                    continue;
                }
                let value: serde_json::Value = match serde_json::from_str(payload) {
                    Ok(value) => value,
                    Err(_) => continue,
                };
                let (text, prompt, completion) = ag::gemini_fields(&value);
                let (_, _, cached) = ag::gemini_usage(&value);
                prompt_tokens = prompt_tokens.max(prompt);
                completion_tokens = completion_tokens.saturating_add(completion);
                cached_tokens = cached_tokens.max(cached);
                let mut deltas: Vec<serde_json::Value> = Vec::new();
                if !text.is_empty() {
                    deltas.push(serde_json::json!({"content": text}));
                }
                for call in ag::gemini_tool_calls(&value) {
                    let index = tool_index;
                    tool_index += 1;
                    deltas.push(serde_json::json!({"tool_calls": [{
                        "index": index,
                        "id": call.get("id"),
                        "type": "function",
                        "function": call.get("function"),
                    }]}));
                }
                if deltas.is_empty() {
                    continue;
                }
                for delta in deltas {
                    let chunk_body = serde_json::json!({
                        "id": completion_id, "object": "chat.completion.chunk", "created": created, "model": model,
                        "choices": [{"index": 0, "delta": delta, "finish_reason": null}],
                    });
                    if !sse_send(&tx, Event::default().data(chunk_body.to_string())).await {
                        return;
                    }
                }
                content_started = true;
            }
        }
        let usage = serde_json::json!({
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
            "prompt_tokens_details": {"cached_tokens": cached_tokens},
            "cache_read_input_tokens": cached_tokens,
        });
        let finish = if tool_index > 0 { "tool_calls" } else { "stop" };
        sse_send(&tx, Event::default().data(
            serde_json::json!({"id": completion_id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": finish}], "usage": usage}).to_string(),
        )).await;
        sse_send(&tx, Event::default().data("[DONE]")).await;
        record_usage(&pool, &slug_o, &account_o, &native_o, &usage).await;
        let outcome = if err_snippet.is_some() || !content_started { "truncated" } else { "ok" };
        let latency_ms = start.elapsed().as_millis() as i64;
        log_event(&pool, &slug_o, Some(&account_o), &native_o, "chat/completions", outcome, 200, latency_ms, err_snippet.as_deref()).await;
        tracing::info!(slug = %slug_o, outcome = %outcome, latency_ms = latency_ms, "gateway stream done");
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default()).into_response()
}

/// Honest foundation gate: resolve the default account via routing policy,
/// check keyring presence only, and return a structured 501. Never proxies,
/// never fabricates upstream traffic, never exposes secrets.
async fn gateway_not_ready(pool: &SqlitePool, slug: &str) -> axum::response::Response {
    let account = default_account_for(pool, slug).await;
    let account_id = account.as_deref().unwrap_or("default");
    let has_token = secrets::has_local_token_async(slug.to_string(), account_id.to_string()).await
        || match &account {
            Some(id) => secrets::has_local_token_async(slug.to_string(), id.clone()).await,
            None => false,
        };
    if !has_token {
        return not_configured(slug);
    }
    // Token present but no routable plan (e.g. Free keys with no subscription,
    // or an unrecognized plan). Command Code Go rides the private CLI-style
    // route; everything else needs a servable plan (see attempt_candidate).
    openai_error_full(
        StatusCode::NOT_IMPLEMENTED,
        format!("{slug} adapter has a local token but no servable plan for this account (refresh quota to detect the plan, or link an account with an active subscription)."),
        "proxy_dock_no_plan",
        Some(slug),
    )
}

/// Default account for a provider: first row of the canonical rolling order.
/// Matches `candidate_accounts` exactly (including the disabled exclusion),
/// so the 501 gate and the rolling path never disagree. Returns the account
/// id or None when unconfigured.
async fn default_account_for(pool: &SqlitePool, slug: &str) -> Option<String> {
    let row: Option<(String,)> = sqlx::query_as(&format!(
        "SELECT a.id FROM accounts a LEFT JOIN routing_policy r ON r.provider_slug = a.provider_slug AND r.account_id = a.id WHERE a.provider_slug = ? AND COALESCE(r.enabled, 1) = 1 ORDER BY {CANDIDATE_ORDER_BY} LIMIT 1"
    ))
    .bind(slug)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);
    row.map(|(id,)| id)
}

fn not_configured(slug: &str) -> axum::response::Response {
    (StatusCode::NOT_IMPLEMENTED, Json(secrets::not_configured_response(slug))).into_response()
}

async fn list_providers(State(state): State<AppState>) -> Json<Vec<ProviderInfo>> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT slug, label FROM providers ORDER BY slug")
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
    Json(rows.into_iter().map(|(slug, label)| ProviderInfo { slug, label }).collect())
}

async fn list_accounts(State(state): State<AppState>, Query(params): Query<HashMap<String, String>>) -> Json<Vec<AccountInfo>> {
    // Never expose secrets: the SQLite `credential` column is legacy and must
    // not be served. Presence comes from the OS keyring only.
    let provider = params.get("provider").cloned().unwrap_or_default();
    let rows: Vec<(String, String, Option<String>)> = if provider.is_empty() {
        sqlx::query_as("SELECT id, provider_slug, plan FROM accounts ORDER BY provider_slug, id")
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default()
    } else {
        sqlx::query_as("SELECT id, provider_slug, plan FROM accounts WHERE provider_slug = ? ORDER BY id")
            .bind(&provider)
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default()
    };
    Json(
        rows.into_iter()
            .map(|(id, provider_slug, plan)| {
                let token = secrets::stored_presence(&provider_slug, &id);
                AccountInfo { id, provider_slug, credential: None, plan, token }
            })
            .collect(),
    )
}

/// Canonical routing row: label/priority/enabled resolved with the same
/// `COALESCE` fallback the gateway orders by, so the API and the send path
/// can never disagree about order or default.
async fn routing_rows(pool: &SqlitePool, slug: &str) -> Vec<serde_json::Value> {
    let rows: Vec<(String, String, i64, i64, Option<String>)> = sqlx::query_as(&format!(
        "SELECT a.id, a.label, COALESCE(r.priority, a.priority, 1), COALESCE(r.enabled, 1), a.plan FROM accounts a \
         LEFT JOIN routing_policy r ON r.provider_slug = a.provider_slug AND r.account_id = a.id \
         WHERE a.provider_slug = ? ORDER BY {CANDIDATE_ORDER_BY}"
    ))
    .bind(slug)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows
        .into_iter()
        .enumerate()
        .map(|(index, (id, label, priority, enabled, plan))| {
            serde_json::json!({
                "accountId": id,
                "label": label,
                "priority": priority,
                "enabled": enabled != 0,
                "plan": plan,
                "hasToken": secrets::has_local_token(slug, &id),
                "isDefault": index == 0,
            })
        })
        .collect()
}

/// `GET /api/routing/accounts?provider=` — canonical ordered candidates.
/// `?provider=` is required (identity is the pair).
async fn routing_accounts(State(state): State<AppState>, Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    let provider = params.get("provider").cloned().unwrap_or_default();
    if provider.is_empty() || !crate::PROVIDER_SLUGS.contains(&provider.as_str()) {
        return openai_error(
            StatusCode::BAD_REQUEST,
            "?provider= must name a known provider".to_string(),
            "proxy_dock_bad_query",
        );
    }
    (StatusCode::OK, Json(serde_json::json!({"provider": provider, "accounts": routing_rows(&state.pool, &provider).await}))).into_response()
}

/// `PATCH /api/accounts/:provider/:id {label?, priority?, enabled?}`.
/// `label → accounts.label`; `priority/enabled → routing_policy` upsert
/// (the `accounts.priority` fallback in the `COALESCE` still applies when no
/// policy row exists). Empty bodies are 422; bad values are 422; unknown
/// provider/account is 404. Behind the §0 origin gate.
async fn patch_account(
    State(state): State<AppState>,
    Path((provider, id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !crate::PROVIDER_SLUGS.contains(&provider.as_str()) {
        return openai_error(StatusCode::NOT_FOUND, format!("unknown provider '{provider}'"), "proxy_dock_unknown_provider");
    }
    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM accounts WHERE provider_slug = ? AND id = ?")
        .bind(&provider)
        .bind(&id)
        .fetch_optional(&state.pool)
        .await
        .unwrap_or(None);
    if exists.is_none() {
        return openai_error(StatusCode::NOT_FOUND, format!("unknown account '{id}' for provider '{provider}'"), "proxy_dock_unknown_account");
    }
    let label = body.get("label").and_then(|v| v.as_str()).map(str::trim);
    let priority = body.get("priority").and_then(|v| v.as_i64());
    let enabled = body.get("enabled").and_then(|v| v.as_bool());
    if label.is_none() && priority.is_none() && enabled.is_none() {
        return openai_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "nothing to update — send at least one of label, priority, enabled".to_string(),
            "proxy_dock_bad_patch",
        );
    }
    if let Some(label) = label {
        if label.is_empty() || label.chars().count() > 64 {
            return openai_error(StatusCode::UNPROCESSABLE_ENTITY, "label must be 1-64 chars".to_string(), "proxy_dock_bad_patch");
        }
        let _ = sqlx::query("UPDATE accounts SET label = ? WHERE provider_slug = ? AND id = ?")
            .bind(label)
            .bind(&provider)
            .bind(&id)
            .execute(&state.pool)
            .await;
    }
    if let Some(priority) = priority {
        if !(0..=100).contains(&priority) {
            return openai_error(StatusCode::UNPROCESSABLE_ENTITY, "priority must be 0-100 (lower wins)".to_string(), "proxy_dock_bad_patch");
        }
        let _ = sqlx::query(
            "INSERT INTO routing_policy (provider_slug, account_id, priority, enabled) VALUES (?, ?, ?, COALESCE((SELECT enabled FROM routing_policy WHERE provider_slug = ? AND account_id = ?), 1)) \
             ON CONFLICT(provider_slug, account_id) DO UPDATE SET priority = excluded.priority",
        )
        .bind(&provider)
        .bind(&id)
        .bind(priority)
        .bind(&provider)
        .bind(&id)
        .execute(&state.pool)
        .await;
    }
    if let Some(enabled) = enabled {
        let _ = sqlx::query(
            "INSERT INTO routing_policy (provider_slug, account_id, priority, enabled) VALUES (?, ?, COALESCE((SELECT priority FROM routing_policy WHERE provider_slug = ? AND account_id = ?), 1), ?) \
             ON CONFLICT(provider_slug, account_id) DO UPDATE SET enabled = excluded.enabled",
        )
        .bind(&provider)
        .bind(&id)
        .bind(&provider)
        .bind(&id)
        .bind(if enabled { 1 } else { 0 })
        .execute(&state.pool)
        .await;
    }
    let rows = routing_rows(&state.pool, &provider).await;
    let updated = rows.into_iter().find(|r| r.get("accountId").and_then(|v| v.as_str()) == Some(id.as_str()));
    match updated {
        Some(row) => (StatusCode::OK, Json(row)).into_response(),
        None => openai_error(StatusCode::NOT_FOUND, format!("unknown account '{id}'"), "proxy_dock_unknown_account"),
    }
}

/// `DELETE /api/accounts/:provider/:id` — transaction, child (`routing_policy`)
/// before parent (`accounts`); history rows (`usage_events`,
/// `quota_observations`, `gateway_events`) purge with the account; keyring
/// cleared best-effort last with an explicit error when the secret survives.
/// Behind the §0 origin gate.
async fn delete_account(State(state): State<AppState>, Path((provider, id)): Path<(String, String)>) -> impl IntoResponse {
    if !crate::PROVIDER_SLUGS.contains(&provider.as_str()) {
        return openai_error(StatusCode::NOT_FOUND, format!("unknown provider '{provider}'"), "proxy_dock_unknown_provider");
    }
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(err) => return openai_error(StatusCode::BAD_GATEWAY, format!("database unavailable: {err}"), "proxy_dock_db_unavailable"),
    };
    let deleted_policy = sqlx::query("DELETE FROM routing_policy WHERE provider_slug = ? AND account_id = ?")
        .bind(&provider)
        .bind(&id)
        .execute(&mut *tx)
        .await
        .map(|r| r.rows_affected())
        .unwrap_or(0);
    let _ = sqlx::query("DELETE FROM usage_events WHERE provider_slug = ? AND account_id = ?")
        .bind(&provider)
        .bind(&id)
        .execute(&mut *tx)
        .await;
    let _ = sqlx::query("DELETE FROM quota_observations WHERE provider_slug = ? AND account_id = ?")
        .bind(&provider)
        .bind(&id)
        .execute(&mut *tx)
        .await;
    let _ = sqlx::query("DELETE FROM gateway_events WHERE provider_slug = ? AND account_id = ?")
        .bind(&provider)
        .bind(&id)
        .execute(&mut *tx)
        .await;
    let deleted_account = sqlx::query("DELETE FROM accounts WHERE provider_slug = ? AND id = ?")
        .bind(&provider)
        .bind(&id)
        .execute(&mut *tx)
        .await
        .map(|r| r.rows_affected())
        .unwrap_or(0);
    if deleted_account == 0 {
        let _ = tx.rollback().await;
        return openai_error(StatusCode::NOT_FOUND, format!("unknown account '{id}' for provider '{provider}'"), "proxy_dock_unknown_account");
    }
    if tx.commit().await.is_err() {
        return openai_error(StatusCode::BAD_GATEWAY, "database commit failed".to_string(), "proxy_dock_db_unavailable");
    }
    // Keyring last: DB truth first, secret best-effort with explicit error.
    let _ = secrets::clear_local_token(provider.clone(), id.clone());
    if secrets::has_local_token(&provider, &id) {
        return openai_error(
            StatusCode::BAD_GATEWAY,
            format!("account rows removed but the OS secret for '{id}' survived — remove it from the keyring manually"),
            "proxy_dock_secret_survived",
        );
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({"deleted": true, "provider": provider, "account": id, "policy_rows": deleted_policy})),
    )
        .into_response()
}

/// `PUT /api/pricing/:provider/:model {prompt_per_1k, completion_per_1k,
/// source}` — manual snapshots only (no scraping). Appends a row;
/// latest-wins on read. Negative/non-finite rates and empty sources are 422.
/// Optional `cache_read_per_1k` / `cache_write_per_1k` price the cache legs;
/// absent/invalid ones fall back to the input rate. Behind the §0 origin gate.
async fn put_pricing(
    State(state): State<AppState>,
    Path((provider, model)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !crate::PROVIDER_SLUGS.contains(&provider.as_str()) {
        return openai_error(StatusCode::NOT_FOUND, format!("unknown provider '{provider}'"), "proxy_dock_unknown_provider");
    }
    if model.trim().is_empty() || model.chars().count() > 256 {
        return openai_error(StatusCode::UNPROCESSABLE_ENTITY, "model must be 1-256 chars".to_string(), "proxy_dock_bad_pricing");
    }
    let prompt_rate = body.get("prompt_per_1k").and_then(|v| v.as_f64());
    let completion_rate = body.get("completion_per_1k").and_then(|v| v.as_f64());
    let cache_read_rate = body.get("cache_read_per_1k").and_then(|v| v.as_f64());
    let cache_write_rate = body.get("cache_write_per_1k").and_then(|v| v.as_f64());
    let source = body.get("source").and_then(|v| v.as_str()).map(str::trim).unwrap_or("");
    let valid = |rate: Option<f64>| rate.map(|r| r.is_finite() && r >= 0.0).unwrap_or(false);
    if !valid(prompt_rate) || !valid(completion_rate) {
        return openai_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "prompt_per_1k and completion_per_1k must be finite numbers >= 0".to_string(),
            "proxy_dock_bad_pricing",
        );
    }
    if !cache_read_rate.map(|r| r.is_finite() && r >= 0.0).unwrap_or(true) {
        return openai_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "cache_read_per_1k must be a finite number >= 0 when present".to_string(),
            "proxy_dock_bad_pricing",
        );
    }
    if !cache_write_rate.map(|r| r.is_finite() && r >= 0.0).unwrap_or(true) {
        return openai_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "cache_write_per_1k must be a finite number >= 0 when present".to_string(),
            "proxy_dock_bad_pricing",
        );
    }
    if source.is_empty() || source.chars().count() > 128 {
        return openai_error(StatusCode::UNPROCESSABLE_ENTITY, "source must be 1-128 chars (who set this price)".to_string(), "proxy_dock_bad_pricing");
    }
    let mut price = serde_json::json!({"prompt_per_1k": prompt_rate, "completion_per_1k": completion_rate});
    if let Some(r) = cache_read_rate {
        price["cache_read_per_1k"] = serde_json::json!(r);
    }
    if let Some(r) = cache_write_rate {
        price["cache_write_per_1k"] = serde_json::json!(r);
    }
    let price_json = price.to_string();
    if sqlx::query("INSERT INTO pricing_snapshots (provider_slug, model, source, price_json) VALUES (?, ?, ?, ?)")
        .bind(&provider)
        .bind(model.trim())
        .bind(source)
        .bind(&price_json)
        .execute(&state.pool)
        .await
        .is_err()
    {
        return openai_error(StatusCode::BAD_GATEWAY, "database unavailable".to_string(), "proxy_dock_db_unavailable");
    }
    // Backfill past NULL-cost rows (shared with the background models.dev sync).
    crate::pricing::backfill_costs(
        &state.pool,
        &provider,
        model.trim(),
        prompt_rate.unwrap_or(0.0),
        completion_rate.unwrap_or(0.0),
        cache_read_rate,
        cache_write_rate,
    )
    .await;
    (
        StatusCode::OK,
        Json(serde_json::json!({"provider": provider, "model": model.trim(), "source": source, "price": serde_json::from_str::<serde_json::Value>(&price_json).unwrap_or_default()})),
    )
        .into_response()
}

/// `POST /api/pricing/sync` — manual models.dev refresh with the same rules
/// as the background sync (fills only missing/refreshed-third-party prices;
/// manual snapshots always win). Returns counts plus the unmatched ids so
/// callers can say which models still lack prices. Behind the §0 origin gate.
async fn sync_pricing_endpoint(State(state): State<AppState>) -> impl IntoResponse {
    let report = crate::pricing::sync_pricing(&state.pool).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "inserted": report.inserted,
            "refreshed": report.refreshed,
            "skipped": report.skipped_have_price,
            "unmatched": report.unmatched,
        })),
    )
        .into_response()
}

/// `GET /api/models?provider=&refresh=` — cached catalog (2E) with
/// `updated_at`/`stale`; `refresh=1` triggers a live token-gated refresh
/// (1A) for the default account. ChatGPT has no known catalog endpoint —
/// refresh returns an honest reason, never fabricated entries.
async fn api_models(State(state): State<AppState>, Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    let provider = params.get("provider").cloned().unwrap_or_default();
    if provider.is_empty() || !crate::PROVIDER_SLUGS.contains(&provider.as_str()) {
        return openai_error(
            StatusCode::BAD_REQUEST,
            "?provider= must name a known provider".to_string(),
            "proxy_dock_bad_query",
        );
    }
    let refresh = params.get("refresh").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false);
    if refresh {
        let account = default_account_for(&state.pool, &provider).await;
        let Some(account_id) = account else {
            return openai_error(
                StatusCode::NOT_IMPLEMENTED,
                "no linked account to refresh the catalog with — sign in first".to_string(),
                "proxy_dock_not_configured",
            );
        };
        // Singleflight: every provider-page mount fires refresh=1 at once.
        // The first request runs the live chain; concurrent ones serve cache
        // immediately (stale) instead of stampeding upstream in parallel.
        let run_live = state
            .catalog_refreshing
            .lock()
            .map(|mut running| running.insert(provider.clone()))
            .unwrap_or(true);
        if run_live {
            let result = catalog::refresh_catalog(&state.pool, &provider, &account_id).await;
            let _ = state.catalog_refreshing.lock().map(|mut running| running.remove(&provider));
            if let Err(reason) = result {
            let (entries, updated_at, _) = catalog::read_cache(&state.pool, &provider).await;
            let sources = catalog::sources_for(&state.pool, &provider).await;
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "provider": provider,
                    "models": catalog_entries_json(&entries, &sources),
                    "updated_at": updated_at,
                    "stale": true,
                    "reason": reason,
                })),
            )
                .into_response();
            }
        }
        // Fresh models may still lack prices — fill from Models.dev in the
        // background (manual snapshots always win).
        crate::pricing::spawn_sync(&state.pool);
    }
    let (entries, updated_at, stale) = catalog::read_cache(&state.pool, &provider).await;
    let sources = catalog::sources_for(&state.pool, &provider).await;
    let reason = if entries.is_empty() {
        if provider == "chatgpt" {
            Some("No Codex models yet — they appear here as you route requests through the gateway.".to_string())
        } else {
            let candidates = candidate_accounts(&state.pool, &provider).await;
            let has_token = candidates.iter().any(|c| secrets::has_local_token(&provider, &c.id));
            Some(if has_token {
                "No models yet — the provider returned none. Hit Refresh catalog to retry.".to_string()
            } else {
                "No models yet — sign in above and they load automatically.".to_string()
            })
        }
    } else {
        None
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "provider": provider,
            "models": catalog_entries_json(&entries, &sources),
            "updated_at": updated_at,
            "stale": stale,
            "reason": reason,
        })),
    )
        .into_response()
}

fn catalog_entries_json(
    entries: &[catalog::CatalogEntry],
    sources: &std::collections::HashMap<String, String>,
) -> Vec<serde_json::Value> {
    entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "nativeId": e.native_id,
                "displayName": e.display_name,
                "endpoints": e.endpoints,
                "contextLength": e.context_length,
                "source": sources.get(&e.native_id).cloned().unwrap_or_else(|| "live".to_string()),
            })
        })
        .collect()
}

/// Partial OAuth JSON (missing account_id/expires_in, as sent by the preview
/// UI). Strict `StoredCredential` parsing alone would fall through to the
/// key-hash slot and ghost-duplicate the email-slotted account.
#[derive(serde::Deserialize)]
struct PartialCred {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    email: Option<String>,
    account_id: Option<String>,
}

/// Normalize a pasted OAuth credential (full or partial `StoredCredential`
/// JSON) for gateway-side storage. `None` means the paste is not OAuth JSON
/// at all (a raw API key or setup token) — callers handle those per
/// provider. The stored value is always normalized `StoredCredential` JSON
/// (never the raw paste) so gateway-side OAuth refresh keeps working.
fn normalize_oauth_import(secret: &str) -> Option<crate::oauth::StoredCredential> {
    let mut stored: crate::oauth::StoredCredential = serde_json::from_str(secret).ok().or_else(|| {
        serde_json::from_str::<PartialCred>(secret).ok().and_then(|p| {
            if p.access_token.trim().is_empty() {
                return None;
            }
            let email = p.email.unwrap_or_default().trim().to_string();
            Some(crate::oauth::StoredCredential {
                access_token: p.access_token.trim().to_string(),
                refresh_token: p.refresh_token.map(|r| r.trim().to_string()).filter(|r| !r.is_empty()),
                id_token: p.id_token,
                account_id: p.account_id.unwrap_or_default(),
                email,
                expires_in: None,
            })
        })
    })?;
    if stored.access_token.trim().is_empty() {
        return None;
    }
    let email = stored.email.trim().to_string();
    stored.email = email.clone();
    if stored.account_id.trim().is_empty() {
        stored.account_id = secrets::account_id_for(&email, &stored.access_token);
    }
    Some(stored)
}

/// Store a normalized OAuth credential (keyring + routing row) and return
/// the import response. Pasted tokens carry no live verification here:
/// accept + store, then let quota refresh validate afterwards with honest
/// errors. Always `verified: false`.
async fn store_oauth_import(
    state: &AppState,
    provider: &str,
    stored: crate::oauth::StoredCredential,
) -> axum::response::Response {
    let account = stored.account_id.clone();
    let email = stored.email.clone();
    let payload = match serde_json::to_string(&stored) {
        Ok(payload) => payload,
        Err(err) => return openai_error(StatusCode::BAD_GATEWAY, format!("credential encode failed: {err}"), "proxy_dock_secret_store"),
    };
    if secrets::store_raw_token(provider, &account, &payload).is_err() {
        return openai_error(StatusCode::BAD_GATEWAY, "OS secret store unavailable".to_string(), "proxy_dock_secret_store");
    }
    let display = if email.is_empty() { account.clone() } else { email.clone() };
    let _ = db::register_account(&state.pool, provider, &account, &display, None).await;
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "provider": provider,
            "account": account,
            "email": if email.is_empty() { None } else { Some(email) },
            "plan": None::<String>,
            "verified": false,
        })),
    )
        .into_response()
}

/// `POST /api/accounts/:provider/credentials {secret}` — loopback credential
/// import for the headless gateway UI (no Tauri IPC there). Mirrors the
/// desktop verify-and-store commands exactly: per-provider verification
/// first, then keyring store + account registration + best-effort catalog
/// refresh. OAuth browser flows are NOT served here (pasted credentials
/// only). Behind the §0 origin gate like all mutating verbs. The secret is
/// never echoed back and never logged.
async fn import_credential(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if !crate::PROVIDER_SLUGS.contains(&provider.as_str()) {
        return openai_error(StatusCode::NOT_FOUND, format!("unknown provider '{provider}'"), "proxy_dock_unknown_provider");
    }
    let secret = body.get("secret").and_then(|v| v.as_str()).map(str::trim).unwrap_or("");
    if secret.len() < 8 {
        return openai_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "that credential looks too short — paste the full value".to_string(),
            "proxy_dock_bad_credential",
        );
    }
    if cc::is_placeholder_token(secret) {
        return openai_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "placeholder credentials are never stored".to_string(),
            "proxy_dock_bad_credential",
        );
    }
    // Per-provider verification (same functions as the desktop commands).
    enum Verified {
        Key { account: String, email: Option<String>, plan: Option<String>, label: String, verified: bool },
    }
    let verified: Result<Verified, String> = match provider.as_str() {
        "opencode" => match crate::oauth::verify_opencode_key(secret).await {
            Ok(status) => Ok(Verified::Key {
                account: secrets::account_id_for("", secret),
                email: None,
                plan: Some(status.plan),
                label: String::new(),
                verified: true,
            }),
            Err(err) => Err(err),
        },
        "commandcode" => match crate::oauth::verify_commandcode_key(secret).await {
            Ok(identity) => {
                let email = if identity.email.is_empty() { identity.user_name.clone() } else { identity.email.clone() };
                Ok(Verified::Key {
                    account: secrets::account_id_for(&identity.email, secret),
                    email: if email.is_empty() { None } else { Some(email.clone()) },
                    plan: None,
                    label: email,
                    verified: true,
                })
            }
            Err(err) => Err(err),
        },
        "chatgpt" | "antigravity" => {
            // Pasted tokens carry no verifiable identity without a live
            // provider round-trip here: accept + store, then let quota
            // refresh validate afterwards with honest errors. The stored
            // value is always normalized `StoredCredential` JSON (never the
            // raw paste) so gateway-side OAuth refresh keeps working.
            match normalize_oauth_import(secret) {
                Some(stored) => return store_oauth_import(&state, &provider, stored).await,
                None => Ok(Verified::Key {
                    account: secrets::account_id_for("", secret),
                    email: None,
                    plan: None,
                    label: String::new(),
                    verified: false,
                }),
            }
        }
        "claude" => {
            // OAuth JSON pastes share the normalized store path. Raw pastes
            // are `sk-ant-oat-*` setup tokens (OAuth bearers without refresh
            // — stored, validated later by quota refresh) or `sk-ant-*` API
            // keys (verified live against the Models API before storing).
            if let Some(stored) = normalize_oauth_import(secret) {
                return store_oauth_import(&state, &provider, stored).await;
            }
            if secret.trim().starts_with("sk-ant-oat-") {
                Ok(Verified::Key {
                    account: secrets::account_id_for("", secret),
                    email: None,
                    plan: None,
                    label: String::new(),
                    verified: false,
                })
            } else {
                match crate::oauth::verify_claude_key(secret).await {
                    Ok(_) => Ok(Verified::Key {
                        account: secrets::account_id_for("", secret),
                        email: None,
                        plan: None,
                        label: String::new(),
                        verified: true,
                    }),
                    Err(err) => Err(err),
                }
            }
        }
        _ => Err(format!("unknown provider '{provider}'")),
    };
    let Verified::Key { account, email, plan, label, verified } = match verified {
        Ok(v) => v,
        Err(message) => return openai_error(StatusCode::BAD_GATEWAY, message, "proxy_dock_credential_rejected"),
    };
    if secrets::store_raw_token(&provider, &account, secret).is_err() {
        return openai_error(StatusCode::BAD_GATEWAY, "OS secret store unavailable".to_string(), "proxy_dock_secret_store");
    }
    let display = if label.is_empty() { account.clone() } else { label };
    let _ = db::register_account(&state.pool, &provider, &account, &display, plan.as_deref()).await;
    // Pastes stored without live verification stay ineligible for routing
    // until a quota refresh validates them (candidate_accounts filter).
    if !verified {
        let _ = db::mark_account_unverified(&state.pool, &provider, &account).await;
    }
    // Best-effort catalog warm-up in the background: never delays sign-in
    // (the UI auto-refreshes the catalog on mount and offers manual Refresh;
    // awaiting the full upstream chain here is what made sign-in feel slow).
    {
        let warm_pool = state.pool.clone();
        let warm_account = account.clone();
        let warm_provider = provider.clone();
        tokio::spawn(async move {
            if catalog::refresh_catalog(&warm_pool, &warm_provider, &warm_account).await.is_ok() {
                crate::pricing::spawn_sync(&warm_pool);
            }
        });
    }
    // Fill any still-unpriced models in the background (manual wins).
    crate::pricing::spawn_sync(&state.pool);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "provider": provider,
            "account": account,
            "email": email,
            "plan": plan,
            "verified": verified,
        })),
    )
        .into_response()
}

/// `GET /api/accounts/:provider/:id/quota/history?since&limit` — persisted
/// quota observations for sparklines; last-known-wins on the UI when empty.
async fn quota_history(
    State(state): State<AppState>,
    Path((provider, id)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if !crate::PROVIDER_SLUGS.contains(&provider.as_str()) {
        return openai_error(StatusCode::NOT_FOUND, format!("unknown provider '{provider}'"), "proxy_dock_unknown_provider");
    }
    let bounds = match time_bounds(&params) {
        Ok(bounds) => bounds,
        Err(resp) => return resp,
    };
    let limit: i64 = params.get("limit").and_then(|v| v.parse().ok()).map(|n: i64| n.clamp(1, 500)).unwrap_or(100);
    let mut conds = vec!["provider_slug = ?".to_string(), "account_id = ?".to_string()];
    if bounds.0.is_some() {
        conds.push("observed_at >= ?".to_string());
    }
    if bounds.1.is_some() {
        conds.push("observed_at <= ?".to_string());
    }
    let sql = format!(
        "SELECT observed_at, remaining, reset_at FROM quota_observations WHERE {} ORDER BY id DESC LIMIT ?",
        conds.join(" AND ")
    );
    let mut q = sqlx::query_as::<_, (String, Option<String>, Option<String>)>(&sql)
        .bind(&provider)
        .bind(&id);
    if let Some(s) = &bounds.0 {
        q = q.bind(s);
    }
    if let Some(u) = &bounds.1 {
        q = q.bind(u);
    }
    let rows: Vec<(String, Option<String>, Option<String>)> = q
        .bind(limit)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "provider": provider,
            "account": id,
            "observations": rows.into_iter().map(|(observed_at, remaining, reset_at)| {
                serde_json::json!({
                    "observed_at": observed_at,
                    "remaining": remaining.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
                    "reset_at": reset_at,
                })
            }).collect::<Vec<_>>(),
        })),
    )
        .into_response()
}

/// `POST /api/accounts/:provider/:id/quota/refresh` — HTTP front for the
/// Tauri `refresh_quota` command (single quota path for UI + desktop).
/// Behind the §0 origin gate like all mutating verbs.
async fn quota_refresh(State(state): State<AppState>, Path((provider, id)): Path<(String, String)>) -> impl IntoResponse {
    if !crate::PROVIDER_SLUGS.contains(&provider.as_str()) {
        return openai_error(StatusCode::NOT_FOUND, format!("unknown provider '{provider}'"), "proxy_dock_unknown_provider");
    }
    match crate::quota::refresh_and_record(&state.pool, &provider, &id).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::json!(result))).into_response(),
        Err(message) => openai_error(StatusCode::BAD_GATEWAY, message, "proxy_dock_quota_failed"),
    }
}

/// `GET /api/events?provider&account&since&limit` — recent gateway activity
/// for Home/provider panels. `?account=` requires `?provider=`.
async fn api_events(State(state): State<AppState>, Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    let provider = params.get("provider").cloned().unwrap_or_default();
    let account = params.get("account").cloned().unwrap_or_default();
    if !account.is_empty() && provider.is_empty() {
        return openai_error(
            StatusCode::BAD_REQUEST,
            "?account= requires ?provider= (identity is the provider+account pair)".to_string(),
            "proxy_dock_bad_query",
        );
    }
    if !provider.is_empty() && !crate::PROVIDER_SLUGS.contains(&provider.as_str()) {
        return openai_error(StatusCode::NOT_FOUND, format!("unknown provider '{provider}'"), "proxy_dock_unknown_provider");
    }
    let bounds = match time_bounds(&params) {
        Ok(bounds) => bounds,
        Err(resp) => return resp,
    };
    let limit: i64 = params.get("limit").and_then(|v| v.parse().ok()).map(|n: i64| n.clamp(1, 500)).unwrap_or(100);
    let mut conds: Vec<String> = Vec::new();
    if !provider.is_empty() {
        conds.push("provider_slug = ?".to_string());
    }
    if !account.is_empty() {
        conds.push("account_id = ?".to_string());
    }
    if bounds.0.is_some() {
        conds.push("created_at >= ?".to_string());
    }
    if bounds.1.is_some() {
        conds.push("created_at <= ?".to_string());
    }
    let where_sql = if conds.is_empty() { String::new() } else { format!("WHERE {}", conds.join(" AND ")) };
    let sql = format!(
        "SELECT id, created_at, provider_slug, account_id, model, route, outcome, status, latency_ms, error_snippet FROM gateway_events {where_sql} ORDER BY id DESC LIMIT ?"
    );
    let mut q = sqlx::query_as::<_, (i64, String, String, String, String, String, String, i64, i64, Option<String>)>(&sql);
    if !provider.is_empty() {
        q = q.bind(&provider);
    }
    if !account.is_empty() {
        q = q.bind(&account);
    }
    if let Some(s) = &bounds.0 {
        q = q.bind(s);
    }
    if let Some(u) = &bounds.1 {
        q = q.bind(u);
    }
    let rows = q.bind(limit).fetch_all(&state.pool).await.unwrap_or_default();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "events": rows.into_iter().map(|(id, created_at, provider_slug, account_id, model, route, outcome, status, latency_ms, error_snippet)| {
                serde_json::json!({
                    "id": id, "ts": created_at, "slug": provider_slug, "account": account_id,
                    "model": model, "route": route, "outcome": outcome, "status": status,
                    "latencyMs": latency_ms, "error": error_snippet,
                })
            }).collect::<Vec<_>>(),
        })),
    )
        .into_response()
}

/// Time-bound validation shared by usage/events/quota-history queries.
/// Present-but-unparseable bounds are 400 (never silently mis-filtered);
/// absent bounds are unbounded.
fn time_bounds(params: &HashMap<String, String>) -> Result<(Option<String>, Option<String>), axum::response::Response> {
    let mut out = (None, None);
    for (key, slot) in [("since", &mut out.0), ("until", &mut out.1)] {
        if let Some(raw) = params.get(key) {
            match normalize_bound(raw) {
                Some(norm) => *slot = Some(norm),
                None => {
                    return Err(openai_error(
                        StatusCode::BAD_REQUEST,
                        format!("unparseable time bound '{key}={raw}' — send RFC-3339 or UTC 'YYYY-MM-DD HH:MM:SS'"),
                        "proxy_dock_bad_query",
                    ))
                }
            }
        }
    }
    Ok(out)
}

/// `GET /api/usage/summary?provider&account&group_by&since&until`.
/// `group_by` in `provider|account|model|day` (account = the
/// `(provider_slug, account_id)` pair — one email on two providers never
/// merges). `?account=` requires `?provider=`. `estimated_cost` is NULL when
/// no pricing snapshot exists (`estimated:false`), never fake `$0.00`.
async fn usage_summary(State(state): State<AppState>, Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    let provider = params.get("provider").cloned().unwrap_or_default();
    let account = params.get("account").cloned().unwrap_or_default();
    if !account.is_empty() && provider.is_empty() {
        return openai_error(
            StatusCode::BAD_REQUEST,
            "?account= requires ?provider= (identity is the provider+account pair)".to_string(),
            "proxy_dock_bad_query",
        );
    }
    let group_by = params.get("group_by").cloned().unwrap_or_default();
    if !group_by.is_empty() && !["provider", "account", "model", "day"].contains(&group_by.as_str()) {
        return openai_error(
            StatusCode::BAD_REQUEST,
            "group_by must be provider|account|model|day".to_string(),
            "proxy_dock_bad_query",
        );
    }
    let (since, until) = match time_bounds(&params) {
        Ok(bounds) => bounds,
        Err(resp) => return resp,
    };
    // Filter fragment shared by totals + groups (all bound params).
    let mut conds: Vec<String> = Vec::new();
    if !provider.is_empty() {
        conds.push("provider_slug = ?".to_string());
    }
    if !account.is_empty() {
        conds.push("account_id = ?".to_string());
    }
    if since.is_some() {
        conds.push("created_at >= ?".to_string());
    }
    if until.is_some() {
        conds.push("created_at <= ?".to_string());
    }
    let where_sql = if conds.is_empty() { String::new() } else { format!("WHERE {}", conds.join(" AND ")) };
    let totals_sql = format!(
        "SELECT COUNT(*), COALESCE(SUM(prompt_tokens),0), COALESCE(SUM(completion_tokens),0), SUM(estimated_cost), COALESCE(SUM(cached_tokens),0), COALESCE(SUM(cache_creation_tokens),0), COALESCE(SUM(cache_savings),0), COUNT(estimated_cost) FROM usage_events {where_sql}"
    );
    let row: (i64, Option<i64>, Option<i64>, Option<f64>, i64, i64, f64, i64) = if provider.is_empty() && account.is_empty() {
        let mut q = sqlx::query_as::<_, (i64, Option<i64>, Option<i64>, Option<f64>, i64, i64, f64, i64)>(&totals_sql);
        if let Some(s) = &since {
            q = q.bind(s);
        }
        if let Some(u) = &until {
            q = q.bind(u);
        }
        q.fetch_one(&state.pool).await.unwrap_or((0, Some(0), Some(0), None, 0, 0, 0.0, 0))
    } else if account.is_empty() {
        let mut q = sqlx::query_as::<_, (i64, Option<i64>, Option<i64>, Option<f64>, i64, i64, f64, i64)>(&totals_sql).bind(&provider);
        if let Some(s) = &since {
            q = q.bind(s);
        }
        if let Some(u) = &until {
            q = q.bind(u);
        }
        q.fetch_one(&state.pool).await.unwrap_or((0, Some(0), Some(0), None, 0, 0, 0.0, 0))
    } else {
        let mut q = sqlx::query_as::<_, (i64, Option<i64>, Option<i64>, Option<f64>, i64, i64, f64, i64)>(&totals_sql)
            .bind(&provider)
            .bind(&account);
        if let Some(s) = &since {
            q = q.bind(s);
        }
        if let Some(u) = &until {
            q = q.bind(u);
        }
        q.fetch_one(&state.pool).await.unwrap_or((0, Some(0), Some(0), None, 0, 0, 0.0, 0))
    };
    let mut doc = serde_json::json!({
        "requests": row.0,
        "prompt_tokens": row.1.unwrap_or(0),
        "completion_tokens": row.2.unwrap_or(0),
        "estimated_cost": row.3,
        "estimated": row.3.is_some(),
        "priced_requests": row.7,
        "unpriced_requests": row.0 - row.7,
        "cached_tokens": row.4,
        "cache_creation_tokens": row.5,
        "cache_savings": row.6,
    });
    if !group_by.is_empty() {
        let key_sql = match group_by.as_str() {
            "provider" => "provider_slug",
            "account" => "provider_slug || '/' || account_id",
            "model" => "model",
            _ => "substr(created_at, 1, 10)",
        };
        let group_q = format!(
            "SELECT {key_sql}, COUNT(*), COALESCE(SUM(prompt_tokens),0), COALESCE(SUM(completion_tokens),0), SUM(estimated_cost), COALESCE(SUM(cached_tokens),0), COALESCE(SUM(cache_creation_tokens),0), COALESCE(SUM(cache_savings),0), COUNT(estimated_cost) FROM usage_events {where_sql} GROUP BY {key_sql} ORDER BY 2 DESC LIMIT 500"
        );
        let groups: Vec<(String, i64, Option<i64>, Option<i64>, Option<f64>, i64, i64, f64, i64)> = if provider.is_empty() && account.is_empty() {
            let mut q = sqlx::query_as::<_, (String, i64, Option<i64>, Option<i64>, Option<f64>, i64, i64, f64, i64)>(&group_q);
            if let Some(s) = &since {
                q = q.bind(s);
            }
            if let Some(u) = &until {
                q = q.bind(u);
            }
            q.fetch_all(&state.pool).await.unwrap_or_default()
        } else if account.is_empty() {
            let mut q = sqlx::query_as::<_, (String, i64, Option<i64>, Option<i64>, Option<f64>, i64, i64, f64, i64)>(&group_q).bind(&provider);
            if let Some(s) = &since {
                q = q.bind(s);
            }
            if let Some(u) = &until {
                q = q.bind(u);
            }
            q.fetch_all(&state.pool).await.unwrap_or_default()
        } else {
            let mut q = sqlx::query_as::<_, (String, i64, Option<i64>, Option<i64>, Option<f64>, i64, i64, f64, i64)>(&group_q)
                .bind(&provider)
                .bind(&account);
            if let Some(s) = &since {
                q = q.bind(s);
            }
            if let Some(u) = &until {
                q = q.bind(u);
            }
            q.fetch_all(&state.pool).await.unwrap_or_default()
        };
        doc["groups"] = serde_json::Value::Array(
            groups
                .into_iter()
                .map(|(key, requests, prompt, completion, cost, cached, creation, savings, priced)| {
                    serde_json::json!({
                        "key": key,
                        "requests": requests,
                        "prompt_tokens": prompt.unwrap_or(0),
                        "completion_tokens": completion.unwrap_or(0),
                        "estimated_cost": cost,
                        "estimated": cost.is_some(),
                        "priced_requests": priced,
                        "unpriced_requests": requests - priced,
                        "cached_tokens": cached,
                        "cache_creation_tokens": creation,
                        "cache_savings": savings,
                    })
                })
                .collect(),
        );
    }
    (StatusCode::OK, Json(doc)).into_response()
}

/// `GET /api/usage/events?provider&account&model&since&until&limit&cursor`.
/// `ORDER BY id DESC`, `limit=min(limit,500)` (default 100), cursor `id<`.
async fn usage_events(State(state): State<AppState>, Query(params): Query<HashMap<String, String>>) -> impl IntoResponse {
    let provider = params.get("provider").cloned().unwrap_or_default();
    let account = params.get("account").cloned().unwrap_or_default();
    let model = params.get("model").cloned().unwrap_or_default();
    if !account.is_empty() && provider.is_empty() {
        return openai_error(
            StatusCode::BAD_REQUEST,
            "?account= requires ?provider= (identity is the provider+account pair)".to_string(),
            "proxy_dock_bad_query",
        );
    }
    let (since, until) = match time_bounds(&params) {
        Ok(bounds) => bounds,
        Err(resp) => return resp,
    };
    let limit: i64 = params.get("limit").and_then(|v| v.parse().ok()).map(|n: i64| n.clamp(1, 500)).unwrap_or(100);
    let cursor: Option<i64> = params.get("cursor").and_then(|v| v.parse().ok());
    let mut conds: Vec<String> = Vec::new();
    if !provider.is_empty() {
        conds.push("provider_slug = ?".to_string());
    }
    if !account.is_empty() {
        conds.push("account_id = ?".to_string());
    }
    if !model.is_empty() {
        conds.push("model = ?".to_string());
    }
    if since.is_some() {
        conds.push("created_at >= ?".to_string());
    }
    if until.is_some() {
        conds.push("created_at <= ?".to_string());
    }
    if cursor.is_some() {
        conds.push("id < ?".to_string());
    }
    let where_sql = if conds.is_empty() { String::new() } else { format!("WHERE {}", conds.join(" AND ")) };
    let sql = format!(
        "SELECT id, provider_slug, account_id, model, prompt_tokens, completion_tokens, cached_tokens, cache_creation_tokens, estimated_cost, cache_savings, created_at FROM usage_events {where_sql} ORDER BY id DESC LIMIT ?"
    );
    // Bind order follows conds order above.
    let mut q = sqlx::query_as::<_, (i64, String, Option<String>, String, i64, i64, i64, i64, Option<f64>, f64, String)>(&sql);
    if !provider.is_empty() {
        q = q.bind(&provider);
    }
    if !account.is_empty() {
        q = q.bind(&account);
    }
    if !model.is_empty() {
        q = q.bind(&model);
    }
    if let Some(s) = &since {
        q = q.bind(s);
    }
    if let Some(u) = &until {
        q = q.bind(u);
    }
    if let Some(c) = cursor {
        q = q.bind(c);
    }
    let rows = q.bind(limit).fetch_all(&state.pool).await.unwrap_or_default();
    let next_cursor = if rows.len() as i64 == limit { rows.last().map(|r| r.0) } else { None };
    let events: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, provider_slug, account_id, model, prompt, completion, cached, creation, cost, savings, created_at)| {
            serde_json::json!({
                "id": id,
                "provider": provider_slug,
                "account": account_id,
                "model": model,
                "prompt_tokens": prompt,
                "completion_tokens": completion,
                "cached_tokens": cached,
                "cache_creation_tokens": creation,
                "estimated_cost": cost,
                "estimated": cost.is_some(),
                "cache_savings": savings,
                "created_at": created_at,
            })
        })
        .collect();
    let mut doc = serde_json::json!({"events": events});
    if let Some(cursor) = next_cursor {
        doc["next_cursor"] = serde_json::Value::from(cursor);
    }
    (StatusCode::OK, Json(doc)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_model_ids_use_commandcode_slug() {
        assert_eq!(
            split_provider_model("commandcode/z-ai/glm-5.3-flash"),
            Some(("commandcode", "z-ai/glm-5.3-flash"))
        );
    }

    #[test]
    fn chat_request_accepts_openai_body() {
        let body = serde_json::json!({
            "model": "chatgpt/gpt-5",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": false,
        });
        assert_eq!(extract_model(&body).as_deref(), Some("chatgpt/gpt-5"));
    }

    #[test]
    fn chat_request_rejects_missing_model() {
        assert_eq!(extract_model(&serde_json::json!({"messages": []})), None);
        assert_eq!(extract_model(&serde_json::json!({"model": ""})), None);
    }

    async fn rolling_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.expect("memory pool");
        // Mirrors migration 005: composite identity (provider_slug, id).
        sqlx::query("CREATE TABLE accounts (id TEXT NOT NULL, provider_slug TEXT NOT NULL, label TEXT NOT NULL, priority INTEGER NOT NULL DEFAULT 1, credential TEXT, plan TEXT, PRIMARY KEY (provider_slug, id))")
            .execute(&pool).await.expect("accounts table");
        sqlx::query("CREATE TABLE routing_policy (provider_slug TEXT NOT NULL, account_id TEXT NOT NULL, priority INTEGER NOT NULL DEFAULT 1, enabled INTEGER NOT NULL DEFAULT 1, PRIMARY KEY (provider_slug, account_id))")
            .execute(&pool).await.expect("policy table");
        sqlx::query("CREATE TABLE usage_events (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, account_id TEXT, model TEXT NOT NULL, prompt_tokens INTEGER NOT NULL DEFAULT 0, completion_tokens INTEGER NOT NULL DEFAULT 0, cached_tokens INTEGER NOT NULL DEFAULT 0, cache_creation_tokens INTEGER NOT NULL DEFAULT 0, estimated_cost REAL, cache_savings REAL NOT NULL DEFAULT 0, cost_source TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(&pool).await.expect("usage table");
        sqlx::query("CREATE TABLE pricing_snapshots (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, model TEXT NOT NULL, source TEXT NOT NULL, price_json TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(&pool).await.expect("pricing table");
        // 'aaa-off' sorts FIRST alphabetically but is disabled: proves the
        // enabled predicate does real work (not alphabetical accident).
        sqlx::query("INSERT INTO accounts (id, provider_slug, label, priority, plan) VALUES ('go-a', 'commandcode', 'a', 2, 'Go'), ('pro-b', 'commandcode', 'b', 1, 'Pro'), ('off-c', 'commandcode', 'c', 0, 'Go'), ('aaa-off', 'commandcode', 'z', 0, 'Go')")
            .execute(&pool).await.expect("seed");
        sqlx::query("INSERT INTO routing_policy (provider_slug, account_id, priority, enabled) VALUES ('commandcode', 'go-a', 0, 1), ('commandcode', 'off-c', 0, 0), ('commandcode', 'aaa-off', 0, 0)")
            .execute(&pool).await.expect("policy seed");
        pool
    }

    #[tokio::test]
    async fn default_account_prefers_routing_priority() {
        let pool = rolling_test_pool().await;
        assert_eq!(default_account_for(&pool, "commandcode").await.as_deref(), Some("go-a"));
        assert_eq!(default_account_for(&pool, "unknown").await, None);
    }

    #[tokio::test]
    async fn same_identity_routes_independently_per_provider() {
        // PK-collision regression: one email on two providers must not
        // clobber — identity is the (provider_slug, id) pair.
        let pool = rolling_test_pool().await;
        sqlx::query("INSERT INTO accounts (id, provider_slug, label, priority, plan) VALUES ('user@example.com', 'chatgpt', 'c', 1, 'Plus'), ('user@example.com', 'antigravity', 'a', 1, 'Pro')")
            .execute(&pool).await.expect("two-provider seed");
        sqlx::query("INSERT INTO routing_policy (provider_slug, account_id, priority, enabled) VALUES ('chatgpt', 'user@example.com', 0, 1), ('antigravity', 'user@example.com', 0, 1)")
            .execute(&pool).await.expect("two-provider policy");
        assert_eq!(default_account_for(&pool, "chatgpt").await.as_deref(), Some("user@example.com"));
        assert_eq!(default_account_for(&pool, "antigravity").await.as_deref(), Some("user@example.com"));
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT provider_slug, id FROM accounts WHERE id = 'user@example.com' ORDER BY provider_slug")
            .fetch_all(&pool).await.expect("both rows survive");
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn fresh_migrations_apply_cleanly() {
        // 001-006 apply on a fresh DB (001-004 checksums untouched).
        // Runtime Migrator (not the compile-time macro) so the test does not
        // depend on macro path-resolution quirks.
        let pool: SqlitePool = SqlitePool::connect("sqlite::memory:").await.expect("memory pool");
        // Migrations live at the workspace root (db.rs uses file-relative
        // "../migrations" from src/ for the same reason).
        let migrator = sqlx::migrate::Migrator::new(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../migrations"))
            .await
            .expect("migrator loads");
        migrator.run(&pool).await.expect("migrations apply");
        let tables: Vec<(String,)> = sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(&pool).await.expect("table list");
        let names: Vec<&str> = tables.iter().map(|(n,)| n.as_str()).collect();
        for required in ["accounts", "routing_policy", "usage_events", "pricing_snapshots", "quota_observations", "model_cache", "gateway_events", "providers"] {
            assert!(names.contains(&required), "missing table {required}");
        }
        // Composite PK + credential purge + new columns/indexes from 005/006.
        let pk: Vec<(String,)> = sqlx::query_as("SELECT sql FROM sqlite_master WHERE name = 'accounts'")
            .fetch_all(&pool).await.expect("accounts ddl");
        assert!(pk[0].0.contains("PRIMARY KEY (provider_slug, id)"), "composite PK missing");
        let cols: Vec<(String,)> = sqlx::query_as("SELECT name FROM pragma_table_info('pricing_snapshots')")
            .fetch_all(&pool).await.expect("pricing cols");
        assert!(cols.iter().any(|(c,)| c == "price_json"), "price_json missing");
        let idx: Vec<(String,)> = sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'index' AND name IN ('idx_usage_q', 'idx_cache_slug', 'idx_events_q', 'idx_pricing_lookup')")
            .fetch_all(&pool).await.expect("index list");
        assert_eq!(idx.len(), 4, "migration indexes missing");
        // Cache legs from 010.
        let usage_cols: Vec<(String,)> = sqlx::query_as("SELECT name FROM pragma_table_info('usage_events')")
            .fetch_all(&pool).await.expect("usage cols");
        for required in ["cached_tokens", "cache_creation_tokens", "cache_savings", "cost_source"] {
            assert!(usage_cols.iter().any(|(c,)| c == required), "usage_events.{required} missing");
        }
    }

    #[tokio::test]
    async fn error_envelope_carries_type_and_numeric_code() {
        // Canonical shape: proxy_dock_* in `type`, numeric HTTP status in
        // `code`, requestId present, `provider` nested inside `error`.
        let resp = openai_error_full(StatusCode::BAD_REQUEST, "nope".to_string(), "proxy_dock_bad_model", Some("opencode"));
        let body = axum::body::to_bytes(resp.into_response().into_body(), 65536).await.expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(value["error"]["type"], "proxy_dock_bad_model");
        assert_eq!(value["error"]["code"], 400);
        assert_eq!(value["error"]["provider"], "opencode");
        assert!(value["error"]["requestId"].as_str().is_some_and(|s| s.starts_with("req_")));
    }

    #[test]
    fn normalize_bound_fixes_lexicographic_timestamps() {
        // RFC-3339 with T/offset normalizes to space-separated UTC; naive
        // space format passes through; garbage is ignored (never mis-filter).
        assert_eq!(normalize_bound("2026-09-27T12:00:00Z").as_deref(), Some("2026-09-27 12:00:00"));
        assert_eq!(normalize_bound("2026-09-27T14:00:00+02:00").as_deref(), Some("2026-09-27 12:00:00"));
        assert_eq!(normalize_bound("2026-09-27 12:00:00").as_deref(), Some("2026-09-27 12:00:00"));
        assert_eq!(normalize_bound("not-a-date"), None);
        assert_eq!(normalize_bound(""), None);
    }

    #[tokio::test]
    async fn provider_nested_routes_resolve_slug() {
        // Regression: provider paths were once built with a static nest()
        // per slug, which strips the prefix without capturing — every
        // `/{slug}/v1/*` handler then 500'd on Path-extractor failure.
        // These go through the real router, so routing regressions fail here.
        // (Authed: the §0 bearer gate now fronts every model route.)
        use tower::ServiceExt;
        let _env = pin_gateway_env(Some("test-gateway-key"));
        let pool: SqlitePool = SqlitePool::connect("sqlite::memory:").await.expect("memory pool");
        let migrator = sqlx::migrate::Migrator::new(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../migrations"))
            .await
            .expect("migrator loads");
        migrator.run(&pool).await.expect("migrations apply");
        sqlx::query("INSERT INTO model_cache (provider_slug, native_id, endpoints, updated_at) VALUES ('antigravity', 'gemini-2.5-flash', '[\"chat\"]', datetime('now'))")
            .execute(&pool).await.expect("cache seed");
        let state = AppState { pool, port: 0, dist_dir: None, catalog_refreshing: Default::default(), inflight: Default::default() };
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/antigravity/v1/models")
                    .header("authorization", "Bearer test-gateway-key")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("models");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 65536).await.expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(value["data"][0]["id"], "gemini-2.5-flash");
        // Unknown slug: honest 404 envelope, never a 500.
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/nope/v1/models")
                    .header("authorization", "Bearer test-gateway-key")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("unknown");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(resp.into_body(), 65536).await.expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(value["error"]["type"], "proxy_dock_unknown_provider");
        // Chat route resolves the slug too: bad body → 400 validation, not 500.
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/antigravity/v1/chat/completions")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer test-gateway-key")
                    .body(axum::body::Body::from(r#"{"model":"x"}"#))
                    .expect("request"),
            )
            .await
            .expect("chat");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// Process-global gateway-key env override is shared by every test in
    /// this binary, so each test that pins it serializes here. Both names
    /// are cleared first — the real vault (possibly holding a dev key) must
    /// never leak into a gate test.
    static GATE_ENV_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn pin_gateway_env(key: Option<&str>) -> std::sync::MutexGuard<'static, ()> {
        let guard = GATE_ENV_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("PROXYDOCK_GATEWAY_KEY");
        std::env::remove_var("PROXYHUB_GATEWAY_KEY");
        if let Some(k) = key {
            std::env::set_var("PROXYDOCK_GATEWAY_KEY", k);
        }
        guard
    }

    fn gate_request(uri: &str, method: Method, key: Option<&str>) -> axum::http::Request<Body> {
        let mut builder = axum::http::Request::builder().uri(uri).method(method);
        if let Some(k) = key {
            builder = builder.header("authorization", format!("Bearer {k}"));
        }
        builder.body(Body::empty()).expect("request")
    }

    async fn gate_error_type(resp: Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 65536).await.expect("body");
        (status, serde_json::from_slice(&body).expect("json"))
    }

    #[test]
    fn gate_path_scope_covers_all_surfaces() {
        assert!(bearer_protected("/v1/models"));
        assert!(bearer_protected("/v1/chat/completions"));
        assert!(bearer_protected("/commandcode/v1/models"));
        assert!(bearer_protected("/opencode/v1/chat/completions"));
        assert!(bearer_protected("/api/providers"));
        assert!(bearer_protected("/api/accounts/opencode/credentials"));
        assert!(bearer_protected("/api"));
        assert!(bearer_protected("/api/health")); // protected but exempt (probe)
        // Health probes stay open; unknown/unrouted paths are not the
        // gate's business (honest 404s below it).
        assert!(bearer_exempt("/health"));
        assert!(bearer_exempt("/api/health"));
        assert!(!bearer_exempt("/api/providers"));
        assert!(!bearer_protected("/health"));
        assert!(!bearer_protected("/"));
        assert!(!bearer_protected("/nope/v1/models"));
        assert!(!bearer_protected("/commandcode/callback"));
    }

    #[tokio::test]
    async fn gate_rejects_missing_and_wrong_keys() {
        // Missing header and wrong key → 401 proxy_dock_unauthorized on
        // every protected surface (unified, provider, control, mutating).
        use tower::ServiceExt;
        let _env = pin_gateway_env(Some("test-gateway-key"));
        let pool = rolling_test_pool().await;
        let state = AppState { pool, port: 0, dist_dir: None, catalog_refreshing: Default::default(), inflight: Default::default() };
        let app = router(state);
        for uri in [
            "/v1/models",
            "/commandcode/v1/models",
            "/api/providers",
            "/api/usage/summary?since=2026-09-01",
            "/api/accounts/commandcode/a/quota/refresh",
        ] {
            for key in [None, Some("wrong-key")] {
                let method = if uri.ends_with("/quota/refresh") { Method::POST } else { Method::GET };
                let (status, value) = gate_error_type(app.clone().oneshot(gate_request(uri, method, key)).await.expect("resp")).await;
                assert_eq!(status, StatusCode::UNAUTHORIZED, "key {key:?} on {uri}");
                assert_eq!(value["error"]["type"], "proxy_dock_unauthorized", "envelope on {uri}");
                assert_eq!(value["error"]["code"], 401, "numeric code on {uri}");
            }
        }
    }

    #[tokio::test]
    async fn gate_passes_right_key_and_leaves_health_open() {
        use tower::ServiceExt;
        let _env = pin_gateway_env(Some("test-gateway-key"));
        let pool = rolling_test_pool().await;
        let state = AppState { pool, port: 0, dist_dir: None, catalog_refreshing: Default::default(), inflight: Default::default() };
        let app = router(state);
        // Right key passes the gate (route's own status, never 401).
        for uri in ["/v1/models", "/commandcode/v1/models", "/api/providers"] {
            let resp = app.clone().oneshot(gate_request(uri, Method::GET, Some("test-gateway-key"))).await.expect("resp");
            assert_ne!(resp.status(), StatusCode::UNAUTHORIZED, "right key on {uri}");
        }
        // Liveness stays open with no key at all.
        for uri in ["/health", "/api/health"] {
            let resp = app.clone().oneshot(gate_request(uri, Method::GET, None)).await.expect("resp");
            assert_eq!(resp.status(), StatusCode::OK, "open {uri}");
        }
        // OPTIONS preflight stays open with no key at all.
        let resp = app
            .clone()
            .oneshot(gate_request("/api/providers", Method::OPTIONS, None))
            .await
            .expect("preflight");
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        // ...while the same path without a key is still 401 outside preflight.
        let (status, _) = gate_error_type(app.clone().oneshot(gate_request("/api/providers", Method::GET, None)).await.expect("resp")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn legacy_db_rebuilds_to_composite_pk() {
        // Existing-DB path: hand-built 001+002 shape (single-column PK,
        // legacy credential value, same id on two providers would collide),
        // then the literal 005 recipe, statement by statement.
        let pool: SqlitePool = SqlitePool::connect("sqlite::memory:").await.expect("memory pool");
        for stmt in [
            "CREATE TABLE providers (slug TEXT PRIMARY KEY, label TEXT NOT NULL)",
            "CREATE TABLE accounts (id TEXT PRIMARY KEY, provider_slug TEXT NOT NULL, label TEXT NOT NULL, priority INTEGER NOT NULL DEFAULT 1)",
            "ALTER TABLE accounts ADD COLUMN credential TEXT",
            "ALTER TABLE accounts ADD COLUMN plan TEXT",
            "CREATE TABLE routing_policy (provider_slug TEXT NOT NULL, account_id TEXT NOT NULL, priority INTEGER NOT NULL DEFAULT 1, enabled INTEGER NOT NULL DEFAULT 1, PRIMARY KEY (provider_slug, account_id))",
            "CREATE TABLE usage_events (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, account_id TEXT, model TEXT NOT NULL, prompt_tokens INTEGER NOT NULL DEFAULT 0, completion_tokens INTEGER NOT NULL DEFAULT 0, estimated_cost REAL, created_at TEXT NOT NULL DEFAULT (datetime('now')))",
            "CREATE TABLE pricing_snapshots (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, model TEXT NOT NULL, source TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT (datetime('now')))",
            "CREATE TABLE quota_observations (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, account_id TEXT, remaining TEXT, reset_at TEXT, observed_at TEXT NOT NULL DEFAULT (datetime('now')))",
            "CREATE TABLE model_cache (provider_slug TEXT NOT NULL, native_id TEXT NOT NULL, context_length INTEGER, PRIMARY KEY (provider_slug, native_id))",
        ] {
            sqlx::query(stmt).execute(&pool).await.expect("old schema");
        }
        // providers is always seeded before any account can exist
        // (db.rs seeds right after migrate!), so the rebuilt FK holds.
        sqlx::query("INSERT INTO providers (slug, label) VALUES ('opencode', 'OpenCode'), ('chatgpt', 'ChatGPT Codex')")
            .execute(&pool).await.expect("providers seed");
        sqlx::query("INSERT INTO accounts (id, provider_slug, label, priority, credential, plan) VALUES ('solo', 'opencode', 'label-kept', 3, 'leaked-secret', 'Go')")
            .execute(&pool).await.expect("legacy row");
        sqlx::query("INSERT INTO routing_policy (provider_slug, account_id, priority, enabled) VALUES ('opencode', 'solo', 2, 1)")
            .execute(&pool).await.expect("legacy policy");
        let recipe = include_str!("../../../migrations/005_accounts_pk.sql");
        // Strip full-line comments; split on ';' (no triggers/procedures here).
        let cleaned: String = recipe
            .lines()
            .filter(|line: &&str| !line.trim_start().starts_with("--"))
            .collect::<Vec<_>>()
            .join("\n");
        for stmt in cleaned.split(';').map(str::trim).filter(|s: &&str| !s.is_empty()) {
            sqlx::query(stmt).execute(&pool).await.expect("005 statement applies to legacy DB");
        }
        // Same identity now coexists per provider; credential purged; label kept.
        sqlx::query("INSERT INTO accounts (id, provider_slug, label, priority, plan) VALUES ('solo', 'chatgpt', 'c', 1, 'Plus')")
            .execute(&pool).await.expect("pair identity");
        let rows: Vec<(String, String, Option<String>, String)> =
            sqlx::query_as("SELECT provider_slug, id, credential, label FROM accounts WHERE id = 'solo' ORDER BY provider_slug")
                .fetch_all(&pool).await.expect("both rows");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(_, _, cred, _)| cred.is_none()), "legacy credential must be NULL");
        assert!(rows.iter().any(|(_, _, _, label)| label == "label-kept"), "user label must survive");
        let policy: Vec<(String,)> = sqlx::query_as("SELECT account_id FROM routing_policy WHERE provider_slug = 'opencode'")
            .fetch_all(&pool).await.expect("policy survives");
        assert_eq!(policy.len(), 1);
    }

    #[tokio::test]
    async fn estimated_cost_null_without_snapshot() {
        let pool = rolling_test_pool().await;
        assert_eq!(estimated_cost_for(&pool, "opencode", "m", 1000, 500, 0, 0).await, (None, None));
        sqlx::query("INSERT INTO pricing_snapshots (provider_slug, model, source, price_json) VALUES ('opencode', 'm', 'manual', '{\"prompt_per_1k\": 1.0, \"completion_per_1k\": 4.0}')")
            .execute(&pool).await.expect("pricing seed");
        assert_eq!(
            estimated_cost_for(&pool, "opencode", "m", 1000, 500, 0, 0).await,
            (Some((3.0, 0.0)), Some("exact".to_string()))
        );
        // Latest-wins on read.
        sqlx::query("INSERT INTO pricing_snapshots (provider_slug, model, source, price_json) VALUES ('opencode', 'm', 'manual', '{\"prompt_per_1k\": 2.0, \"completion_per_1k\": 2.0}')")
            .execute(&pool).await.expect("pricing update");
        assert_eq!(
            estimated_cost_for(&pool, "opencode", "m", 1000, 1000, 0, 0).await,
            (Some((4.0, 0.0)), Some("exact".to_string()))
        );
    }

    #[tokio::test]
    async fn rolling_order_prefers_policy_and_carries_plans() {
        // off-c is disabled -> excluded; go-a wins on policy priority 0.
        let pool = rolling_test_pool().await;
        let ordered: Vec<String> = candidate_accounts(&pool, "commandcode").await.into_iter().map(|c| c.id).collect();
        assert_eq!(ordered, vec!["go-a".to_string(), "pro-b".to_string()]);
        let plans: Vec<Option<String>> = candidate_accounts(&pool, "commandcode").await.into_iter().map(|c| c.plan).collect();
        assert_eq!(plans, vec![Some("Go".to_string()), Some("Pro".to_string())]);
        assert!(candidate_accounts(&pool, "unknown").await.is_empty());
    }

    #[tokio::test]
    async fn put_pricing_backfills_null_costs() {
        // Requests recorded before any price existed carry NULL. Saving a
        // price fills those rows (same rates as new rows) without touching
        // rows that already have a cost.
        let pool = rolling_test_pool().await;
        sqlx::query("INSERT INTO usage_events (provider_slug, account_id, model, prompt_tokens, completion_tokens, estimated_cost) VALUES ('opencode', 'a', 'm', 1000, 500, NULL), ('opencode', 'a', 'm', 1000, 1000, 0.5), ('opencode', 'a', 'other', 1000, 1000, NULL)")
            .execute(&pool).await.expect("usage seed");
        let state = AppState { pool: pool.clone(), port: 0, dist_dir: None, catalog_refreshing: Default::default(), inflight: Default::default() };
        let resp = put_pricing(
            State(state),
            Path(("opencode".to_string(), "m".to_string())),
            Json(serde_json::json!({"prompt_per_1k": 1.0, "completion_per_1k": 4.0, "source": "manual"})),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let costs: Vec<(String, Option<f64>)> =
            sqlx::query_as("SELECT model, estimated_cost FROM usage_events ORDER BY id")
                .fetch_all(&pool).await.expect("costs");
        assert_eq!(costs[0], ("m".to_string(), Some(3.0)));
        assert_eq!(costs[1], ("m".to_string(), Some(0.5)));
        assert_eq!(costs[2], ("other".to_string(), None));
        // New rows use the snapshot too.
        assert_eq!(
            estimated_cost_for(&pool, "opencode", "m", 2000, 0, 0, 0).await,
            (Some((2.0, 0.0)), Some("exact".to_string()))
        );
    }

    #[tokio::test]
    async fn bare_slug_shares_pricing_across_providers() {
        // The same model served via antigravity or commandcode is the same
        // pricing: a vendor-prefixed row serves the bare slug elsewhere, an
        // exact row wins where recorded, and provenance lets exact prices
        // overwrite earlier bare fills.
        let pool = rolling_test_pool().await;
        sqlx::query("INSERT INTO pricing_snapshots (provider_slug, model, source, price_json) VALUES ('commandcode', 'anthropic/claude-opus-5-5', 'official', '{\"prompt_per_1k\": 5.0, \"completion_per_1k\": 25.0}')")
            .execute(&pool).await.expect("cc price");
        assert_eq!(
            estimated_cost_for(&pool, "antigravity", "claude-opus-5-5", 1000, 0, 0, 0).await,
            (Some((5.0, 0.0)), Some("bare".to_string()))
        );
        sqlx::query("INSERT INTO pricing_snapshots (provider_slug, model, source, price_json) VALUES ('antigravity', 'claude-opus-5-5', 'official', '{\"prompt_per_1k\": 1.0, \"completion_per_1k\": 1.0}')")
            .execute(&pool).await.expect("ag price");
        assert_eq!(
            estimated_cost_for(&pool, "antigravity", "claude-opus-5-5", 1000, 0, 0, 0).await,
            (Some((1.0, 0.0)), Some("exact".to_string()))
        );
        // record_usage stores provenance; cross-backfill fills bare rows.
        record_usage(
            &pool,
            "opencode",
            "a",
            "shared-m",
            &serde_json::json!({"prompt_tokens": 1000, "completion_tokens": 0}),
        )
        .await;
        let state = AppState { pool: pool.clone(), port: 0, dist_dir: None, catalog_refreshing: Default::default(), inflight: Default::default() };
        let ok = put_pricing(
            State(state),
            Path(("commandcode".to_string(), "shared-m".to_string())),
            Json(serde_json::json!({"prompt_per_1k": 1.0, "completion_per_1k": 1.0, "source": "official"})),
        )
        .await
        .into_response();
        assert_eq!(ok.status(), StatusCode::OK);
        let row: (Option<f64>, Option<String>) =
            sqlx::query_as("SELECT estimated_cost, cost_source FROM usage_events WHERE provider_slug = 'opencode' AND model = 'shared-m'")
                .fetch_one(&pool).await.expect("shared row");
        assert_eq!(row, (Some(1.0), Some("bare".to_string())));
        // The provider's own later price overwrites the bare fill as exact.
        let state = AppState { pool: pool.clone(), port: 0, dist_dir: None, catalog_refreshing: Default::default(), inflight: Default::default() };
        put_pricing(
            State(state),
            Path(("opencode".to_string(), "shared-m".to_string())),
            Json(serde_json::json!({"prompt_per_1k": 2.0, "completion_per_1k": 2.0, "source": "official"})),
        )
        .await
        .into_response();
        let row: (Option<f64>, Option<String>) =
            sqlx::query_as("SELECT estimated_cost, cost_source FROM usage_events WHERE provider_slug = 'opencode' AND model = 'shared-m'")
                .fetch_one(&pool).await.expect("shared row again");
        assert_eq!(row, (Some(2.0), Some("exact".to_string())));
    }

    #[tokio::test]
    async fn cached_tokens_price_at_cache_rates_with_savings() {
        let pool = rolling_test_pool().await;
        // Snapshot with a discounted cache-read leg: 1000 prompt of which 700
        // cached read + 100 cache creation, 0 completion.
        // Cost = 200/1000*1.0 + 700/1000*0.1 + 100/1000*1.25 = 0.395.
        // Savings = 700/1000*(1.0-0.1) = 0.63 (read leg only, like t3code).
        sqlx::query("INSERT INTO pricing_snapshots (provider_slug, model, source, price_json) VALUES ('opencode', 'm', 'official', '{\"prompt_per_1k\": 1.0, \"completion_per_1k\": 4.0, \"cache_read_per_1k\": 0.1, \"cache_write_per_1k\": 1.25}')")
            .execute(&pool).await.expect("cache price");
        let (priced, source) =
            estimated_cost_for(&pool, "opencode", "m", 1000, 0, 700, 100).await;
        assert_eq!(source, Some("exact".to_string()));
        let (cost, savings) = priced.expect("priced");
        assert!((cost - 0.395).abs() < 1e-9, "cost {cost}");
        assert!((savings - 0.63).abs() < 1e-9, "savings {savings}");
        // record_usage persists the legs alongside the cost.
        record_usage(
            &pool,
            "opencode",
            "a",
            "m",
            &serde_json::json!({
                "prompt_tokens": 1000, "completion_tokens": 0,
                "prompt_tokens_details": {"cached_tokens": 700},
                "cache_creation_input_tokens": 100,
            }),
        )
        .await;
        let row: (i64, i64, i64, Option<f64>, f64) =
            sqlx::query_as("SELECT prompt_tokens, cached_tokens, cache_creation_tokens, estimated_cost, cache_savings FROM usage_events WHERE provider_slug = 'opencode' AND model = 'm'")
                .fetch_one(&pool).await.expect("usage row");
        assert_eq!((row.0, row.1, row.2), (1000, 700, 100));
        assert!((row.3.unwrap() - 0.395).abs() < 1e-9);
        assert!((row.4 - 0.63).abs() < 1e-9);
        // Without cache rates the same row costs the old two-rate math exactly.
        sqlx::query("INSERT INTO pricing_snapshots (provider_slug, model, source, price_json) VALUES ('opencode', 'm2', 'official', '{\"prompt_per_1k\": 1.0, \"completion_per_1k\": 4.0}')")
            .execute(&pool).await.expect("plain price");
        let (priced, _) =
            estimated_cost_for(&pool, "opencode", "m2", 1000, 500, 700, 100).await;
        let (cost, savings) = priced.expect("priced");
        assert!((cost - 3.0).abs() < 1e-9, "fallback cost {cost}");
        assert_eq!(savings, 0.0);
    }

    #[test]
    fn inflight_caps_isolate_commandcode() {
        assert_eq!(inflight_cap_for("commandcode"), INFLIGHT_CAP_COMMANDCODE);
        assert_eq!(inflight_cap_for("opencode"), INFLIGHT_CAP_DEFAULT);
        assert_eq!(INFLIGHT_CAP_COMMANDCODE, 2);
        assert_eq!(INFLIGHT_CAP_DEFAULT, 8);
    }

    #[test]
    fn snippet_redaction_truncates() {
        let long = "x".repeat(600);
        assert_eq!(redact_snippet(&long).chars().count(), 500);
    }

    #[test]
    fn normalize_bound_rejects_pre_2001_epoch() {
        assert_eq!(normalize_bound("999999999"), None);
        assert!(normalize_bound("1759276800").is_some());
    }

    #[tokio::test]
    async fn all_failed_keeps_provider_and_bridge_empty() {
        let err = AdapterError { message: "upstream returned no content".to_string(), status: 502, retryable: true, retry_after: None };
        let resp = all_failed_response("commandcode", 1, 0, 0, Some(err));
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(resp.into_response().into_body(), 65536).await.expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(value["error"]["type"], "proxy_dock_bridge_empty");
        assert_eq!(value["error"]["provider"], "commandcode");
    }

    #[tokio::test]
    async fn busy_response_carries_retry_after() {
        let resp = account_busy_response("commandcode");
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(resp.headers().contains_key("retry-after"));
        let body = axum::body::to_bytes(resp.into_response().into_body(), 65536).await.expect("body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(value["error"]["type"], "proxy_dock_account_busy");
    }
}
