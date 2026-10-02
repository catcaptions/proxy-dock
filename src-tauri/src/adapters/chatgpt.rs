//! ChatGPT / Codex serving: OpenAI chat -> Codex Responses translation.
//!
//! Verified against official + proven-proxy sources (none invented):
//! - Upstream base `https://chatgpt.com/backend-api/codex`, primary endpoint
//!   `POST /responses` (SSE), from `openai/codex` (`model-provider-info`,
//!   `core/src/client.rs`, `codex-api/src/endpoint/responses.rs`).
//! - Identity envelope: `Authorization: Bearer <oauth access_token>` +
//!   `ChatGPT-Account-Id` from the id-token claim (official
//!   `bearer_auth_provider.rs`; CLIProxyAPI `jwt_parser.go` +
//!   `codex_executor.go:786-846,1625-1677`).
//! - OAuth flow (auth.openai.com, PKCE, local :1455, `codex_cli_simplified_flow`)
//!   already implemented in `oauth.rs`; refresh already in `quota.rs`.
//! - Chat<->Responses compat shape mirrors the proposed `codex
//!   responses-api-proxy` adapter (chat completions over the Responses backend).
//!
//! This adapter serves the gateway's OpenAI `/chat/completions` surface by
//! translating to Responses upstream and back. Credentials are OAuth
//! `StoredCredential` JSON (or a pasted raw access token with no account id
//! and no refresh — best effort).

use super::{is_auth_status, is_retryable_status, truncate_snippet, AdapterError};

pub const FLOW_NOTE: &str = "ChatGPT/Codex browser OAuth flow. Manual local token only in foundation; no invented OAuth details.";

pub const CODEX_BASE_DEFAULT: &str = "https://chatgpt.com/backend-api/codex";
pub const RESPONSES_PATH: &str = "/responses";
/// Subscription-backend model catalog. Community-attested only (NOT official
/// OpenAI documentation): `codex-backend-sdk` and `chatgpt-codex-proxy`
/// both document `GET /codex/models` ("list models available to this
/// account", OpenAI-shaped objects with a `slug` id, same Bearer +
/// `ChatGPT-Account-Id` auth as `/responses`). The endpoint additionally
/// requires `?client_version=` validated as a CLI release version: per
/// explicit user direction (2026-09-29) it carries the official release
/// discovered from the npm registry (never invented, never our own
/// version). Everything else about the request stays honest proxy-dock
/// identity (`Originator`/`User-Agent`). Parsed defensively; failures
/// surface honestly and the gateway keeps its observed-models fallback.
pub const MODELS_PATH: &str = "/models";
/// npm metadata for the official Codex CLI release (user-directed version
/// discovery for `client_version`). `CODEX_NPM_REGISTRY` overrides the
/// registry root (tests point it at a local mock).
pub const CODEX_NPM_LATEST_DEFAULT: &str = "https://registry.npmjs.org/@openai/codex/latest";
const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

pub fn codex_base() -> String {
    std::env::var("CODEX_API_BASE")
        .ok()
        .map(|base| base.trim_end_matches('/').to_string())
        .filter(|base| !base.is_empty())
        .unwrap_or_else(|| CODEX_BASE_DEFAULT.to_string())
}

pub fn serving_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Parsed serving credential: OAuth bundle or raw pasted access token.
#[derive(Debug, Clone)]
pub struct ServingCred {
    pub access_token: String,
    pub account_id: String,
    pub refresh_token: Option<String>,
}

pub fn parse_serving_cred(secret: &str) -> Option<ServingCred> {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(stored) = serde_json::from_str::<crate::oauth::StoredCredential>(trimmed) {
        if stored.access_token.trim().is_empty() {
            return None;
        }
        return Some(ServingCred {
            access_token: stored.access_token.trim().to_string(),
            account_id: stored.account_id.trim().to_string(),
            refresh_token: stored.refresh_token.clone().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()),
        });
    }
    // Raw pasted token: no account id, no refresh.
    if super::commandcode::is_placeholder_token(trimmed) {
        return None;
    }
    Some(ServingCred { access_token: trimmed.to_string(), account_id: String::new(), refresh_token: None })
}

