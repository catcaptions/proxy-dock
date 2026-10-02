//! Claude (Anthropic) serving: OpenAI chat -> Anthropic Messages translation.
//!
//! Verified against official + proven-proxy sources (none invented):
//! - Messages API `POST https://api.anthropic.com/v1/messages` with
//!   `x-api-key` + `anthropic-version: 2023-06-01` (official API overview).
//! - Models API `GET /v1/models` returning `{data: [{id, display_name}]}`.
//! - OAuth subscription flow mirrors CLIProxyAPI v7.3.15
//!   (`internal/auth/claude`): authorize `https://claude.ai/oauth/authorize`
//!   (PKCE, `client_id 9d1c250a-e61b-44d9-88ed-5944d1962f5e`, redirect
//!   `http://localhost:54545/callback`, scope `user:profile user:inference
//!   user:sessions:claude_code user:mcp_servers user:file_upload`); code
//!   exchange `POST https://platform.claude.com/v1/oauth/token` (JSON body,
//!   field order grant_type/code/redirect_uri/client_id/code_verifier/state,
//!   axios-shaped headers); refresh posts
//!   `{client_id, grant_type: refresh_token, refresh_token, scope}`; identity
//!   via `GET https://api.anthropic.com/api/oauth/profile`.
//! - OAuth serving is `Authorization: Bearer` on `/v1/messages?beta=true`
//!   with `anthropic-beta` carrying `oauth-2025-04-20` (community header gist
//!   + anthropics/claude-code#40515, which shows
//!   `claude-code-20250219,oauth-2025-04-20` in real CLI traffic).
//! - OAuth quota is `GET https://api.anthropic.com/api/oauth/usage` (Bearer +
//!   `anthropic-beta: oauth-2025-04-20` + `User-Agent: claude-code/<version>`,
//!   which the endpoint requires to avoid aggressive 429s — same rule as the
//!   Antigravity `User-Agent: antigravity` requirement).
//!
//! Honesty rules (no cloaking): the gateway never injects the Claude Code
//! system prompt. OAuth requests on non-Haiku models without a leading
//! Claude Code identity block 400 upstream (anthropics/claude-code#40515) —
//! that error surfaces honestly. Serving is text-only v1 (tool calls are
//! translated; image/file parts get an honest 400 like the Antigravity
//! adapter). Credentials are OAuth `StoredCredential` JSON, `sk-ant-oat-*`
//! setup tokens (OAuth bearer, no refresh), or `sk-ant-*` API keys
//! (`x-api-key`).

use super::{is_auth_status, is_retryable_status, sanitize_tool_schema, truncate_snippet, AdapterError};

pub const FLOW_NOTE: &str = "Claude browser OAuth (subscription) or Anthropic API key. Manual local credential only in foundation; OAuth mirrors CLIProxyAPI's real endpoints.";

pub const ANTHROPIC_BASE_DEFAULT: &str = "https://api.anthropic.com";
pub const MESSAGES_PATH: &str = "/v1/messages";
pub const MODELS_PATH: &str = "/v1/models";
pub const OAUTH_USAGE_PATH: &str = "/api/oauth/usage";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Beta flags sent on OAuth (subscription) requests. Both values are attested
/// in real Claude Code traffic (see module docs); API-key requests send no
/// beta header at all.
pub const OAUTH_BETA: &str = "claude-code-20250219,oauth-2025-04-20";
pub const OAUTH_BETA_SINGLE: &str = "oauth-2025-04-20";
/// User-Agent for the internal OAuth usage endpoint only. The endpoint
/// aggressively rate-limits unknown UAs (community finding, see module docs);
/// serving traffic identifies honestly as `proxy-dock`.
pub const USAGE_USER_AGENT: &str = "claude-code/2.1.77";
pub const SERVING_USER_AGENT: &str = "proxy-dock";
/// `max_tokens` is REQUIRED by the Messages API (documented) — this default
/// applies only when the client sent none.
pub const DEFAULT_MAX_TOKENS: u64 = 1024;

