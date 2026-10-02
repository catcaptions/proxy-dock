//! Antigravity serving: OpenAI chat -> Cloud Code `generateContent` translation.
//!
//! Verified against proven-client sources (none invented):
//! - Endpoints `POST /v1internal:generateContent` (unary) and
//!   `POST /v1internal:streamGenerateContent?alt=sse` (SSE) on
//!   `https://cloudcode-pa.googleapis.com` with prod -> daily -> sandbox
//!   fallback (`opencode-antigravity-auth` API spec, `antigravity-proxy.py`,
//!   `picoclaw` provider, `jcode-provider-antigravity`).
//! - Envelope `{project, model, request, requestType: agent,
//!   userAgent: antigravity, requestId: agent-<uuid>}` with a Gemini-format
//!   inner `request` (`antigravity-proxy.py::_build_passthrough_envelope`,
//!   `picoclaw` envelope builder).
//! - `User-Agent: antigravity` (generic UAs get rejected — same finding as
//!   the quota module, mirroring the proven `antigravity-usage` tool).
//! - Project discovery via `POST /v1internal:loadCodeAssist` with IDE metadata
//!   `{ideType: ANTIGRAVITY, platform: PLATFORM_UNSPECIFIED, pluginType: GEMINI}`
//!   returning `cloudaicompanionProject` (already used in `quota.rs`).
//!
//! Credentials are Google OAuth `StoredCredential` JSON (or a pasted raw
//! access token — best effort, no refresh, no project cache).

use super::{is_retryable_status, sanitize_claude_via_google, sanitize_gemini_schema, truncate_snippet, AdapterError};

pub const FLOW_NOTE: &str = "Antigravity subscription flow. Manual local token only in foundation; no invented auth details.";

pub const HOSTS: [&str; 3] = [
    "https://cloudcode-pa.googleapis.com",
    "https://daily-cloudcode-pa.googleapis.com",
    "https://daily-cloudcode-pa.sandbox.googleapis.com",
];
pub const LOAD_PATH: &str = "/v1internal:loadCodeAssist";
pub const GENERATE_PATH: &str = "/v1internal:generateContent";
pub const STREAM_PATH: &str = "/v1internal:streamGenerateContent?alt=sse";

pub fn serving_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[derive(Debug, Clone)]
pub struct ServingCred {
    pub access_token: String,
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
            refresh_token: stored.refresh_token.clone().map(|r| r.trim().to_string()).filter(|r| !r.is_empty()),
        });
    }
    if super::commandcode::is_placeholder_token(trimmed) {
        return None;
    }
    Some(ServingCred { access_token: trimmed.to_string(), refresh_token: None })
}

fn base_override() -> Option<String> {
    std::env::var("ANTIGRAVITY_API_BASE")
        .ok()
        .map(|base| base.trim_end_matches('/').to_string())
        .filter(|base| !base.is_empty())
}

fn hosts() -> Vec<String> {
    hosts_for_catalog()
}

/// Host fallback list for serving + catalog (prod → daily → sandbox),
/// honoring the `ANTIGRAVITY_API_BASE` override. Public for the catalog
/// module so both share one fallback rule.
pub fn hosts_for_catalog() -> Vec<String> {
    if let Some(base) = base_override() {
        return vec![base];
    }
    HOSTS.iter().map(|h| h.to_string()).collect()
}