/// True when the chat body carries image/file parts with no verified
/// Responses mapping (mirrors `claude::has_unsupported_parts` and
/// `antigravity::has_unsupported_parts`). The router rejects these with an
/// honest 400 (never coerced to text silently).
pub fn has_unsupported_parts(body: &serde_json::Value) -> bool {
    let Some(messages) = body.get("messages").and_then(|v| v.as_array()) else {
        return false;
    };
    for msg in messages {
        let Some(content) = msg.get("content") else { continue };
        let items = match content {
            serde_json::Value::Array(items) => items.clone(),
            _ => continue,
        };
        for item in items {
            let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match kind {
                "text" | "input_text" | "output_text" | "refusal" => {}
                _ => {
                    if item.get("text").and_then(|v| v.as_str()).is_none() {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn text_of_content(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match kind {
                    "text" | "input_text" | "output_text" => item.get("text").and_then(|v| v.as_str()).map(str::to_string),
                    _ => item.get("text").and_then(|v| v.as_str()).map(str::to_string),
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// OpenAI chat body -> Codex Responses body.
///
/// Mapping (best-effort, upstream 400s surface honestly):
/// - `system`/`developer` messages -> `instructions` (concatenated).
/// - `user`/`assistant` text -> `input` message items with
///   `content: [{type: input_text, text}]`.
/// - assistant `tool_calls` -> `function_call` output items (best effort).
/// - `tool` role messages -> `function_call_output` items.
/// - `tools` (OpenAI function) -> Responses `tools` (function shape passthrough).
/// - `store: false` always (stateless gateway, mirrors official client).
pub fn build_responses_body(native_model: &str, body: &serde_json::Value) -> serde_json::Value {
    let mut instructions: Vec<String> = Vec::new();
    let mut input: Vec<serde_json::Value> = Vec::new();
    let messages = body.get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for msg in &messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        match role {
            "system" | "developer" => {
                let text = msg.get("content").map(text_of_content).unwrap_or_default();
                if !text.trim().is_empty() {
                    instructions.push(text);
                }
            }
            "user" => {
                let text = msg.get("content").map(text_of_content).unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                input.push(serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": text}],
                }));
            }
            "assistant" => {
                let text = msg.get("content").map(text_of_content).unwrap_or_default();
                if !text.trim().is_empty() {
                    input.push(serde_json::json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}],
                    }));
                }
                if let Some(calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in calls {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let name = tc.get("function").and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let args = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .map(|a| match a {
                                serde_json::Value::String(s) => s.clone(),
                                _ => serde_json::to_string(a).unwrap_or_else(|_| "{}".to_string()),
                            })
                            .unwrap_or_else(|| "{}".to_string());
                        if name.is_empty() && id.is_empty() {
                            continue;
                        }
                        input.push(serde_json::json!({
                            "type": "function_call",
                            "call_id": id,
                            "name": name,
                            "arguments": args,
                        }));
                    }
                }
            }
            "tool" => {
                let text = msg.get("content").map(text_of_content).unwrap_or_default();
                let call_id = msg.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                input.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": text,
                }));
            }
            _ => {}
        }
    }
    let mut out = serde_json::json!({
        "model": native_model,
        "input": input,
        "store": false,
        "stream": body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false),
    });
    if !instructions.is_empty() {
        out["instructions"] = serde_json::Value::String(instructions.join("\n\n"));
    }
    // Tools: OpenAI function shape passes through; Responses accepts
    // `{type: function, name, description, parameters}`. Copy best-effort.
    if let Some(tools) = body.get("tools") {
        let converted: Vec<serde_json::Value> = tools
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|tool| {
                if tool.get("type").and_then(|v| v.as_str()) == Some("function") {
                    let func = tool.get("function").cloned().unwrap_or(serde_json::Value::Null);
                    serde_json::json!({
                        "type": "function",
                        "name": func.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        "description": func.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                        "parameters": func.get("parameters").cloned().unwrap_or(serde_json::Value::Null),
                    })
                } else {
                    tool
                }
            })
            .collect();
        if !converted.is_empty() {
            out["tools"] = serde_json::Value::Array(converted);
        }
    }
    if let Some(choice) = body.get("tool_choice") {
        out["tool_choice"] = choice.clone();
    }
    // Sampling / reasoning hints pass through when present; unknown keys are
    // ignored upstream rather than failing the gateway.
    for key in ["temperature", "top_p", "max_output_tokens", "max_tokens", "reasoning", "parallel_tool_calls", "text"] {
        if let Some(value) = body.get(key) {
            // `max_tokens` (chat) maps to `max_output_tokens` (responses).
            if key == "max_tokens" {
                if out.get("max_output_tokens").is_none() {
                    out["max_output_tokens"] = value.clone();
                }
            } else {
                out[key] = value.clone();
            }
        }
    }
    out
}