pub fn anthropic_base() -> String {
    std::env::var("ANTHROPIC_API_BASE")
        .ok()
        .map(|base| base.trim_end_matches('/').to_string())
        .filter(|base| !base.is_empty())
        .unwrap_or_else(|| ANTHROPIC_BASE_DEFAULT.to_string())
}

pub fn serving_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredKind {
    ApiKey,
    OAuth,
}

/// Parsed serving credential: OAuth bundle (StoredCredential JSON or
/// `sk-ant-oat-*` setup token) or `sk-ant-*` API key.
#[derive(Debug, Clone)]
pub struct ServingCred {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub kind: CredKind,
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
        // A stored bundle could theoretically wrap an API key paste; keys
        // never start with the OAuth setup-token prefix, so routing on the
        // token shape keeps both working.
        let kind = if stored.access_token.trim().starts_with("sk-ant-") && !stored.access_token.trim().starts_with("sk-ant-oat-") {
            CredKind::ApiKey
        } else {
            CredKind::OAuth
        };
        return Some(ServingCred {
            access_token: stored.access_token.trim().to_string(),
            refresh_token: stored.refresh_token.clone().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()),
            kind,
        });
    }
    if super::commandcode::is_placeholder_token(trimmed) {
        return None;
    }
    if trimmed.starts_with("sk-ant-oat-") {
        return Some(ServingCred { access_token: trimmed.to_string(), refresh_token: None, kind: CredKind::OAuth });
    }
    if trimmed.starts_with("sk-ant-") {
        return Some(ServingCred { access_token: trimmed.to_string(), refresh_token: None, kind: CredKind::ApiKey });
    }
    None
}

