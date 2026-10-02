//! Live model catalog (1A/2E).
//!
//! Sources (none invented):
//! - OpenCode Zen Go: `GET {zen_go_base}/models` (OpenAI list shape
//!   `{data: [{id}]}`; same host that serves `/chat/completions`).
//! - Command Code Provider: `GET {provider_base}/models` (OpenAI list shape;
//!   each entry may carry `supported_endpoints` naming the wires that serve
//!   it, e.g. `/chat/completions`, `/responses`, `/messages`). Tried WITHOUT
//!   a token first — the official endpoint answers publicly (verified live),
//!   so every plan gets a catalog; authenticated per-plan fetch and the
//!   Go-plan honest error remain as fallbacks.
//! - Antigravity: `POST /v1internal:fetchAvailableModels` with `{project}`
//!   (project via `loadCodeAssist`), `User-Agent: antigravity`, prod → daily
//!   → sandbox fallback — the same proven path as the quota module.
//! - ChatGPT/Codex: `GET {codex_base}/models` with the OAuth credential
//!   (Bearer + `ChatGPT-Account-Id`). Community-attested only — see
//!   `adapters::chatgpt::MODELS_PATH` — NOT official OpenAI documentation,
//!   so failures keep the honest "unavailable" reason and the observed-rows
//!   fallback instead of fabricating entries. Models proven by successful
//!   serves are recorded as `observed` rows by note_observed (migration 008)
//!   and served from cache.
//! - Claude: `GET {anthropic_base}/v1/models` (documented `{data: [{id,
//!   display_name}]}` shape) with the stored credential (`x-api-key` for
//!   API keys, Bearer for OAuth subscription tokens).
//!
//! Results persist into `model_cache` (migration 005 columns `endpoints` as a
//! JSON array of `chat|responses|messages`, `updated_at` as UTC
//! `'YYYY-MM-DD HH:MM:SS'`). Entries never come from `BUILTIN_MODELS`.

use sqlx::SqlitePool;

pub const CATALOG_TTL_HOURS: i64 = 24;

/// One cached catalog entry.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CatalogEntry {
    pub native_id: String,
    pub display_name: Option<String>,
    pub endpoints: Vec<String>,
    pub context_length: Option<u32>,
}

/// Parse an OpenAI-style `{data: [{id}]}` body. Unknown shapes yield no
/// entries — never fabricated.
pub fn parse_openai_models(body: &serde_json::Value) -> Vec<CatalogEntry> {
    let items = body
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        out.push(CatalogEntry {
            native_id: id.to_string(),
            display_name: item.get("display_name").or_else(|| item.get("name")).and_then(|v| v.as_str()).map(str::to_string),
            endpoints: vec!["chat".to_string()],
            context_length: item
                .get("context_length")
                .or_else(|| item.get("context_window"))
                .and_then(|v| v.as_u64())
                .map(|n| n as u32),
        });
    }
    out
}

/// Parse a Command Code Provider `{data: [{id, supported_endpoints?}]}` body.
/// Wire names map to gateway wires: anything containing `messages` →
/// `messages`, `responses` → `responses`, otherwise `chat`. Entries without
/// endpoint info default to `chat`.
pub fn parse_provider_models(body: &serde_json::Value) -> Vec<CatalogEntry> {
    let items = body
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        let mut endpoints: Vec<String> = item
            .get("supported_endpoints")
            .and_then(|v| v.as_array())
            .map(|list| {
                list.iter()
                    .filter_map(|e| e.as_str())
                    .map(|e| {
                        let lower = e.to_lowercase();
                        if lower.contains("messages") {
                            "messages".to_string()
                        } else if lower.contains("responses") {
                            "responses".to_string()
                        } else {
                            "chat".to_string()
                        }
                    })
                    .collect()
            })
            .unwrap_or_else(|| vec!["chat".to_string()]);
        endpoints.sort();
        endpoints.dedup();
        out.push(CatalogEntry {
            native_id: id.to_string(),
            display_name: item.get("display_name").or_else(|| item.get("name")).and_then(|v| v.as_str()).map(str::to_string),
            endpoints,
            context_length: item
                .get("context_length")
                .or_else(|| item.get("context_window"))
                .and_then(|v| v.as_u64())
                .map(|n| n as u32),
        });
    }
    out
}