fn codex_headers(access_token: &str, account_id: &str, stream: bool) -> reqwest::header::HeaderMap {
    use std::str::FromStr;
    let mut map = reqwest::header::HeaderMap::new();
    let insert = |map: &mut reqwest::header::HeaderMap, name: &str, value: String| {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_str(name),
            reqwest::header::HeaderValue::from_str(&value),
        ) {
            map.insert(name, value);
        }
    };
    insert(&mut map, "Content-Type", "application/json".to_string());
    insert(&mut map, "Authorization", format!("Bearer {}", access_token.trim()));
    if !account_id.trim().is_empty() {
        insert(&mut map, "ChatGPT-Account-Id", account_id.trim().to_string());
    }
    if stream {
        insert(&mut map, "Accept", "text/event-stream".to_string());
    } else {
        insert(&mut map, "Accept", "application/json".to_string());
    }
    // Honest client identity (not impersonating the official CLI).
    insert(&mut map, "Originator", "proxy-dock".to_string());
    insert(&mut map, "User-Agent", "proxy-dock".to_string());
    map
}

/// POST the Responses body. Single attempt — the caller rolls accounts on
/// retryable errors. `refresh_token` is used by the router for one 401 retry
/// (see `refresh_access_token`); this function itself never refreshes.
pub async fn post_responses(
    client: &reqwest::Client,
    cred: &ServingCred,
    native_model: &str,
    body: &serde_json::Value,
    stream: bool,
) -> Result<reqwest::Response, AdapterError> {
    let url = format!("{}{}", codex_base(), RESPONSES_PATH);
    let payload = build_responses_body(native_model, body);
    let resp = client
        .post(&url)
        .headers(codex_headers(&cred.access_token, &cred.account_id, stream))
        .json(&payload)
        .send()
        .await
        .map_err(|err| AdapterError::retryable(format!("codex upstream error: {}", truncate_snippet(&err.to_string(), 1000)), 502))?;
    let status = resp.status().as_u16();
    let retry_after = super::retry_after_secs(resp.headers());
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        let snippet = truncate_snippet(text.trim(), 2000);
        if is_auth_status(status) {
            return Err(AdapterError::terminal(format!("codex API {status}: {snippet}"), status).with_retry_after(retry_after));
        }
        return Err(AdapterError {
            message: format!("codex API {status}: {snippet}"),
            status,
            retryable: is_retryable_status(status),
            retry_after,
        });
    }
    Ok(resp)
}

/// Native Responses passthrough (1B): the client's Responses body is sent
/// untouched (no `store` forcing, no translation) for Codex-native clients.
pub async fn post_responses_raw(
    client: &reqwest::Client,
    cred: &ServingCred,
    raw_body: &serde_json::Value,
    stream: bool,
) -> Result<reqwest::Response, AdapterError> {
    let url = format!("{}{}", codex_base(), RESPONSES_PATH);
    let resp = client
        .post(&url)
        .headers(codex_headers(&cred.access_token, &cred.account_id, stream))
        .json(raw_body)
        .send()
        .await
        .map_err(|err| AdapterError::retryable(format!("codex upstream error: {}", truncate_snippet(&err.to_string(), 1000)), 502))?;
    let status = resp.status().as_u16();
    let retry_after = super::retry_after_secs(resp.headers());
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        let snippet = truncate_snippet(text.trim(), 2000);
        if is_auth_status(status) {
            return Err(AdapterError::terminal(format!("codex API {status}: {snippet}"), status).with_retry_after(retry_after));
        }
        return Err(AdapterError {
            message: format!("codex API {status}: {snippet}"),
            status,
            retryable: is_retryable_status(status),
            retry_after,
        });
    }
    Ok(resp)
}

fn npm_latest_url() -> String {
    std::env::var("CODEX_NPM_REGISTRY")
        .ok()
        .map(|base| base.trim_end_matches('/').to_string())
        .filter(|base| !base.is_empty())
        .map(|base| format!("{base}/@openai/codex/latest"))
        .unwrap_or_else(|| CODEX_NPM_LATEST_DEFAULT.to_string())
}