fn text_of_content(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                item.get("text").and_then(|v| v.as_str()).map(str::to_string)
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// True when the chat body carries image/file parts with no verified
/// Messages mapping (mirrors `ag::has_unsupported_parts`).
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

/// OpenAI chat body -> Anthropic Messages body.
///
/// Mapping (best-effort, upstream 400s surface honestly):
/// - `system`/`developer` messages -> top-level `system` string (joined).
/// - `user` text -> `{role: user, content: <text>}`; `assistant` text ->
///   `{role: assistant, content: <text>}`; assistant `tool_calls` ->
///   `tool_use` blocks; `tool` messages -> `tool_result` blocks in a user
///   turn (Anthropic requires tool results in user role).
/// - `tools` (OpenAI function) -> `{name, description, input_schema}`.
/// - `max_tokens` defaults to 1024 (REQUIRED upstream — documented).
pub fn build_messages_body(native_model: &str, body: &serde_json::Value) -> serde_json::Value {
    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<serde_json::Value> = Vec::new();
    let items = body.get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for msg in &items {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        match role {
            "system" | "developer" => {
                let text = msg.get("content").map(text_of_content).unwrap_or_default();
                if !text.trim().is_empty() {
                    system_parts.push(text);
                }
            }
            "user" => {
                let text = msg.get("content").map(text_of_content).unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                messages.push(serde_json::json!({"role": "user", "content": text}));
            }
            "assistant" => {
                let text = msg.get("content").map(text_of_content).unwrap_or_default();
                let mut blocks: Vec<serde_json::Value> = Vec::new();
                if !text.trim().is_empty() {
                    blocks.push(serde_json::json!({"type": "text", "text": text}));
                }
                if let Some(calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in calls {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let name = tc.get("function").and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let input = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .map(|a| match a {
                                serde_json::Value::String(s) => serde_json::from_str(s).unwrap_or(serde_json::json!({})),
                                _ => a.clone(),
                            })
                            .unwrap_or_else(|| serde_json::json!({}));
                        if name.is_empty() && id.is_empty() {
                            continue;
                        }
                        blocks.push(serde_json::json!({
                            "type": "tool_use",
                            "id": if id.is_empty() { format!("toolu_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]) } else { id },
                            "name": name,
                            "input": input,
                        }));
                    }
                }
                if blocks.is_empty() {
                    continue;
                }
                messages.push(serde_json::json!({"role": "assistant", "content": blocks}));
            }
            "tool" => {
                let text = msg.get("content").map(text_of_content).unwrap_or_default();
                let tool_use_id = msg.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{"type": "tool_result", "tool_use_id": tool_use_id, "content": text}],
                }));
            }
            _ => {}
        }
    }
    let mut out = serde_json::json!({
        "model": native_model,
        "messages": messages,
        "max_tokens": body.get("max_tokens").and_then(|v| v.as_u64()).or_else(|| body.get("max_completion_tokens").and_then(|v| v.as_u64())).unwrap_or(DEFAULT_MAX_TOKENS),
    });
    if !system_parts.is_empty() {
        out["system"] = serde_json::Value::String(system_parts.join("\n\n"));
    }
    if let Some(tools) = body.get("tools") {
        let converted: Vec<serde_json::Value> = tools
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|tool| {
                if tool.get("type").and_then(|v| v.as_str()) == Some("function") {
                    let func = tool.get("function").cloned().unwrap_or(serde_json::Value::Null);
                    Some(serde_json::json!({
                        "name": func.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        "description": func.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                        "input_schema": sanitize_tool_schema(
                            &func.get("parameters").cloned().unwrap_or_else(|| serde_json::json!({"type": "object"}))
                        ),
                    }))
                } else {
                    None
                }
            })
            .collect();
        if !converted.is_empty() {
            out["tools"] = serde_json::Value::Array(converted);
        }
    }
    if let Some(choice) = body.get("tool_choice") {
        out["tool_choice"] = map_tool_choice(choice);
    }
    for key in ["temperature", "top_p", "top_k", "stop_sequences", "stop_sequence"] {
        if let Some(value) = body.get(key) {
            // OpenAI `stop` (string|string[]) maps to `stop_sequences`.
            if key == "stop_sequence" || key == "stop_sequences" {
                if out.get("stop_sequences").is_none() {
                    out["stop_sequences"] = match value {
                        serde_json::Value::String(s) => serde_json::json!([s]),
                        _ => value.clone(),
                    };
                }
            } else {
                out[key] = value.clone();
            }
        }
    }
    if let Some(stop) = body.get("stop") {
        if out.get("stop_sequences").is_none() {
            out["stop_sequences"] = match stop {
                serde_json::Value::String(s) => serde_json::json!([s]),
                _ => stop.clone(),
            };
        }
    }
    out
}

fn map_tool_choice(choice: &serde_json::Value) -> serde_json::Value {
    match choice {
        serde_json::Value::String(s) => match s.as_str() {
            "none" => serde_json::json!({"type": "none"}),
            "required" => serde_json::json!({"type": "any"}),
            _ => serde_json::json!({"type": "auto"}),
        },
        serde_json::Value::Object(_) => {
            let name = choice
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str())
                .or_else(|| choice.get("name").and_then(|v| v.as_str()))
                .unwrap_or("");
            if name.is_empty() {
                serde_json::json!({"type": "auto"})
            } else {
                serde_json::json!({"type": "tool", "name": name})
            }
        }
        _ => serde_json::json!({"type": "auto"}),
    }
}

fn serving_headers(cred: &ServingCred, stream: bool) -> reqwest::header::HeaderMap {
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
    insert(&mut map, "anthropic-version", ANTHROPIC_VERSION.to_string());
    insert(&mut map, "User-Agent", SERVING_USER_AGENT.to_string());
    match cred.kind {
        CredKind::ApiKey => {
            insert(&mut map, "x-api-key", cred.access_token.trim().to_string());
        }
        CredKind::OAuth => {
            insert(&mut map, "Authorization", format!("Bearer {}", cred.access_token.trim()));
            insert(&mut map, "anthropic-beta", OAUTH_BETA.to_string());
        }
    }
    if stream {
        insert(&mut map, "Accept", "text/event-stream".to_string());
    } else {
        insert(&mut map, "Accept", "application/json".to_string());
    }
    // Honest client identity (not impersonating the Claude Code CLI).
    insert(&mut map, "Originator", "proxy-dock".to_string());
    map
}