/// Parse an Antigravity `fetchAvailableModels` body (`{models: {id: info}}`).
/// All entries serve the `chat` wire via the generateContent translation.
pub fn parse_ag_models(body: &serde_json::Value) -> Vec<CatalogEntry> {
    let mut out = Vec::new();
    let Some(table) = body.get("models").and_then(|v| v.as_object()) else {
        return out;
    };
    for (id, info) in table {
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        out.push(CatalogEntry {
            native_id: id.to_string(),
            display_name: info.get("displayName").and_then(|v| v.as_str()).map(str::to_string),
            endpoints: vec!["chat".to_string()],
            context_length: info
                .get("maxInputTokens")
                .or_else(|| info.get("context_window"))
                .and_then(|v| v.as_u64())
                .map(|n| n as u32),
        });
    }
    out.sort_by(|a, b| a.native_id.cmp(&b.native_id));
    out
}

fn utc_now_text() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Refresh one provider's catalog behind its local-token gate. Returns the
/// fresh entries (also persisted). Errors are honest reasons, never empty
/// fabrications: no token, unservable plan, or upstream rejection. ChatGPT
/// Codex publishes no model list, so its "refresh" returns the models proven
/// servable through this gateway (observed rows) — possibly empty.
pub async fn refresh_catalog(pool: &SqlitePool, slug: &str, account_id: &str) -> Result<Vec<CatalogEntry>, String> {
    let secret = crate::secrets::read_raw_token(slug, account_id)
        .map_err(|_| "No saved credential found — sign in again.".to_string())?;
    if secret.trim().is_empty() || crate::adapters::commandcode::is_placeholder_token(&secret) {
        return Err("No saved credential found — sign in again.".to_string());
    }
    let entries = match slug {
        "opencode" => refresh_opencode(&secret).await?,
        "commandcode" => refresh_commandcode(pool, &secret, account_id).await?,
        "antigravity" => refresh_antigravity(&secret, account_id).await?,
        "claude" => refresh_claude(&secret, account_id).await?,
        "chatgpt" => refresh_chatgpt(&secret, account_id).await?,
        _ => return Err(format!("unknown provider '{slug}'")),
    };
    store_cache(pool, slug, &entries).await;
    Ok(entries)
}

async fn refresh_opencode(api_key: &str) -> Result<Vec<CatalogEntry>, String> {
    let url = format!("{}/models", crate::adapters::opencode::zen_go_base());
    let resp = http_client()
        .get(&url)
        .bearer_auth(api_key.trim())
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| format!("opencode catalog check failed: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!("opencode catalog check failed ({})", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|err| format!("catalog parse failed: {err}"))?;
    Ok(parse_openai_models(&body))
}

async fn refresh_commandcode(pool: &SqlitePool, api_key: &str, account_id: &str) -> Result<Vec<CatalogEntry>, String> {
    let url = format!("{}/models", crate::adapters::commandcode::provider_api_base());
    // Public catalog first: this official endpoint answers without a token
    // (verified live: 200 + full id/name/context/endpoints list), so every
    // plan — including Go, which has no Provider API serving — gets a real
    // catalog. Per-plan availability is still enforced at serve time.
    if let Ok(entries) = fetch_commandcode_models(&url, None).await {
        if !entries.is_empty() {
            return Ok(entries);
        }
    }
    // Go-plan keys have no Provider API (403): the bridge serves them, and a
    // catalog fetch would only produce an honest rejection.
    let plan: Option<(String,)> = sqlx::query_as("SELECT plan FROM accounts WHERE provider_slug = 'commandcode' AND id = ?")
        .bind(account_id)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);
    if crate::adapters::commandcode::is_go_plan(plan.as_ref().and_then(|(p,)| Some(p.as_str()))) {
        return Err("Go-plan keys have no Provider API catalog — models are resolved from aliases at request time.".to_string());
    }
    fetch_commandcode_models(&url, Some(api_key)).await
}