/// Parse an npm `.../latest` metadata body to its `version`. Strict shape
/// check (numeric `major.minor.patch` core, optional prerelease tail) so
/// upstream never receives garbage — and our own `proxy-dock/x.y.z` identity
/// string can never pass through here (the `/` is rejected).
pub fn parse_npm_version(body: &serde_json::Value) -> Option<String> {
    let version = body.get("version").and_then(|v| v.as_str()).map(str::trim).unwrap_or("");
    if version.is_empty() || !version.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        return None;
    }
    if !version.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return None;
    }
    let core = version.split('-').next().unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    Some(version.to_string())
}

async fn fetch_npm_version() -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = client
        .get(npm_latest_url())
        .header("Accept", "application/json")
        .header("User-Agent", "proxy-dock")
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    parse_npm_version(&body)
}

static CODEX_CLI_VERSION: std::sync::OnceLock<tokio::sync::Mutex<Option<String>>> = std::sync::OnceLock::new();

/// Official Codex CLI release version for the catalog's `client_version`
/// field (user-directed npm-latest discovery, see `MODELS_PATH`). Fetched
/// once per process (bounded 10s); `None` when unreachable/unparseable, in
/// which case callers omit the field and keep the honest fallback. A gateway
/// restart picks up newer releases.
pub async fn codex_cli_version() -> Option<String> {
    let slot = CODEX_CLI_VERSION.get_or_init(|| tokio::sync::Mutex::new(None));
    if let Some(cached) = slot.lock().await.clone() {
        return Some(cached);
    }
    let version = fetch_npm_version().await;
    if let Some(ref v) = version {
        *slot.lock().await = Some(v.clone());
    }
    version
}