/// POST the translated Messages body. Single attempt — the caller rolls
/// accounts on retryable errors and refreshes OAuth on 401.
pub async fn post_messages(
    client: &reqwest::Client,
    cred: &ServingCred,
    native_model: &str,
    body: &serde_json::Value,
    stream: bool,
) -> Result<reqwest::Response, AdapterError> {
    let mut url = format!("{}{}", anthropic_base(), MESSAGES_PATH);
    if cred.kind == CredKind::OAuth {
        url.push_str("?beta=true");
    }
    let payload = build_messages_body(native_model, body);
    let resp = client
        .post(&url)
        .headers(serving_headers(cred, stream))
        .json(&payload)
        .send()
        .await
        .map_err(|err| AdapterError::retryable(format!("claude upstream error: {}", truncate_snippet(&err.to_string(), 1000)), 502))?;
    let status = resp.status().as_u16();
    let retry_after = super::retry_after_secs(resp.headers());
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        let snippet = truncate_snippet(text.trim(), 2000);
        if is_auth_status(status) {
            return Err(AdapterError::terminal(format!("claude API {status}: {snippet}"), status).with_retry_after(retry_after));
        }
        return Err(AdapterError {
            message: format!("claude API {status}: {snippet}"),
            status,
            retryable: is_retryable_status(status),
            retry_after,
        });
    }
    Ok(resp)
}

/// Native Messages passthrough: the client's Messages body goes untouched
/// (only the URL gains `?beta=true` for OAuth creds). For key creds the
/// documented endpoint shape is used as-is.
pub async fn post_messages_raw(
    client: &reqwest::Client,
    cred: &ServingCred,
    raw_body: &serde_json::Value,
    stream: bool,
) -> Result<reqwest::Response, AdapterError> {
    let mut url = format!("{}{}", anthropic_base(), MESSAGES_PATH);
    if cred.kind == CredKind::OAuth {
        url.push_str("?beta=true");
    }
    let resp = client
        .post(&url)
        .headers(serving_headers(cred, stream))
        .json(raw_body)
        .send()
        .await
        .map_err(|err| AdapterError::retryable(format!("claude upstream error: {}", truncate_snippet(&err.to_string(), 1000)), 502))?;
    let status = resp.status().as_u16();
    let retry_after = super::retry_after_secs(resp.headers());
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        let snippet = truncate_snippet(text.trim(), 2000);
        if is_auth_status(status) {
            return Err(AdapterError::terminal(format!("claude API {status}: {snippet}"), status).with_retry_after(retry_after));
        }
        return Err(AdapterError {
            message: format!("claude API {status}: {snippet}"),
            status,
            retryable: is_retryable_status(status),
            retry_after,
        });
    }
    Ok(resp)
}

/// One OAuth refresh (`POST platform.claude.com/v1/oauth/token`, mirroring
/// CLIProxyAPI). Returns the new access token (and rotated refresh, if any).
pub async fn refresh_access_token(refresh_token: &str) -> Result<(String, Option<String>), String> {
    crate::oauth::refresh_claude_token(refresh_token).await
}