/// Fetch a Command Code Provider `{data: [{id, …}]}` catalog, optionally
/// authenticated. Base shapes are validated by `parse_provider_models`.
async fn fetch_commandcode_models(url: &str, api_key: Option<&str>) -> Result<Vec<CatalogEntry>, String> {
    let mut req = http_client().get(url).header("Accept", "application/json");
    if let Some(key) = api_key {
        req = req.bearer_auth(key.trim());
    }
    let resp = req
        .send()
        .await
        .map_err(|err| format!("commandcode catalog check failed: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!("commandcode catalog check failed ({})", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|err| format!("catalog parse failed: {err}"))?;
    Ok(parse_provider_models(&body))
}

/// ChatGPT catalog via the subscription-backend models endpoint
/// (community-attested — see `adapters::chatgpt::MODELS_PATH`). Entries
/// serve the `responses` wire natively and the `chat` wire via the gateway's
/// chat->Responses translation, so both are recorded. Failures propagate as
/// honest reasons; the caller serves the observed cache alongside the reason.
/// Like the serving path, one OAuth refresh is attempted on 401 before
/// giving up on the account (persisted back to the keyring).
async fn refresh_chatgpt(secret: &str, account_id: &str) -> Result<Vec<CatalogEntry>, String> {
    use crate::adapters::chatgpt as cg;
    let Some(cred) = cg::parse_serving_cred(secret.trim()) else {
        return Err("No saved credential found — sign in again.".to_string());
    };
    let entries = match cg::fetch_models(&cred).await {
        Ok(entries) => entries,
        Err(err) if err.contains("401") => {
            // Singleflight: concurrent 401s serialize; a waiter re-reads the
            // stored secret first (the leader may have rotated already).
            let lock = crate::oauth::refresh_lock_for("chatgpt", account_id);
            let _guard = lock.lock().await;
            let fresh_secret = crate::secrets::read_raw_token("chatgpt", account_id).unwrap_or_else(|_| secret.to_string());
            if let Some(fresh) = cg::parse_serving_cred(fresh_secret.trim()) {
                if fresh.access_token != cred.access_token {
                    if let Ok(entries) = cg::fetch_models(&fresh).await {
                        return Ok(entries
                            .into_iter()
                            .map(|(native_id, display_name)| CatalogEntry {
                                native_id,
                                display_name,
                                endpoints: vec!["chat".to_string(), "responses".to_string()],
                                context_length: None,
                            })
                            .collect());
                    }
                }
            }
            let live = cg::parse_serving_cred(fresh_secret.trim()).unwrap_or_else(|| cred.clone());
            let Some(refresh) = live.refresh_token.clone().filter(|r| !r.is_empty()) else {
                return Err("ChatGPT rejected the credential — sign in again.".to_string());
            };
            let (access, rotated) = cg::refresh_access_token(&refresh)
                .await
                .map_err(|_| "ChatGPT rejected the credential — sign in again.".to_string())?;
            persist_refreshed_credential("chatgpt", account_id, &fresh_secret, &access, rotated.as_deref())?;
            let retried = cg::ServingCred {
                access_token: access,
                account_id: live.account_id.clone(),
                refresh_token: rotated.or(live.refresh_token.clone()),
            };
            cg::fetch_models(&retried).await.map_err(|err| {
                if err.contains("401") || err.contains("403") {
                    "ChatGPT rejected the credential — sign in again.".to_string()
                } else {
                    err
                }
            })?
        }
        Err(err) => {
            return Err(if err.contains("Invalid client_version") {
                // Upstream only accepts official CLI release versions here;
                // Proxy Dock identifies honestly and won't spoof one, so the
                // catalog stays on models proven by real serves (below).
                "Codex lists models for official CLI versions only — showing models proven by your requests instead.".to_string()
            } else if err.contains("client_version") && err.contains("Field required") {
                // The npm discovery failed, so the field was omitted.
                "The official CLI version couldn't be checked right now — try again shortly.".to_string()
            } else if err.contains("401") || err.contains("403") {
                "ChatGPT rejected the credential — sign in again.".to_string()
            } else {
                err
            })
        }
    };
    Ok(entries
        .into_iter()
        .map(|(native_id, display_name)| CatalogEntry {
            native_id,
            display_name,
            endpoints: vec!["chat".to_string(), "responses".to_string()],
            context_length: None,
        })
        .collect())
}

/// Persist a refreshed OAuth credential (access + rotated refresh) back to
/// the keyring without touching routing metadata. Only handles JSON
/// `StoredCredential` secrets; raw pastes have nowhere to persist a rotation
/// and are left alone (the caller still retries with the fresh token).
fn persist_refreshed_credential(
    slug: &str,
    account_id: &str,
    old_secret: &str,
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<(), String> {
    // Pure merge (last-writer-wins; drops id_token, keeps refresh_token).
    let payload = crate::oauth::refreshed_payload(old_secret, access_token, refresh_token)
        .ok_or_else(|| "stored credential is not OAuth JSON".to_string())?;
    crate::secrets::store_raw_token(slug, account_id, &payload).map_err(|err| err.to_string())
}

/// Claude catalog via the documented Models API, authenticated per
/// credential kind (`x-api-key` for keys, Bearer for OAuth). Entries serve
/// the `messages` wire natively and the `chat` wire via the gateway's
/// chat->Messages translation, so both are recorded. One OAuth refresh on
/// 401, like the serving path.
async fn refresh_claude(secret: &str, account_id: &str) -> Result<Vec<CatalogEntry>, String> {
    use crate::adapters::claude as cl;
    let Some(cred) = cl::parse_serving_cred(secret.trim()) else {
        return Err("No saved credential found — sign in again.".to_string());
    };
    let map_err = |err: String| {
        if err.contains("unauthorized") {
            "Claude rejected the credential — sign in again.".to_string()
        } else {
            err
        }
    };
    let entries = match cl::fetch_models(&cred.access_token, cred.kind).await {
        Ok(entries) => entries,
        Err(err) if err.contains("401") && cred.kind == cl::CredKind::OAuth => {
            // Singleflight around refresh+persist (see chatgpt path).
            let lock = crate::oauth::refresh_lock_for("claude", account_id);
            let _guard = lock.lock().await;
            let fresh_secret = crate::secrets::read_raw_token("claude", account_id).unwrap_or_else(|_| secret.to_string());
            if let Some(fresh) = cl::parse_serving_cred(fresh_secret.trim()) {
                if fresh.access_token != cred.access_token {
                    if let Ok(entries) = cl::fetch_models(&fresh.access_token, fresh.kind).await {
                        return Ok(entries
                            .into_iter()
                            .map(|(native_id, display_name)| CatalogEntry {
                                native_id,
                                display_name,
                                endpoints: vec!["chat".to_string(), "messages".to_string()],
                                context_length: None,
                            })
                            .collect());
                    }
                }
            }
            let live = cl::parse_serving_cred(fresh_secret.trim()).unwrap_or_else(|| cred.clone());
            let Some(refresh) = live.refresh_token.clone().filter(|r| !r.is_empty()) else {
                return Err(map_err(err));
            };
            let (access, rotated) = cl::refresh_access_token(&refresh)
                .await
                .map_err(map_err)?;
            persist_refreshed_credential("claude", account_id, &fresh_secret, &access, rotated.as_deref()).ok();
            cl::fetch_models(&access, cl::CredKind::OAuth).await.map_err(map_err)?
        }
        Err(err) => return Err(map_err(err)),
    };
    Ok(entries
        .into_iter()
        .map(|(native_id, display_name)| CatalogEntry {
            native_id,
            display_name,
            endpoints: vec!["chat".to_string(), "messages".to_string()],
            context_length: None,
        })
        .collect())
}

async fn refresh_antigravity(secret: &str, account_id: &str) -> Result<Vec<CatalogEntry>, String> {
    // Stored OAuth bundles carry JSON; raw pasted tokens pass through.
    let stored = serde_json::from_str::<crate::oauth::StoredCredential>(secret.trim()).ok();
    let token = stored
        .as_ref()
        .map(|c| c.access_token.clone())
        .unwrap_or_else(|| secret.trim().to_string());
    match fetch_antigravity_catalog(&token).await {
        Ok(entries) => Ok(entries),
        Err(err) if err.contains("rejected the credential") => {
            // Singleflight around refresh+persist (see chatgpt path).
            let lock = crate::oauth::refresh_lock_for("antigravity", account_id);
            let _guard = lock.lock().await;
            let fresh_secret = crate::secrets::read_raw_token("antigravity", account_id).unwrap_or_else(|_| secret.to_string());
            let fresh_token = serde_json::from_str::<crate::oauth::StoredCredential>(fresh_secret.trim())
                .ok()
                .map(|c| c.access_token)
                .unwrap_or_else(|| fresh_secret.trim().to_string());
            if fresh_token != token {
                if let Ok(entries) = fetch_antigravity_catalog(&fresh_token).await {
                    return Ok(entries);
                }
            }
            // One OAuth refresh before giving up, mirroring the quota path.
            let refresh = serde_json::from_str::<crate::oauth::StoredCredential>(fresh_secret.trim())
                .ok()
                .and_then(|c| c.refresh_token)
                .unwrap_or_default();
            if refresh.trim().is_empty() {
                return Err(err);
            }
            let tokens = crate::quota::refresh_google_token(refresh.trim())
                .await
                .map_err(|_| "Google rejected the credential — sign in again.".to_string())?;
            persist_refreshed_credential("antigravity", account_id, &fresh_secret, &tokens.access_token, tokens.refresh_token.as_deref())?;
            fetch_antigravity_catalog(&tokens.access_token).await
        }
        Err(err) => Err(err),
    }
}

async fn fetch_antigravity_catalog(access_token: &str) -> Result<Vec<CatalogEntry>, String> {
    use crate::adapters::antigravity as ag;
    let token = access_token.trim();
    let client = ag::serving_client();
    let project = ag::discover_project(&client, &token)
        .await
        .map_err(|err| format!("antigravity project discovery failed: {}", err.message))?;
    let mut last_error = "antigravity catalog exhausted".to_string();
    for host in ag::hosts_for_catalog() {
        let resp = client
            .post(format!("{host}/v1internal:fetchAvailableModels"))
            .bearer_auth(token.trim())
            .header("Content-Type", "application/json")
            .header("User-Agent", "antigravity")
            .json(&serde_json::json!({"project": project}))
            .send()
            .await;
        let resp = match resp {
            Ok(resp) => resp,
            Err(err) => {
                last_error = format!("antigravity catalog request failed: {err}");
                continue;
            }
        };
        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return Err("Google rejected the credential — sign in again.".to_string());
        }
        if !resp.status().is_success() {
            last_error = format!("antigravity catalog check failed ({status})");
            continue;
        }
        let body: serde_json::Value = resp.json().await.map_err(|err| format!("catalog parse failed: {err}"))?;
        return Ok(parse_ag_models(&body));
    }
    Err(last_error)
}

async fn store_cache(pool: &SqlitePool, slug: &str, entries: &[CatalogEntry]) {
    let now = utc_now_text();
    for entry in entries {
        let endpoints = serde_json::to_string(&entry.endpoints).unwrap_or_else(|_| r#"["chat"]"#.to_string());
        let _ = sqlx::query(
            "INSERT INTO model_cache (provider_slug, native_id, context_length, endpoints, updated_at, source) VALUES (?, ?, ?, ?, ?, 'live') \
             ON CONFLICT(provider_slug, native_id) DO UPDATE SET context_length = excluded.context_length, endpoints = excluded.endpoints, updated_at = excluded.updated_at, source = 'live'",
        )
        .bind(slug)
        .bind(&entry.native_id)
        .bind(entry.context_length.map(|n| n as i64))
        .bind(endpoints)
        .bind(&now)
        .execute(pool)
        .await;
    }
}

/// Record a model proven servable by a successful gateway request.
/// Creates an `observed` row (or merges the wire into an existing observed
/// row and refreshes its timestamp). Rows from live provider refreshes
/// (`source = 'live'`) are never touched — the live refresh owns them.
pub async fn note_observed(pool: &SqlitePool, slug: &str, native_id: &str, wire: &str) {
    if !["chat", "responses", "messages"].contains(&wire) {
        return;
    }
    let id = native_id.trim();
    if id.is_empty() || id.chars().count() > 256 {
        return;
    }
    let existing: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT endpoints, source FROM model_cache WHERE provider_slug = ? AND native_id = ?")
            .bind(slug)
            .bind(id)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);
    match existing {
        None => {
            let _ = sqlx::query(
                "INSERT INTO model_cache (provider_slug, native_id, endpoints, updated_at, source) VALUES (?, ?, ?, ?, 'observed')",
            )
            .bind(slug)
            .bind(id)
            .bind(format!(r#"["{wire}"]"#))
            .bind(utc_now_text())
            .execute(pool)
            .await;
        }
        Some((_, source)) if source != "observed" => {}
        Some((endpoints, _)) => {
            let mut wires: Vec<String> = endpoints
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            if !wires.iter().any(|w| w == wire) {
                wires.push(wire.to_string());
                wires.sort();
            }
            let merged = serde_json::to_string(&wires).unwrap_or_else(|_| format!(r#"["{wire}"]"#));
            let _ = sqlx::query(
                "UPDATE model_cache SET endpoints = ?, updated_at = ? WHERE provider_slug = ? AND native_id = ?",
            )
            .bind(merged)
            .bind(utc_now_text())
            .bind(slug)
            .bind(id)
            .execute(pool)
            .await;
        }
    }
}

/// Provenance per cached model id (`live` vs `observed`; absent = `live`).
pub async fn sources_for(pool: &SqlitePool, slug: &str) -> std::collections::HashMap<String, String> {
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT native_id, source FROM model_cache WHERE provider_slug = ?")
            .bind(slug)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    rows.into_iter()
        .map(|(id, source)| (id, source.unwrap_or_else(|| "live".to_string())))
        .collect()
}

/// Read the cached catalog: `(entries, updated_at, stale)`. `stale` is true
/// when the cache is older than the TTL or was never populated.
pub async fn read_cache(pool: &SqlitePool, slug: &str) -> (Vec<CatalogEntry>, Option<String>, bool) {
    let rows: Vec<(String, Option<i64>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT native_id, context_length, endpoints, updated_at FROM model_cache WHERE provider_slug = ? ORDER BY native_id",
    )
    .bind(slug)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    if rows.is_empty() {
        return (Vec::new(), None, true);
    }
    let mut entries = Vec::new();
    let mut newest: Option<String> = None;
    for (native_id, context_length, endpoints, updated_at) in rows {
        let endpoints: Vec<String> = endpoints
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_else(|| vec!["chat".to_string()]);
        if updated_at.as_ref().map(|u| newest.as_ref().map(|n| u > n).unwrap_or(true)).unwrap_or(false) {
            newest = updated_at.clone();
        }
        entries.push(CatalogEntry {
            native_id,
            display_name: None,
            endpoints,
            context_length: context_length.map(|n| n as u32),
        });
    }
    let stale = match newest.clone() {
        Some(ts) => is_stale(&ts),
        None => true,
    };
    (entries, newest, stale)
}

fn is_stale(updated_at: &str) -> bool {
    let parsed = chrono::NaiveDateTime::parse_from_str(updated_at, "%Y-%m-%d %H:%M:%S").ok();
    match parsed {
        Some(ts) => {
            let age = chrono::Utc::now().naive_utc() - ts;
            age.num_hours() >= CATALOG_TTL_HOURS
        }
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_list_parses_ids_only() {
        let body = serde_json::json!({"data": [{"id": "glm-5.1"}, {"id": ""}, {"nope": 1}]});
        let entries = parse_openai_models(&body);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].native_id, "glm-5.1");
        assert_eq!(entries[0].endpoints, vec!["chat".to_string()]);
        assert!(parse_openai_models(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn provider_endpoints_map_to_wires() {
        let body = serde_json::json!({"data": [
            {"id": "a", "supported_endpoints": ["/provider/v1/chat/completions"]},
            {"id": "b", "supported_endpoints": ["/provider/v1/responses"]},
            {"id": "c", "supported_endpoints": ["/provider/v1/messages"]},
            {"id": "d"},
        ]});
        let entries = parse_provider_models(&body);
        assert_eq!(entries[0].endpoints, vec!["chat".to_string()]);
        assert_eq!(entries[1].endpoints, vec!["responses".to_string()]);
        assert_eq!(entries[2].endpoints, vec!["messages".to_string()]);
        assert_eq!(entries[3].endpoints, vec!["chat".to_string()]);
    }

    #[test]
    fn ag_models_parse_object_table() {
        let mut models = serde_json::Map::new();
        models.insert("gemini-3-flash".to_string(), serde_json::json!({"displayName": "Flash"}));
        models.insert(String::new(), serde_json::json!({}));
        let body = serde_json::json!({"models": models});
        let entries = parse_ag_models(&body);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].native_id, "gemini-3-flash");
    }

    #[test]
    fn stale_threshold_is_ttl() {
        assert!(is_stale("2000-01-01 00:00:00"));
        assert!(!is_stale(&utc_now_text()));
        assert!(is_stale("not-a-time"));
    }

    #[tokio::test]
    async fn observed_catalog_merges_without_touching_live() {
        let pool = SqlitePool::connect("sqlite::memory:").await.expect("memory pool");
        sqlx::query("CREATE TABLE model_cache (provider_slug TEXT NOT NULL, native_id TEXT NOT NULL, context_length INTEGER, endpoints TEXT NOT NULL DEFAULT '[\"chat\"]', updated_at TEXT, source TEXT NOT NULL DEFAULT 'live', PRIMARY KEY (provider_slug, native_id))")
            .execute(&pool).await.expect("cache table");
        // A live-listed row exists before any observed traffic.
        sqlx::query("INSERT INTO model_cache (provider_slug, native_id, endpoints, updated_at, source) VALUES ('chatgpt', 'live-m', '[\"chat\"]', '2026-09-28 00:00:00', 'live')")
            .execute(&pool).await.expect("live seed");
        // New model -> observed row with the proven wire; second wire merges.
        note_observed(&pool, "chatgpt", "gpt-5.3-codex", "chat").await;
        note_observed(&pool, "chatgpt", "gpt-5.3-codex", "responses").await;
        // Live rows are never touched; unknown wires are ignored.
        note_observed(&pool, "chatgpt", "live-m", "responses").await;
        note_observed(&pool, "chatgpt", "bogus", "carrier-pigeon").await;
        let rows: Vec<(String, String, String)> =
            sqlx::query_as("SELECT native_id, endpoints, source FROM model_cache ORDER BY native_id")
                .fetch_all(&pool).await.expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "gpt-5.3-codex");
        assert_eq!(rows[0].2, "observed");
        let wires: Vec<String> = serde_json::from_str(&rows[0].1).expect("endpoints json");
        assert_eq!(wires, vec!["chat".to_string(), "responses".to_string()]);
        assert_eq!(rows[1], ("live-m".to_string(), "[\"chat\"]".to_string(), "live".to_string()));
        let sources = sources_for(&pool, "chatgpt").await;
        assert_eq!(sources.get("gpt-5.3-codex").map(String::as_str), Some("observed"));
        assert_eq!(sources.get("live-m").map(String::as_str), Some("live"));
    }

    #[tokio::test]
    async fn commandcode_public_catalog_needs_no_token() {
        // Local stand-in for https://api.commandcode.ai/provider/v1/models
        // (verified live: 200 + full list with no auth): serves the official
        // shape without checking for credentials.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = vec![0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let body = r#"{"object":"list","data":[{"id":"moonshotai/Kimi-K3","name":"Kimi K3","context_length":1000000,"supported_endpoints":["/chat/completions","/responses"]}]}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });
        let url = format!("http://127.0.0.1:{port}/models");
        let entries = fetch_commandcode_models(&url, None).await.expect("public fetch");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].native_id, "moonshotai/Kimi-K3");
        assert_eq!(entries[0].endpoints, vec!["chat".to_string(), "responses".to_string()]);
        assert_eq!(entries[0].context_length, Some(1000000));
    }
}