/// Fetch the subscription-backend model list with this credential (catalog).
/// Same auth as `/responses` (Bearer + `ChatGPT-Account-Id`). Returns
/// `(id, display_name)` pairs; unknown shapes yield no entries — never
/// fabricated. Upstream rejections keep the response snippet (truncated) so
/// a 400 names its missing field instead of failing opaque. See
/// `MODELS_PATH` for provenance (community-attested).
pub async fn fetch_models(cred: &ServingCred) -> Result<Vec<(String, Option<String>)>, String> {
    let url = format!("{}{}", codex_base(), MODELS_PATH);
    let client = serving_client();
    let mut req = client.get(&url);
    // `client_version` is REQUIRED upstream, validated as a CLI release.
    // Per explicit user direction this is the official release from the npm
    // registry (discovered, never invented); when undiscoverable the field
    // is omitted and the caller keeps the honest fallback.
    if let Some(cli_version) = codex_cli_version().await {
        req = req.query(&[("client_version", cli_version)]);
    }
    let resp = req
        .headers(codex_headers(&cred.access_token, &cred.account_id, false))
        .send()
        .await
        .map_err(|err| format!("codex catalog check failed: {err}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let snippet = truncate_snippet(resp.text().await.unwrap_or_default().trim(), 500);
        return Err(format!("codex catalog check failed ({status}): {snippet}"));
    }
    let body: serde_json::Value = resp.json().await.map_err(|err| format!("catalog parse failed: {err}"))?;
    Ok(parse_models_list(&body))
}

/// Parse the Codex catalog body. Community sources describe `{models:
/// [{slug, ...}]}` with OpenAI-shaped objects; a `{data: [...]}` list is
/// accepted too. Id from `slug` then `id`; display from `display_name` /
/// `name` when present.
pub fn parse_models_list(body: &serde_json::Value) -> Vec<(String, Option<String>)> {
    let items = body
        .get("models")
        .or_else(|| body.get("data"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for item in items {
        let Some(id) = item
            .get("slug")
            .or_else(|| item.get("id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        out.push((
            id.to_string(),
            item.get("display_name")
                .or_else(|| item.get("displayName"))
                .or_else(|| item.get("name"))
                .and_then(|v| v.as_str())
                .map(str::to_string),
        ));
    }
    out
}

/// One OAuth refresh (mirrors `quota::refresh_codex_token`). Returns the new
/// access token (and rotated refresh token, if any).
pub async fn refresh_access_token(refresh_token: &str) -> Result<(String, Option<String>), String> {
    let client = serving_client();
    let resp = client
        .post(CODEX_TOKEN_URL)
        .form(&[("grant_type", "refresh_token"), ("client_id", CODEX_CLIENT_ID), ("refresh_token", refresh_token)])
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| format!("codex token refresh failed: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!("codex token refresh failed ({})", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|err| format!("refresh parse failed: {err}"))?;
    let access = body.get("access_token").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if access.trim().is_empty() {
        return Err("codex refresh returned no access token".to_string());
    }
    let refresh = body.get("refresh_token").and_then(|v| v.as_str()).map(str::to_string);
    Ok((access, refresh))
}

// ---------------------------------------------------------------------------
// Responses -> chat translation
// ---------------------------------------------------------------------------

/// Extract assistant text + tool calls + usage from a complete Responses body
/// (non-streaming `POST /responses`).
pub fn responses_to_chat_fields(body: &serde_json::Value) -> (String, Vec<serde_json::Value>, Option<serde_json::Value>) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<serde_json::Value> = Vec::new();
    let output = body.get("output").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for item in &output {
        let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "message" => {
                if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
                    for part in content {
                        let ptype = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        if ptype == "output_text" {
                            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                if !text.is_empty() {
                                    text_parts.push(text.to_string());
                                }
                            }
                        }
                    }
                }
            }
            "function_call" => {
                let id = item.get("call_id").or_else(|| item.get("id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let args = item.get("arguments").map(|a| match a {
                    serde_json::Value::String(s) => s.clone(),
                    _ => serde_json::to_string(a).unwrap_or_else(|_| "{}".to_string()),
                }).unwrap_or_else(|| "{}".to_string());
                if name.is_empty() && id.is_empty() {
                    continue;
                }
                tool_calls.push(serde_json::json!({
                    "id": if id.is_empty() { format!("call_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]) } else { id },
                    "type": "function",
                    "function": {"name": name, "arguments": args},
                }));
            }
            _ => {}
        }
    }
    let usage = body.get("usage").map(|u| {
        let prompt = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let completion = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let cached = u
            .get("input_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        serde_json::json!({
            "prompt_tokens": prompt,
            "completion_tokens": completion,
            "total_tokens": prompt + completion,
            "prompt_tokens_details": {"cached_tokens": cached},
            "cache_read_input_tokens": cached,
        })
    });
    (text_parts.join(""), tool_calls, usage)
}

/// Build a non-streaming `chat.completion` object from a Responses body.
pub fn responses_to_chat_completion(responses: &serde_json::Value, completion_id: &str, echo_model: &str, created: u64) -> serde_json::Value {
    let (text, tool_calls, usage) = responses_to_chat_fields(responses);
    let mut message = serde_json::json!({"role": "assistant", "content": text});
    if !tool_calls.is_empty() {
        message["tool_calls"] = serde_json::Value::Array(tool_calls);
    }
    serde_json::json!({
        "id": completion_id,
        "object": "chat.completion",
        "created": created,
        "model": echo_model,
        "choices": [{"index": 0, "message": message, "finish_reason": if message.get("tool_calls").is_some() { "tool_calls" } else { "stop" }}],
        "usage": usage.unwrap_or_else(|| serde_json::json!({"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0})),
    })
}

/// Streaming state for Responses SSE -> chat chunks.
#[derive(Debug, Default)]
pub struct CodexSseState {
    pub sent_role: bool,
    pub tool_index_by_call: std::collections::HashMap<String, u32>,
    pub next_tool_index: u32,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
}

/// One Responses SSE `data:` payload -> zero or more OpenAI chat-chunk JSON
/// lines (no framing). Returns the lines plus whether the stream is complete
/// (`response.completed` / `response.failed` / `response.incomplete`).
pub fn translate_responses_data(
    completion_id: &str,
    echo_model: &str,
    created: u64,
    payload: &serde_json::Value,
    state: &mut CodexSseState,
) -> (Vec<String>, bool) {
    let mut lines: Vec<String> = Vec::new();
    let mut done = false;
    let chunk = |delta: serde_json::Value, finish: serde_json::Value| {
        serde_json::json!({
            "id": completion_id, "object": "chat.completion.chunk", "created": created, "model": echo_model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        })
        .to_string()
    };
    let ensure_role = |state: &mut CodexSseState, lines: &mut Vec<String>| {
        if !state.sent_role {
            state.sent_role = true;
            lines.push(chunk(serde_json::json!({"role": "assistant"}), serde_json::Value::Null));
        }
    };
    // Usage may ride alongside any event.
    if let Some(usage) = payload.get("usage").or_else(|| payload.get("response").and_then(|r| r.get("usage"))) {
        if let Some(p) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
            state.prompt_tokens = p;
        }
        if let Some(c) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
            state.completion_tokens = c;
        }
        if let Some(cached) = usage
            .get("input_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_u64())
        {
            state.cached_tokens = cached;
        }
    }
    let kind = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "response.output_text.delta" => {
            let text = payload.get("delta").and_then(|v| v.as_str()).unwrap_or("");
            if !text.is_empty() {
                ensure_role(state, &mut lines);
                lines.push(chunk(serde_json::json!({"content": text}), serde_json::Value::Null));
            }
        }
        "response.function_call_arguments.delta" => {
            let call_id = payload.get("call_id").or_else(|| payload.get("item_id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let delta = payload.get("delta").and_then(|v| v.as_str()).unwrap_or("");
            if !call_id.is_empty() || !delta.is_empty() {
                ensure_role(state, &mut lines);
                let index = *state.tool_index_by_call.entry(call_id.clone()).or_insert_with(|| {
                    let i = state.next_tool_index;
                    state.next_tool_index += 1;
                    i
                });
                lines.push(chunk(
                    serde_json::json!({"tool_calls": [{"index": index, "id": call_id, "type": "function", "function": {"arguments": delta}}]}),
                    serde_json::Value::Null,
                ));
            }
        }
        "response.output_item.added" | "response.output_item.done" => {
            if let Some(item) = payload.get("item") {
                let itype = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if itype == "function_call" {
                    let call_id = item.get("call_id").or_else(|| item.get("id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if !call_id.is_empty() || !name.is_empty() {
                        ensure_role(state, &mut lines);
                        let index = *state.tool_index_by_call.entry(call_id.clone()).or_insert_with(|| {
                            let i = state.next_tool_index;
                            state.next_tool_index += 1;
                            i
                        });
                        lines.push(chunk(
                            serde_json::json!({"tool_calls": [{"index": index, "id": call_id, "type": "function", "function": {"name": name, "arguments": ""}}]}),
                            serde_json::Value::Null,
                        ));
                    }
                }
            }
        }
        "response.completed" | "response.failed" | "response.incomplete" => {
            done = true;
        }
        _ => {}
    }
    (lines, done)
}

/// Surface an upstream stream error, if this Responses SSE payload carries
/// one: `response.failed` / `response.incomplete` (never a clean stop) and
/// `error`-shaped payloads. Returns the terse message; `None` means the
/// payload is ordinary content. The serving path must surface `Some` as a
/// stream error, never a fabricated clean stop.
pub fn responses_stream_error(payload: &serde_json::Value) -> Option<String> {
    let kind = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "response.failed" | "response.incomplete" => {
            let detail = payload
                .get("response")
                .and_then(|r| r.get("status_details"))
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("error").and_then(|v| v.as_str()))
                .unwrap_or(kind);
            Some(format!("codex stream {kind}: {detail}"))
        }
        "error" | "response.error" => {
            let detail = payload
                .get("error")
                .and_then(|e| e.get("message").and_then(|v| v.as_str()).or_else(|| e.as_str()))
                .or_else(|| payload.get("message").and_then(|v| v.as_str()))
                .unwrap_or("codex stream error");
            Some(detail.to_string())
        }
        _ => {
            // Inline error objects without a typed envelope.
            if let Some(err) = payload.get("error") {
                let detail = err
                    .get("message")
                    .and_then(|v| v.as_str())
                    .or_else(|| err.as_str())
                    .unwrap_or("codex stream error");
                return Some(detail.to_string());
            }
            None
        }
    }
}

/// Checked variant of [`translate_responses_data`]: stream errors surface as
/// `Err` (never a fabricated clean stop). Ordinary payloads behave exactly
/// like the unchecked version. New serving code should use this; the legacy
/// wrapper below stays for its current callers.
pub fn translate_responses_data_checked(
    completion_id: &str,
    echo_model: &str,
    created: u64,
    payload: &serde_json::Value,
    state: &mut CodexSseState,
) -> Result<(Vec<String>, bool), String> {
    if let Some(err) = responses_stream_error(payload) {
        return Err(err);
    }
    Ok(translate_responses_data(completion_id, echo_model, created, payload, state))
}

pub fn codex_usage(state: &CodexSseState) -> serde_json::Value {
    serde_json::json!({
        "prompt_tokens": state.prompt_tokens,
        "completion_tokens": state.completion_tokens,
        "total_tokens": state.prompt_tokens + state.completion_tokens,
        "prompt_tokens_details": {"cached_tokens": state.cached_tokens},
        "cache_read_input_tokens": state.cached_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serving_cred_parses_oauth_and_raw() {
        let oauth = serde_json::json!({"access_token": "at", "refresh_token": "rt", "id_token": null, "account_id": "acc", "email": "e", "expires_in": null}).to_string();
        let cred = parse_serving_cred(&oauth).expect("oauth");
        assert_eq!(cred.access_token, "at");
        assert_eq!(cred.account_id, "acc");
        let raw = parse_serving_cred("sk-live-real-token-value").expect("raw");
        assert_eq!(raw.account_id, "");
        assert!(parse_serving_cred("no-key-required").is_none());
        assert!(parse_serving_cred("").is_none());
    }

    #[test]
    fn models_list_parses_slug_shape_only() {
        let body = serde_json::json!({"models": [
            {"slug": "gpt-5.4", "name": "GPT 5.4"},
            {"id": "gpt-5.3-codex"},
            {"slug": ""},
            {"nope": 1},
        ]});
        let entries = parse_models_list(&body);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "gpt-5.4");
        assert_eq!(entries[0].1.as_deref(), Some("GPT 5.4"));
        assert_eq!(entries[1].0, "gpt-5.3-codex");
        // OpenAI-style {data: [...]} is accepted too; unknown shapes are empty.
        let alt = serde_json::json!({"data": [{"id": "gpt-5.4"}]});
        assert_eq!(parse_models_list(&alt).len(), 1);
        assert!(parse_models_list(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn npm_version_parses_strict_semver_only() {
        assert_eq!(
            parse_npm_version(&serde_json::json!({"version": "0.159.1"})),
            Some("0.159.1".to_string())
        );
        assert_eq!(
            parse_npm_version(&serde_json::json!({"version": "0.159.1-beta.1"})),
            Some("0.159.1-beta.1".to_string())
        );
        assert!(parse_npm_version(&serde_json::json!({"version": "1.2"})).is_none());
        assert!(parse_npm_version(&serde_json::json!({"version": ""})).is_none());
        assert!(parse_npm_version(&serde_json::json!({})).is_none());
        // Our own identity string must never pass as a CLI release.
        assert!(parse_npm_version(&serde_json::json!({"version": "proxy-dock/0.1.0"})).is_none());
    }

    #[tokio::test]
    async fn npm_version_fetches_registry_metadata() {
        // Local stand-in for https://registry.npmjs.org/@openai/codex/latest.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = vec![0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let body = r#"{"name":"@openai/codex","version":"0.159.1"}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });
        std::env::set_var("CODEX_NPM_REGISTRY", format!("http://127.0.0.1:{port}"));
        let version = fetch_npm_version().await;
        std::env::remove_var("CODEX_NPM_REGISTRY");
        assert_eq!(version.as_deref(), Some("0.159.1"));
    }

    #[test]
    fn chat_maps_to_responses_stateless() {
        let body = serde_json::json!({
            "model": "chatgpt/gpt-5.4",
            "messages": [
                {"role": "system", "content": "be nice"},
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hey", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]},
                {"role": "tool", "tool_call_id": "c1", "content": "done"},
            ],
            "stream": true,
        });
        let out = build_responses_body("gpt-5.4", &body);
        assert_eq!(out["model"], "gpt-5.4");
        assert_eq!(out["store"], false);
        assert_eq!(out["stream"], true);
        assert_eq!(out["instructions"], "be nice");
        let input = out["input"].as_array().unwrap();
        assert_eq!(input.len(), 4);
        assert_eq!(input[0]["content"][0]["text"], "hi");
        assert_eq!(input[1]["content"][0]["text"], "hey");
    }

    #[test]
    fn responses_maps_to_chat_completion() {
        let upstream = serde_json::json!({
            "output": [
                {"type": "message", "content": [{"type": "output_text", "text": "hello"}]},
                {"type": "function_call", "call_id": "c9", "name": "read", "arguments": "{\"p\":\"x\"}"},
            ],
            "usage": {"input_tokens": 10, "output_tokens": 5},
        });
        let obj = responses_to_chat_completion(&upstream, "chatcmpl-1", "chatgpt/gpt-5", 7);
        assert_eq!(obj["choices"][0]["message"]["content"], "hello");
        assert_eq!(obj["choices"][0]["message"]["tool_calls"][0]["id"], "c9");
        assert_eq!(obj["usage"]["prompt_tokens"], 10);
        assert_eq!(obj["usage"]["completion_tokens"], 5);
    }

    #[test]
    fn responses_cached_input_survives_translation() {
        let upstream = serde_json::json!({
            "output": [],
            "usage": {
                "input_tokens": 100, "output_tokens": 20,
                "input_tokens_details": {"cached_tokens": 40},
            },
        });
        let obj = responses_to_chat_completion(&upstream, "chatcmpl-1", "chatgpt/gpt-5", 7);
        assert_eq!(obj["usage"]["cache_read_input_tokens"], 40);
        // Streaming fold carries the leg too.
        let mut state = CodexSseState::default();
        let (_, done) = translate_responses_data(
            "chatcmpl-1",
            "m",
            7,
            &serde_json::json!({
                "type": "response.completed",
                "response": {"usage": {
                    "input_tokens": 100, "output_tokens": 20,
                    "input_tokens_details": {"cached_tokens": 40},
                }},
            }),
            &mut state,
        );
        assert!(done);
        let usage = codex_usage(&state);
        assert_eq!(usage["prompt_tokens"], 100);
        assert_eq!(usage["cache_read_input_tokens"], 40);
    }

    #[test]
    fn unsupported_parts_detected() {
        let bad = serde_json::json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "hi"}]},
            {"role": "user", "content": [{"type": "image_url", "image_url": {"url": "x"}}]},
        ]});
        assert!(has_unsupported_parts(&bad));
        let file = serde_json::json!({"messages": [
            {"role": "user", "content": [{"type": "file", "file": {"file_data": "x"}}]},
        ]});
        assert!(has_unsupported_parts(&file));
        let ok = serde_json::json!({"messages": [{"role": "user", "content": "hi"}]});
        assert!(!has_unsupported_parts(&ok));
        let ok_parts = serde_json::json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "hi"}]},
        ]});
        assert!(!has_unsupported_parts(&ok_parts));
    }

    #[test]
    fn stream_errors_surface_never_clean_stop() {
        // failed/incomplete surface as errors, not clean stops.
        let failed = serde_json::json!({"type": "response.failed"});
        assert!(responses_stream_error(&failed).is_some());
        let incomplete = serde_json::json!({"type": "response.incomplete"});
        assert!(responses_stream_error(&incomplete).is_some());
        let typed = serde_json::json!({"type": "error", "error": {"message": "boom"}});
        assert_eq!(responses_stream_error(&typed).as_deref(), Some("boom"));
        // Ordinary deltas carry no error.
        let delta = serde_json::json!({"type": "response.output_text.delta", "delta": "hi"});
        assert!(responses_stream_error(&delta).is_none());
        // Checked translate errors instead of fabricating done-with-no-lines.
        let mut state = CodexSseState::default();
        assert!(translate_responses_data_checked("c", "m", 7, &failed, &mut state).is_err());
        let (lines, done) = translate_responses_data("c", "m", 7, &delta, &mut state);
        assert!(!done);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn responses_stream_translates_deltas() {        let mut state = CodexSseState::default();
        let (lines, done) = translate_responses_data(
            "chatcmpl-1",
            "m",
            7,
            &serde_json::json!({"type": "response.output_text.delta", "delta": "hi"}),
            &mut state,
        );
        assert!(!done);
        assert_eq!(lines.len(), 2); // role + content
        let (end_lines, done) =
            translate_responses_data("chatcmpl-1", "m", 7, &serde_json::json!({"type": "response.completed"}), &mut state);
        assert!(done);
        assert!(end_lines.is_empty());
    }
}