/// Fetch the Models API list with this credential (catalog + key verify).
/// OAuth uses Bearer; keys use `x-api-key`. Both hit the documented
/// endpoint — no invented URLs. `limit=1000` (the documented maximum) so a
/// single page carries the whole catalog instead of the default first 20.
pub async fn fetch_models(api_key: &str, kind: CredKind) -> Result<Vec<(String, Option<String>)>, String> {
    let url = format!("{}{}?limit=1000", anthropic_base(), MODELS_PATH);
    let client = serving_client();
    let mut req = client
        .get(&url)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("Accept", "application/json")
        .header("User-Agent", SERVING_USER_AGENT);
    req = match kind {
        CredKind::ApiKey => req.header("x-api-key", api_key.trim()),
        CredKind::OAuth => req.bearer_auth(api_key.trim()),
    };
    let resp = req.send().await.map_err(|err| format!("claude models check failed: {err}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED || resp.status() == reqwest::StatusCode::FORBIDDEN {
        return Err("claude rejected the credential (unauthorized) — check it and try again".to_string());
    }
    if !resp.status().is_success() {
        return Err(format!("claude models check failed ({})", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|err| format!("models parse failed: {err}"))?;
    Ok(parse_models_list(&body))
}

/// Parse a documented `{data: [{id, display_name?}]}` Models body. Unknown
/// shapes yield no entries — never fabricated.
pub fn parse_models_list(body: &serde_json::Value) -> Vec<(String, Option<String>)> {
    let items = body.get("data").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut out = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        out.push((
            id.to_string(),
            item.get("display_name").or_else(|| item.get("displayName")).or_else(|| item.get("name")).and_then(|v| v.as_str()).map(str::to_string),
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// Messages -> chat translation
// ---------------------------------------------------------------------------

/// Anthropic `stop_reason` -> OpenAI `finish_reason`.
pub fn finish_reason(stop_reason: Option<&str>) -> &'static str {
    match stop_reason {
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        _ => "stop",
    }
}

/// Extract assistant text + tool calls + usage from a complete Messages body
/// (non-streaming `POST /v1/messages`).
pub fn messages_to_chat_fields(body: &serde_json::Value) -> (String, Vec<serde_json::Value>, Option<serde_json::Value>) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut tool_calls: Vec<serde_json::Value> = Vec::new();
    let content = body.get("content").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for block in &content {
        let kind = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "text" => {
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        text_parts.push(text.to_string());
                    }
                }
            }
            "tool_use" => {
                let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let input = block.get("input").cloned().unwrap_or_else(|| serde_json::json!({}));
                if name.is_empty() && id.is_empty() {
                    continue;
                }
                tool_calls.push(serde_json::json!({
                    "id": if id.is_empty() { format!("call_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]) } else { id },
                    "type": "function",
                    "function": {"name": name, "arguments": serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string())},
                }));
            }
            _ => {}
        }
    }
    let usage = body.get("usage").map(|u| {
        let prompt = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let completion = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let cached = u.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let creation = u.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        serde_json::json!({
            "prompt_tokens": prompt,
            "completion_tokens": completion,
            "total_tokens": prompt + completion,
            "prompt_tokens_details": {"cached_tokens": cached},
            "cache_read_input_tokens": cached,
            "cache_creation_input_tokens": creation,
        })
    });
    (text_parts.join(""), tool_calls, usage)
}

/// Build a non-streaming `chat.completion` object from a Messages body.
pub fn messages_to_chat_completion(messages: &serde_json::Value, completion_id: &str, echo_model: &str, created: u64) -> serde_json::Value {
    let (text, tool_calls, usage) = messages_to_chat_fields(messages);
    let reason = if tool_calls.is_empty() {
        finish_reason(messages.get("stop_reason").and_then(|v| v.as_str()))
    } else {
        "tool_calls"
    };
    let mut message = serde_json::json!({"role": "assistant", "content": text});
    if !tool_calls.is_empty() {
        message["tool_calls"] = serde_json::Value::Array(tool_calls);
    }
    serde_json::json!({
        "id": completion_id,
        "object": "chat.completion",
        "created": created,
        "model": echo_model,
        "choices": [{"index": 0, "message": message, "finish_reason": reason}],
        "usage": usage.unwrap_or_else(|| serde_json::json!({"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0})),
    })
}

/// Streaming state for Messages SSE -> chat chunks.
#[derive(Debug, Default)]
pub struct ClaudeSseState {
    pub sent_role: bool,
    pub tool_index_by_block: std::collections::HashMap<u32, u32>,
    pub next_tool_index: u32,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub cache_creation_tokens: u64,
}

/// One Messages SSE `data:` payload -> zero or more OpenAI chat-chunk JSON
/// lines (no framing), plus whether the stream is complete (`message_stop`).
pub fn translate_messages_data(
    completion_id: &str,
    echo_model: &str,
    created: u64,
    payload: &serde_json::Value,
    state: &mut ClaudeSseState,
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
    let ensure_role = |state: &mut ClaudeSseState, lines: &mut Vec<String>| {
        if !state.sent_role {
            state.sent_role = true;
            lines.push(chunk(serde_json::json!({"role": "assistant"}), serde_json::Value::Null));
        }
    };
    let fold_usage = |state: &mut ClaudeSseState, usage: &serde_json::Value| {
        if let Some(p) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
            state.prompt_tokens = p;
        }
        if let Some(c) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
            state.completion_tokens = c;
        }
        if let Some(cached) = usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()) {
            state.cached_tokens = cached;
        }
        if let Some(creation) = usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()) {
            state.cache_creation_tokens = creation;
        }
    };
    let kind = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "message_start" => {
            if let Some(usage) = payload.get("message").and_then(|m| m.get("usage")) {
                fold_usage(state, usage);
            }
        }
        "content_block_start" => {
            let index = payload.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            if let Some(block) = payload.get("content_block") {
                let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if btype == "tool_use" {
                    let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if !id.is_empty() || !name.is_empty() {
                        ensure_role(state, &mut lines);
                        let tool_index = *state.tool_index_by_block.entry(index).or_insert_with(|| {
                            let i = state.next_tool_index;
                            state.next_tool_index += 1;
                            i
                        });
                        lines.push(chunk(
                            serde_json::json!({"tool_calls": [{"index": tool_index, "id": id, "type": "function", "function": {"name": name, "arguments": ""}}]}),
                            serde_json::Value::Null,
                        ));
                    }
                }
            }
        }
        "content_block_delta" => {
            let index = payload.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            if let Some(delta) = payload.get("delta") {
                let dtype = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match dtype {
                    "text_delta" => {
                        let text = delta.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        if !text.is_empty() {
                            ensure_role(state, &mut lines);
                            lines.push(chunk(serde_json::json!({"content": text}), serde_json::Value::Null));
                        }
                    }
                    "input_json_delta" => {
                        let partial = delta.get("partial_json").and_then(|v| v.as_str()).unwrap_or("");
                        if !partial.is_empty() {
                            ensure_role(state, &mut lines);
                            let tool_index = *state.tool_index_by_block.entry(index).or_insert_with(|| {
                                let i = state.next_tool_index;
                                state.next_tool_index += 1;
                                i
                            });
                            lines.push(chunk(
                                serde_json::json!({"tool_calls": [{"index": tool_index, "function": {"arguments": partial}}]}),
                                serde_json::Value::Null,
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        "message_delta" => {
            if let Some(usage) = payload.get("usage") {
                fold_usage(state, usage);
            }
        }
        "message_stop" => {
            done = true;
        }
        _ => {}
    }
    (lines, done)
}

/// Surface an upstream stream error, if this Messages SSE payload carries
/// one: `type: error` envelopes (never ignored, never a fabricated clean
/// stop). Returns the terse message; `None` means ordinary content. The
/// serving path must surface `Some` as a stream error.
pub fn messages_stream_error(payload: &serde_json::Value) -> Option<String> {
    let kind = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if kind == "error" {
        let detail = payload
            .get("error")
            .and_then(|e| e.get("message").and_then(|v| v.as_str()).or_else(|| e.as_str()))
            .or_else(|| payload.get("message").and_then(|v| v.as_str()))
            .unwrap_or("claude stream error");
        return Some(detail.to_string());
    }
    // Inline error objects without the typed envelope.
    if kind.is_empty() {
        if let Some(err) = payload.get("error") {
            let detail = err
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| err.as_str())
                .unwrap_or("claude stream error");
            return Some(detail.to_string());
        }
    }
    None
}

/// Checked variant of [`translate_messages_data`]: stream errors surface as
/// `Err` (never swallowed into a clean stop). Ordinary payloads behave
/// exactly like the unchecked version. New serving code should use this; the
/// legacy wrapper below stays for its current callers.
pub fn translate_messages_data_checked(
    completion_id: &str,
    echo_model: &str,
    created: u64,
    payload: &serde_json::Value,
    state: &mut ClaudeSseState,
) -> Result<(Vec<String>, bool), String> {
    if let Some(err) = messages_stream_error(payload) {
        return Err(err);
    }
    Ok(translate_messages_data(completion_id, echo_model, created, payload, state))
}

pub fn claude_usage(state: &ClaudeSseState) -> serde_json::Value {
    serde_json::json!({
        "prompt_tokens": state.prompt_tokens,
        "completion_tokens": state.completion_tokens,
        "total_tokens": state.prompt_tokens + state.completion_tokens,
        "prompt_tokens_details": {"cached_tokens": state.cached_tokens},
        "cache_read_input_tokens": state.cached_tokens,
        "cache_creation_input_tokens": state.cache_creation_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serving_cred_routes_on_token_shape() {
        let oauth = serde_json::json!({"access_token": "sk-ant-oat-abc", "refresh_token": "rt", "id_token": null, "account_id": "acc", "email": "e", "expires_in": null}).to_string();
        let cred = parse_serving_cred(&oauth).expect("oauth bundle");
        assert_eq!(cred.kind, CredKind::OAuth);
        assert_eq!(cred.refresh_token.as_deref(), Some("rt"));
        let setup = parse_serving_cred("sk-ant-oat-setup-token-value").expect("setup token");
        assert_eq!(setup.kind, CredKind::OAuth);
        assert!(setup.refresh_token.is_none());
        let key = parse_serving_cred("sk-ant-api03-key-value").expect("api key");
        assert_eq!(key.kind, CredKind::ApiKey);
        assert!(parse_serving_cred("no-key-required").is_none());
        assert!(parse_serving_cred("").is_none());
    }

    #[test]
    fn chat_maps_to_messages_with_required_max_tokens() {
        let body = serde_json::json!({
            "model": "claude/claude-sonnet-4-6",
            "messages": [
                {"role": "system", "content": "be nice"},
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hey", "tool_calls": [{"id": "t1", "type": "function", "function": {"name": "read", "arguments": "{\"p\":\"x\"}"}}]},
                {"role": "tool", "tool_call_id": "t1", "content": "done"},
            ],
            "tools": [{"type": "function", "function": {"name": "read", "description": "d", "parameters": {"type": "object"}}}],
            "tool_choice": "required",
        });
        let out = build_messages_body("claude-sonnet-4-6", &body);
        assert_eq!(out["model"], "claude-sonnet-4-6");
        assert_eq!(out["system"], "be nice");
        // max_tokens is REQUIRED upstream: the default fills it honestly.
        assert_eq!(out["max_tokens"], DEFAULT_MAX_TOKENS);
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        let tool_use = &messages[1]["content"].as_array().unwrap()[1];
        assert_eq!(tool_use["type"], "tool_use");
        assert_eq!(tool_use["input"]["p"], "x");
        let result = &messages[2]["content"].as_array().unwrap()[0];
        assert_eq!(result["type"], "tool_result");
        assert_eq!(out["tools"][0]["input_schema"]["type"], "object");
        assert_eq!(out["tool_choice"]["type"], "any");
    }

    #[test]
    fn messages_maps_to_chat_completion_with_cache_legs() {
        let upstream = serde_json::json!({
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "tool_use", "id": "t9", "name": "read", "input": {"p": "x"}},
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 40, "cache_creation_input_tokens": 10},
        });
        let obj = messages_to_chat_completion(&upstream, "chatcmpl-1", "claude/claude-sonnet-4-6", 7);
        assert_eq!(obj["choices"][0]["message"]["content"], "hello");
        assert_eq!(obj["choices"][0]["message"]["tool_calls"][0]["id"], "t9");
        assert_eq!(obj["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(obj["usage"]["prompt_tokens"], 100);
        assert_eq!(obj["usage"]["cache_read_input_tokens"], 40);
        assert_eq!(obj["usage"]["cache_creation_input_tokens"], 10);
    }

    #[test]
    fn messages_stop_reasons_map() {
        assert_eq!(finish_reason(Some("max_tokens")), "length");
        assert_eq!(finish_reason(Some("end_turn")), "stop");
        assert_eq!(finish_reason(None), "stop");
    }

    #[test]
    fn messages_stream_translates_deltas_and_tool_use() {
        let mut state = ClaudeSseState::default();
        let (start_lines, done) = translate_messages_data(
            "chatcmpl-1",
            "m",
            7,
            &serde_json::json!({"type": "message_start", "message": {"usage": {"input_tokens": 12}}}),
            &mut state,
        );
        assert!(!done);
        assert!(start_lines.is_empty());
        assert_eq!(state.prompt_tokens, 12);
        let (lines, done) = translate_messages_data(
            "chatcmpl-1",
            "m",
            7,
            &serde_json::json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi"}}),
            &mut state,
        );
        assert!(!done);
        assert_eq!(lines.len(), 2); // role + content
        let (tool_lines, _) = translate_messages_data(
            "chatcmpl-1",
            "m",
            7,
            &serde_json::json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": "t1", "name": "read"}}),
            &mut state,
        );
        assert_eq!(tool_lines.len(), 1);
        let (arg_lines, _) = translate_messages_data(
            "chatcmpl-1",
            "m",
            7,
            &serde_json::json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"p\":"}}),
            &mut state,
        );
        assert_eq!(arg_lines.len(), 1);
        let (end_lines, done) = translate_messages_data(
            "chatcmpl-1",
            "m",
            7,
            &serde_json::json!({"type": "message_delta", "usage": {"output_tokens": 5}}),
            &mut state,
        );
        assert!(!done);
        assert!(end_lines.is_empty());
        let (_, done) =
            translate_messages_data("chatcmpl-1", "m", 7, &serde_json::json!({"type": "message_stop"}), &mut state);
        assert!(done);
        let usage = claude_usage(&state);
        assert_eq!(usage["prompt_tokens"], 12);
        assert_eq!(usage["completion_tokens"], 5);
    }

    #[test]
    fn models_list_parses_documented_shape_only() {
        let body = serde_json::json!({"data": [
            {"id": "claude-sonnet-4-6", "display_name": "Sonnet"},
            {"id": ""},
            {"nope": 1},
        ]});
        let entries = parse_models_list(&body);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "claude-sonnet-4-6");
        assert_eq!(entries[0].1.as_deref(), Some("Sonnet"));
        assert!(parse_models_list(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn stream_errors_surface_never_clean_stop() {
        let typed = serde_json::json!({"type": "error", "error": {"message": "overloaded"}});
        assert_eq!(messages_stream_error(&typed).as_deref(), Some("overloaded"));
        let inline = serde_json::json!({"error": "boom"});
        assert!(messages_stream_error(&inline).is_some());
        let delta = serde_json::json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "hi"}});
        assert!(messages_stream_error(&delta).is_none());
        let stop = serde_json::json!({"type": "message_stop"});
        assert!(messages_stream_error(&stop).is_none());
        let mut state = ClaudeSseState::default();
        assert!(translate_messages_data_checked("c", "m", 7, &typed, &mut state).is_err());
        let (lines, done) = translate_messages_data_checked("c", "m", 7, &delta, &mut state).expect("delta");
        assert!(!done);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn unsupported_parts_detected() {
        let body = serde_json::json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "hi"}]},
            {"role": "user", "content": [{"type": "image", "source": {}}]},
        ]});
        assert!(has_unsupported_parts(&body));
        let ok = serde_json::json!({"messages": [{"role": "user", "content": "hi"}]});
        assert!(!has_unsupported_parts(&ok));
    }
}