fn serving_headers(access_token: &str, stream: bool) -> reqwest::header::HeaderMap {
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
    insert(&mut map, "User-Agent", "antigravity".to_string());
    insert(&mut map, "X-Goog-Api-Client", "google-cloud-sdk vscode_cloudshelleditor/0.1".to_string());
    insert(
        &mut map,
        "Client-Metadata",
        serde_json::json!({"ideType": "IDE_UNSPECIFIED", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"}).to_string(),
    );
    if stream {
        insert(&mut map, "Accept", "text/event-stream".to_string());
    } else {
        insert(&mut map, "Accept", "application/json".to_string());
    }
    map
}

/// In-memory project cache: `loadCodeAssist` project per account (TTL 1h).
/// Keyed by the full access token (memory-only, never logged). The serve path
/// hits this instead of re-running discovery per request; quota/catalog keep
/// their full host loops and repopulate it as a side effect.
pub const PROJECT_CACHE_TTL_SECS: u64 = 3600;

static PROJECT_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (String, std::time::Instant)>>> =
    std::sync::OnceLock::new();

fn project_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, (String, std::time::Instant)>> {
    PROJECT_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Cached project for this credential, if fresh (TTL 1h). Pure read.
pub fn cached_project(access_token: &str) -> Option<String> {
    let map = project_cache().lock().ok()?;
    let (project, at) = map.get(access_token.trim())?;
    if at.elapsed().as_secs() < PROJECT_CACHE_TTL_SECS {
        Some(project.clone())
    } else {
        None
    }
}

fn store_project(access_token: &str, project: &str) {
    if let Ok(mut map) = project_cache().lock() {
        map.insert(access_token.trim().to_string(), (project.to_string(), std::time::Instant::now()));
    }
}

/// Clear the project cache (tests only).
#[cfg(test)]
fn clear_project_cache() {
    if let Ok(mut map) = project_cache().lock() {
        map.clear();
    }
}

/// Discover the `cloudaicompanionProject` for this credential, trying hosts in
/// order. Auth failures (401/403) are terminal; anything else (incl. regional
/// 400s) falls through to the next host — same rule as the quota module.
/// Fresh cache entries (TTL 1h) return immediately with no network.
pub async fn discover_project(client: &reqwest::Client, access_token: &str) -> Result<String, AdapterError> {
    if let Some(project) = cached_project(access_token) {
        return Ok(project);
    }
    let mut last_error = "antigravity project discovery exhausted".to_string();
    for host in hosts() {
        let resp = client
            .post(format!("{host}{LOAD_PATH}"))
            .headers(serving_headers(access_token, false))
            .json(&serde_json::json!({"metadata": {"ideType": "ANTIGRAVITY", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"}}))
            .send()
            .await;
        let resp = match resp {
            Ok(resp) => resp,
            Err(err) => {
                last_error = format!("antigravity project request failed: {}", truncate_snippet(&err.to_string(), 500));
                continue;
            }
        };
        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return Err(AdapterError::terminal("antigravity rejected the credential (sign in again)", status));
        }
        if !resp.status().is_success() {
            last_error = format!("antigravity project check failed ({status})");
            continue;
        }
        let body: serde_json::Value = resp.json().await.map_err(|err| AdapterError::retryable(format!("project parse failed: {err}"), 502))?;
        if let Some(project) = body.get("cloudaicompanionProject").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
            store_project(access_token, project);
            return Ok(project.to_string());
        }
        last_error = "antigravity project response carried no project id".to_string();
    }
    Err(AdapterError::retryable(last_error, 502))
}

fn text_of_content(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.get("text").and_then(|v| v.as_str()).map(str::to_string))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Caps for function payloads (large tool outputs must not blow up the
/// envelope; truncated outputs carry an explicit marker, never silent cuts).
pub const MAX_FUNCTION_ARGS_CHARS: usize = 8000;
pub const MAX_FUNCTION_OUTPUT_CHARS: usize = 12000;

/// True when the body carries image/file parts, which have no proven Gemini
/// mapping — the router rejects these with an honest 400 (never dropped
/// silently, never guessed).
pub fn has_unsupported_parts(body: &serde_json::Value) -> bool {
    let messages = body.get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for msg in &messages {
        if let Some(parts) = msg.get("content").and_then(|v| v.as_array()) {
            for part in parts {
                let kind = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match kind {
                    "text" | "input_text" | "output_text" => {}
                    _ => {
                        // Any non-text part (image_url, input_image, file,
                        // input_file, …) is unsupported.
                        if part.get("text").and_then(|v| v.as_str()).is_none() {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

fn truncate_marked(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}\n[truncated {max} chars]")
}

fn parse_args_object(args: &str) -> serde_json::Value {
    if args.trim().is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({"_raw": truncate_marked(args, MAX_FUNCTION_ARGS_CHARS)}))
}

/// OpenAI chat body -> Gemini inner `request`.
///
/// Mapping (best-effort, upstream 400s surface honestly):
/// - `system`/`developer` -> `systemInstruction.parts[{text}]`.
/// - `user` -> `contents[{role: user, parts:[{text}]}]`.
/// - `assistant` text -> `contents[{role: model, ...}]` plus paired
///   `functionCall` parts (see pairing rule below).
/// - `tools` (OpenAI function) -> `tools[{functionDeclarations[]}]`
///   (name/description pass through; parameters sanitized model-aware:
///   Claude models get draft 2020-12 output that is also Google-proto
///   safe (`sanitize_claude_via_google` — Google parses `parameters`
///   before Anthropic validates it); all other models get the OpenAPI-3.0
///   subset (`sanitize_gemini_schema` — unknown names INVALID_ARGUMENT).
///   `required` rides inside the caller's schema object, filtered to
///   declared properties).
/// - `tool` role -> `functionResponse` parts, paired by `tool_call_id`.
///
/// Pairing/prune invariant (mirrors `commandcode::openai_messages_to_cc`):
/// a `functionCall` is emitted only when its `tool_call_id` has a paired
/// `tool` output, and a `tool` output is emitted only when its id matches a
/// known call — dangling pairs are pruned, never fabricated.
/// - `temperature` / `top_p` / `max_tokens` -> `generationConfig`.
pub fn build_gemini_request(native_model: &str, body: &serde_json::Value) -> serde_json::Value {
    let _ = native_model;
    let mut system_texts: Vec<String> = Vec::new();
    let mut contents: Vec<serde_json::Value> = Vec::new();
    let messages = body.get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    // First pass: index tool calls and outputs by id (pairing for prune).
    let mut call_name_by_id: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut call_args_by_id: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut output_by_id: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for msg in &messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role == "assistant" {
            if let Some(calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                for tc in calls {
                    let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if id.is_empty() {
                        continue;
                    }
                    let name = tc
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let args = tc
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .map(|a| match a {
                            serde_json::Value::String(s) => s.clone(),
                            _ => serde_json::to_string(a).unwrap_or_else(|_| "{}".to_string()),
                        })
                        .unwrap_or_else(|| "{}".to_string());
                    call_name_by_id.insert(id.clone(), name);
                    call_args_by_id.insert(id, args);
                }
            }
        } else if role == "tool" {
            let id = msg.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !id.is_empty() {
                output_by_id.insert(id, text_of_content(msg.get("content").unwrap_or(&serde_json::Value::Null)));
            }
        }
    }
    for msg in &messages {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        match role {
            "system" | "developer" => {
                let text = msg.get("content").map(text_of_content).unwrap_or_default();
                if !text.trim().is_empty() {
                    system_texts.push(text);
                }
            }
            "user" => {
                let text = msg.get("content").map(text_of_content).unwrap_or_default();
                if text.trim().is_empty() {
                    continue;
                }
                contents.push(serde_json::json!({"role": "user", "parts": [{"text": text}]}));
            }
            "assistant" => {
                let text = msg.get("content").map(text_of_content).unwrap_or_default();
                let mut parts: Vec<serde_json::Value> = Vec::new();
                if !text.trim().is_empty() {
                    parts.push(serde_json::json!({"text": text}));
                }
                // Paired calls only: a call without its tool output is
                // pruned (mirrors the commandcode dangling-pair rule).
                if let Some(calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in calls {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        if id.is_empty() || !output_by_id.contains_key(id) {
                            continue;
                        }
                        let name = call_name_by_id.get(id).cloned().unwrap_or_default();
                        let args = call_args_by_id.get(id).cloned().unwrap_or_else(|| "{}".to_string());
                        let function_call =
                            serde_json::json!({"name": name, "args": parse_args_object(&args)});
                        // Echo the smuggled thought signature at PART level
                        // (where Gemini returns it): the internal API rejects
                        // it inside functionCall, and 400s multi-turn tool use
                        // without it anywhere.
                        let mut part = serde_json::json!({"functionCall": function_call});
                        if let Some(sig) = thought_signature_for_id(id) {
                            part["thoughtSignature"] = serde_json::Value::String(sig.to_string());
                        }
                        parts.push(part);
                    }
                }
                if parts.is_empty() {
                    continue;
                }
                contents.push(serde_json::json!({"role": "model", "parts": parts}));
            }
            "tool" => {
                let id = msg.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
                // Prune outputs whose call is unknown (dangling result).
                let Some(name) = call_name_by_id.get(id).filter(|n| !n.is_empty()) else {
                    continue;
                };
                let output = output_by_id.get(id).cloned().unwrap_or_default();
                contents.push(serde_json::json!({
                    "role": "user",
                    "parts": [{"functionResponse": {"name": name, "response": {"output": truncate_marked(&output, MAX_FUNCTION_OUTPUT_CHARS)}}}],
                }));
            }
            _ => {}
        }
    }
    if contents.is_empty() {
        contents.push(serde_json::json!({"role": "user", "parts": [{"text": " "}]}));
    }
    let mut request = serde_json::json!({"contents": contents});
    // OpenAI function tools -> Gemini functionDeclarations, sanitized for
    // the model family behind this request (see module docs).
    let sanitize: fn(&serde_json::Value) -> serde_json::Value =
        if native_model.to_lowercase().starts_with("claude") {
            sanitize_claude_via_google
        } else {
            sanitize_gemini_schema
        };
    if let Some(tools) = body.get("tools").and_then(|v| v.as_array()) {
        let mut declarations: Vec<serde_json::Value> = Vec::new();
        for tool in tools {
            if tool.get("type").and_then(|v| v.as_str()) != Some("function") {
                continue;
            }
            let func = tool.get("function").cloned().unwrap_or(serde_json::Value::Null);
            let name = func.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            let mut decl = serde_json::json!({"name": name});
            if let Some(desc) = func.get("description").and_then(|v| v.as_str()) {
                decl["description"] = serde_json::Value::String(desc.to_string());
            }
            if let Some(params) = func.get("parameters") {
                decl["parameters"] = sanitize(params);
            }
            declarations.push(decl);
        }
        if !declarations.is_empty() {
            request["tools"] = serde_json::json!([{"functionDeclarations": declarations}]);
        }
    }
    if !system_texts.is_empty() {
        request["systemInstruction"] = serde_json::json!({"parts": [{"text": system_texts.join("\n\n")}]});
    }
    let mut generation_config = serde_json::Map::new();
    if let Some(temp) = body.get("temperature").and_then(|v| v.as_f64()) {
        generation_config.insert("temperature".to_string(), serde_json::Value::from(temp));
    }
    if let Some(top_p) = body.get("top_p").and_then(|v| v.as_f64()) {
        generation_config.insert("topP".to_string(), serde_json::Value::from(top_p));
    }
    if let Some(max_tokens) = body.get("max_tokens").and_then(|v| v.as_u64()).or_else(|| body.get("max_output_tokens").and_then(|v| v.as_u64())) {
        generation_config.insert("maxOutputTokens".to_string(), serde_json::Value::from(max_tokens));
    }
    if !generation_config.is_empty() {
        request["generationConfig"] = serde_json::Value::Object(generation_config);
    }
    request
}

/// Wrap the inner request in the `v1internal` envelope.
pub fn build_envelope(project: &str, native_model: &str, inner: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "project": project,
        "model": native_model,
        "request": inner,
        "requestType": "agent",
        "userAgent": "antigravity",
        "requestId": format!("agent-{}", uuid::Uuid::new_v4().simple()),
    })
}

/// POST the envelope on the serve path. Single-host, fail-fast: the first
/// host is tried once and retryable failures return immediately so the caller
/// rolls to the next account instead of burning through a 3-host inner loop
/// per account. Quota/catalog keep their own full host loops (see
/// `hosts_for_catalog` / quota `cloudcode_post`).
/// Single logical attempt — the caller rolls accounts on retryable errors.
pub async fn post_generate(
    client: &reqwest::Client,
    access_token: &str,
    project: &str,
    native_model: &str,
    body: &serde_json::Value,
    stream: bool,
) -> Result<reqwest::Response, AdapterError> {
    let inner = build_gemini_request(native_model, body);
    let envelope = build_envelope(project, native_model, &inner);
    let path = if stream { STREAM_PATH } else { GENERATE_PATH };
    let host = hosts().into_iter().next().unwrap_or_else(|| HOSTS[0].to_string());
    let resp = client
        .post(format!("{host}{path}"))
        .headers(serving_headers(access_token, stream))
        .json(&envelope)
        .send()
        .await
        .map_err(|err| AdapterError::retryable(format!("antigravity upstream error: {}", truncate_snippet(&err.to_string(), 1000)), 502))?;
    let status = resp.status().as_u16();
    if status == 401 || status == 403 {
        return Err(AdapterError::terminal("antigravity rejected the credential (sign in again)", status));
    }
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        let snippet = truncate_snippet(text.trim(), 2000);
        if is_retryable_status(status) {
            return Err(AdapterError::retryable(format!("antigravity API {status}: {snippet}"), status));
        }
        return Err(AdapterError::terminal(format!("antigravity API {status}: {snippet}"), status));
    }
    Ok(resp)
}

/// Full host-fallback generate for quota/catalog-style callers that want the
/// prod -> daily -> sandbox rule instead of serve-path fail-fast. Same
/// terminal-vs-fallback classification as project discovery.
pub async fn post_generate_all_hosts(
    client: &reqwest::Client,
    access_token: &str,
    project: &str,
    native_model: &str,
    body: &serde_json::Value,
    stream: bool,
) -> Result<reqwest::Response, AdapterError> {
    let inner = build_gemini_request(native_model, body);
    let envelope = build_envelope(project, native_model, &inner);
    let path = if stream { STREAM_PATH } else { GENERATE_PATH };
    let mut last_error = "antigravity upstream exhausted".to_string();
    for host in hosts() {
        let resp = client
            .post(format!("{host}{path}"))
            .headers(serving_headers(access_token, stream))
            .json(&envelope)
            .send()
            .await;
        let resp = match resp {
            Ok(resp) => resp,
            Err(err) => {
                last_error = format!("antigravity upstream error: {}", truncate_snippet(&err.to_string(), 1000));
                continue;
            }
        };
        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return Err(AdapterError::terminal("antigravity rejected the credential (sign in again)", status));
        }
        if status >= 400 {
            let text = resp.text().await.unwrap_or_default();
            let snippet = truncate_snippet(text.trim(), 2000);
            if is_retryable_status(status) {
                last_error = format!("antigravity API {status}: {snippet}");
                continue;
            }
            return Err(AdapterError::terminal(format!("antigravity API {status}: {snippet}"), status));
        }
        return Ok(resp);
    }
    Err(AdapterError::retryable(last_error, 502))
}

/// Unwrap the `{"response": {...}}` envelope (or accept a bare body).
pub fn unwrap_response(body: &serde_json::Value) -> &serde_json::Value {
    body.get("response").unwrap_or(body)
}

/// Gemini response -> `(text, prompt_tokens, completion_tokens)`.
///
/// Reads the first candidate's `content.parts[].text` and
/// `usageMetadata.{promptTokenCount, candidatesTokenCount}`. Missing shapes
/// map to empty text / zero usage (never fabricated).
pub fn gemini_fields(body: &serde_json::Value) -> (String, u64, u64) {
    let inner = unwrap_response(body);
    let mut texts: Vec<String> = Vec::new();
    if let Some(candidates) = inner.get("candidates").and_then(|v| v.as_array()) {
        if let Some(first) = candidates.first() {
            if let Some(parts) = first.get("content").and_then(|c| c.get("parts")).and_then(|v| v.as_array()) {
                for part in parts {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        if !text.is_empty() {
                            texts.push(text.to_string());
                        }
                    }
                }
            }
        }
    }
    let usage = inner.get("usageMetadata");
    let prompt = usage.and_then(|u| u.get("promptTokenCount")).and_then(|v| v.as_u64()).unwrap_or(0);
    let completion = usage.and_then(|u| u.get("candidatesTokenCount")).and_then(|v| v.as_u64()).unwrap_or(0);
    (texts.join(""), prompt, completion)
}

/// Gemini response -> `(prompt_tokens, completion_tokens, cached_tokens)`.
///
/// Same `usageMetadata` read as [`gemini_fields`], plus
/// `cachedContentTokenCount` (context-cache reads, billed at the reduced
/// rate). Split out so streaming folds can track the cache leg without
/// re-parsing text.
pub fn gemini_usage(body: &serde_json::Value) -> (u64, u64, u64) {
    let usage = unwrap_response(body).get("usageMetadata");
    let prompt = usage.and_then(|u| u.get("promptTokenCount")).and_then(|v| v.as_u64()).unwrap_or(0);
    let completion = usage.and_then(|u| u.get("candidatesTokenCount")).and_then(|v| v.as_u64()).unwrap_or(0);
    let cached = usage.and_then(|u| u.get("cachedContentTokenCount")).and_then(|v| v.as_u64()).unwrap_or(0);
    if cached > prompt {
        (prompt, completion, 0)
    } else {
        (prompt, completion, cached)
    }
}

/// Extract OpenAI-style `tool_calls` from a Gemini body: first candidate's
/// `content.parts[]` with `functionCall` become `{id, type: function,
/// function: {name, arguments}}`. Ids are synthesized (`call_<12hex>`) since
/// Gemini calls carry no client id. Parts without a name are skipped.
///
/// Thought-signature round-trip: Gemini requires the part's
/// `thoughtSignature` echoed back on later turns, but the OpenAI shape has no
/// field for it — so it rides inside the id as `call_<12hex>~<signature>`
/// (`~` appears in no base64 alphabet, so parsing is unambiguous). Foreign
/// ids without `~` behave exactly as before.
pub fn gemini_tool_calls(body: &serde_json::Value) -> Vec<serde_json::Value> {
    let inner = unwrap_response(body);
    let mut out = Vec::new();
    let candidates = inner.get("candidates").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let Some(first) = candidates.first() else {
        return out;
    };
    let parts = first.get("content").and_then(|c| c.get("parts")).and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for part in parts {
        let Some(call) = part.get("functionCall") else {
            continue;
        };
        let name = call.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let args = call.get("args").map(|a| serde_json::to_string(a).unwrap_or_else(|_| "{}".to_string())).unwrap_or_else(|| "{}".to_string());
        let rand = &uuid::Uuid::new_v4().simple().to_string()[..12];
        let id = match part.get("thoughtSignature").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            Some(sig) => format!("call_{rand}~{sig}"),
            None => format!("call_{rand}"),
        };
        out.push(serde_json::json!({
            "id": id,
            "type": "function",
            "function": {"name": name, "arguments": args},
        }));
    }
    out
}

/// Thought signature smuggled in a synthesized tool-call id (`call_<12hex>`
/// or `call_<12hex>~<signature>`); `None` for foreign ids.
fn thought_signature_for_id(id: &str) -> Option<&str> {
    let rest = id.strip_prefix("call_")?;
    let tilde = rest.find('~')?;
    let sig = &rest[tilde + 1..];
    if sig.is_empty() {
        return None;
    }
    Some(sig)
}

/// Build a non-streaming `chat.completion` object from a Gemini body.
pub fn gemini_to_chat_completion(body: &serde_json::Value, completion_id: &str, echo_model: &str, created: u64) -> serde_json::Value {
    let (text, prompt, completion) = gemini_fields(body);
    let (_, _, cached) = gemini_usage(body);
    let tool_calls = gemini_tool_calls(body);
    let mut message = serde_json::json!({"role": "assistant", "content": text});
    let finish = if tool_calls.is_empty() {
        "stop"
    } else {
        message["tool_calls"] = serde_json::Value::Array(tool_calls);
        "tool_calls"
    };
    serde_json::json!({
        "id": completion_id,
        "object": "chat.completion",
        "created": created,
        "model": echo_model,
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        "usage": {
            "prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt + completion,
            "prompt_tokens_details": {"cached_tokens": cached},
            "cache_read_input_tokens": cached,
        },
    })
}

/// Refresh the Google access token (mirrors `quota::refresh_google_token`).
/// The client secret comes from oauth_secret (vault/env) — never hardcoded.
pub async fn refresh_access_token(refresh_token: &str) -> Result<(String, Option<String>), String> {
    const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
    const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
    let client_secret = crate::oauth_secret::read()?;
    let client = serving_client();
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
            ("client_secret", client_secret.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|err| format!("google token refresh failed: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!("google token refresh failed ({})", resp.status()));
    }
    let body: serde_json::Value = resp.json().await.map_err(|err| format!("refresh parse failed: {err}"))?;
    let access = body.get("access_token").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if access.trim().is_empty() {
        return Err("google refresh returned no access token".to_string());
    }
    let refresh = body.get("refresh_token").and_then(|v| v.as_str()).map(str::to_string);
    Ok((access, refresh))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_cache_serves_fresh_and_isolates_accounts() {
        clear_project_cache();
        assert!(cached_project("tok-a").is_none());
        store_project("tok-a", "proj-a");
        store_project("tok-b", "proj-b");
        assert_eq!(cached_project("tok-a").as_deref(), Some("proj-a"));
        assert_eq!(cached_project("tok-b").as_deref(), Some("proj-b"));
        assert!(cached_project("tok-c").is_none());
        // Whitespace-trimmed tokens share the slot.
        assert_eq!(cached_project("  tok-a  ").as_deref(), Some("proj-a"));
        clear_project_cache();
        assert!(cached_project("tok-a").is_none());
    }

    #[test]
    fn serving_cred_parses_oauth_and_raw() {
        let oauth = serde_json::json!({"access_token": "ya29.x", "refresh_token": "1//y", "id_token": null, "account_id": "", "email": "e", "expires_in": null}).to_string();
        let cred = parse_serving_cred(&oauth).expect("oauth");
        assert_eq!(cred.access_token, "ya29.x");
        let raw = parse_serving_cred("ya29-live-real-token-value").expect("raw");
        assert_eq!(raw.access_token, "ya29-live-real-token-value");
        assert!(parse_serving_cred("no-key-required").is_none());
    }

    #[test]
    fn chat_maps_to_gemini_contents() {
        let body = serde_json::json!({
            "model": "antigravity/gemini-3-flash",
            "messages": [
                {"role": "system", "content": "be nice"},
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hey"},
            ],
            "temperature": 0.5,
            "max_tokens": 100,
        });
        let inner = build_gemini_request("gemini-3-flash", &body);
        assert_eq!(inner["systemInstruction"]["parts"][0]["text"], "be nice");
        let contents = inner["contents"].as_array().unwrap();
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(inner["generationConfig"]["temperature"], 0.5);
        assert_eq!(inner["generationConfig"]["maxOutputTokens"], 100);
        let envelope = build_envelope("proj-1", "gemini-3-flash", &inner);
        assert_eq!(envelope["project"], "proj-1");
        assert_eq!(envelope["requestType"], "agent");
    }

    #[test]
    fn gemini_maps_to_chat_completion() {
        let upstream = serde_json::json!({
            "response": {
                "candidates": [{"content": {"parts": [{"text": "hello"}]}}],
                "usageMetadata": {"promptTokenCount": 8, "candidatesTokenCount": 3},
            }
        });
        let obj = gemini_to_chat_completion(&upstream, "chatcmpl-1", "antigravity/gemini-3-flash", 7);
        assert_eq!(obj["choices"][0]["message"]["content"], "hello");
        assert_eq!(obj["usage"]["prompt_tokens"], 8);
        assert_eq!(obj["usage"]["completion_tokens"], 3);
        assert_eq!(obj["usage"]["total_tokens"], 11);
    }

    #[test]
    fn gemini_cached_content_count_is_a_cache_leg() {
        let body = serde_json::json!({
            "usageMetadata": {
                "promptTokenCount": 100, "candidatesTokenCount": 20,
                "cachedContentTokenCount": 70,
            }
        });
        assert_eq!(gemini_usage(&body), (100, 20, 70));
        let obj = gemini_to_chat_completion(&body, "chatcmpl-1", "antigravity/gemini-3-flash", 7);
        assert_eq!(obj["usage"]["cache_read_input_tokens"], 70);
        // Absent count maps to zero; impossible counts are dropped, not trusted.
        assert_eq!(gemini_usage(&serde_json::json!({})), (0, 0, 0));
        assert_eq!(
            gemini_usage(&serde_json::json!({"usageMetadata": {"promptTokenCount": 5, "cachedContentTokenCount": 50}})),
            (5, 0, 0)
        );
    }

    #[test]
    fn gemini_missing_shapes_map_to_zero() {
        let (text, prompt, completion) = gemini_fields(&serde_json::json!({}));
        assert_eq!(text, "");
        assert_eq!((prompt, completion), (0, 0));
    }

    #[test]
    fn tool_round_trip_pairs_and_prunes() {
        let body = serde_json::json!({
            "model": "antigravity/gemini-3-flash",
            "messages": [
                {"role": "assistant", "content": "looking",
                 "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "read", "arguments": "{\"p\":\"x\"}"}},
                    {"id": "dangling", "type": "function", "function": {"name": "gone", "arguments": "{}"}},
                 ]},
                {"role": "tool", "tool_call_id": "c1", "content": "file-bytes"},
                {"role": "tool", "tool_call_id": "orphan", "content": "no-call"},
                {"role": "user", "content": "go"},
            ],
            "tools": [{"type": "function", "function": {"name": "read", "description": "d", "parameters": {"type": "object", "required": ["p"]}}}],
        });
        let inner = build_gemini_request("gemini-3-flash", &body);
        // functionDeclarations carry the schema (incl. required) untouched.
        assert_eq!(inner["tools"][0]["functionDeclarations"][0]["name"], "read");
        assert_eq!(inner["tools"][0]["functionDeclarations"][0]["parameters"]["required"][0], "p");
        let contents = inner["contents"].as_array().unwrap();
        // Paired call emitted, dangling call pruned, orphan output pruned.
        let model = contents.iter().find(|c| c["role"] == "model").expect("model turn");
        let parts = model["parts"].as_array().unwrap();
        assert!(parts.iter().any(|p| p["functionCall"]["name"] == "read"));
        assert!(!parts.iter().any(|p| p.get("functionCall").and_then(|c| c.get("name")).and_then(|v| v.as_str()) == Some("gone")));
        assert!(contents.iter().any(|c| c["parts"][0]["functionResponse"]["name"] == "read"));
        assert!(!contents.iter().any(|c| c["parts"][0].get("functionResponse").and_then(|r| r.get("name")).and_then(|v| v.as_str()) == Some("orphan")));
    }

    #[test]
    fn unsupported_parts_detected() {
        let bad = serde_json::json!({"messages": [{"role": "user", "content": [{"type": "image_url", "image_url": {"url": "x"}}]}]});
        assert!(has_unsupported_parts(&bad));
        let ok = serde_json::json!({"messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]});
        assert!(!has_unsupported_parts(&ok));
        let plain = serde_json::json!({"messages": [{"role": "user", "content": "hi"}]});
        assert!(!has_unsupported_parts(&plain));
    }

    #[test]
    fn function_calls_extract_with_ids() {
        let upstream = serde_json::json!({
            "response": {"candidates": [{"content": {"parts": [
                {"text": "working"},
                {"functionCall": {"name": "read", "args": {"p": "x"}}},
                {"functionCall": {"args": {}}},
            ]}}]}
        });
        let calls = gemini_tool_calls(&upstream);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["function"]["name"], "read");
        assert!(calls[0]["id"].as_str().is_some_and(|s| s.starts_with("call_")));
        let obj = gemini_to_chat_completion(&upstream, "c", "m", 1);
        assert_eq!(obj["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn thought_signature_round_trips_through_call_ids() {
        // Extraction embeds the part signature in the id …
        let upstream = serde_json::json!({
            "response": {"candidates": [{"content": {"parts": [
                {"functionCall": {"name": "calc", "args": {}}, "thoughtSignature": "sig-ABC_123"},
            ]}}]}
        });
        let calls = gemini_tool_calls(&upstream);
        assert_eq!(calls.len(), 1);
        let id = calls[0]["id"].as_str().expect("id");
        assert!(id.starts_with("call_") && id.ends_with("~sig-ABC_123"), "unexpected id {id}");
        assert_eq!(thought_signature_for_id(id), Some("sig-ABC_123"));
        // … and the request mapper echoes it back onto the functionCall.
        let body = serde_json::json!({
            "messages": [
                {"role": "assistant", "content": "", "tool_calls": [
                    {"id": id, "type": "function", "function": {"name": "calc", "arguments": "{}"}},
                ]},
                {"role": "tool", "tool_call_id": id, "content": "4"},
            ],
        });
        let inner = build_gemini_request("gemini-3-flash", &body);
        let model = inner["contents"].as_array().unwrap().iter().find(|c| c["role"] == "model").expect("model turn");
        let fc = model["parts"].as_array().unwrap().iter().find(|p| p.get("functionCall").is_some()).expect("functionCall");
        assert_eq!(fc["thoughtSignature"], "sig-ABC_123");
        // Foreign ids (no smuggled signature) behave exactly as before.
        assert_eq!(thought_signature_for_id("call_3d3314a2c856"), None);
        assert_eq!(thought_signature_for_id("call_"), None);
        assert_eq!(thought_signature_for_id("external-id"), None);
    }
}
