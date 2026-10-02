//! OpenCode (Zen Go) serving: OpenAI-compatible passthrough.
//!
//! Verified against official sources (none invented):
//! - Base URL `https://opencode.ai/zen/go/v1` with Bearer Zen key
//!   (`opencode.ai/docs/go`, Docker provider docs, GoModel provider docs).
//! - Most models serve OpenAI-style `POST /chat/completions`; a few serve
//!   `/responses`, and Anthropic-native models serve `/messages`
//!   (GoModel routing notes). This adapter serves the `/chat/completions`
//!   surface — models requiring another wire surface an honest upstream 400.
//! - Key verification shape (`/zen/go/v1/usage`: 401 = bad key,
//!   403 EntitlementError = valid key without a Go subscription) is already
//!   implemented in `oauth::verify_opencode_key`.
//!
//! Plan gating: only `Go` (or legacy `None`, which predates plan tracking
//! and was Go-verified) attempts upstream. `Free` keys have no subscription
//! and return an honest 501 via the router's skip path.

use super::{is_auth_status, is_retryable_status, serving_client as shared_client, truncate_snippet, AdapterError};

pub const FLOW_NOTE: &str = "OpenCode Go subscription key flow. Manual local token only in foundation; no automated /connect capture.";

pub const ZEN_GO_BASE_DEFAULT: &str = "https://opencode.ai/zen/go/v1";
pub const CHAT_COMPLETIONS_PATH: &str = "/chat/completions";

pub fn zen_go_base() -> String {
    std::env::var("OPENCODE_ZEN_BASE")
        .ok()
        .map(|base| base.trim_end_matches('/').to_string())
        .filter(|base| !base.is_empty())
        .unwrap_or_else(|| ZEN_GO_BASE_DEFAULT.to_string())
}

/// True for servable plans: explicit `Go`, or legacy `None` (pre-plan metas
/// were all Go-verified). `Free` and anything else never attempts upstream.
pub fn is_servable_plan(plan: Option<&str>) -> bool {
    match plan {
        None => true,
        Some(raw) => {
            let key = raw.trim().to_lowercase();
            key == "go"
        }
    }
}

pub fn serving_client() -> reqwest::Client {
    shared_client()
}

/// OpenAI chat body with the native model id. Injects
/// `stream_options.include_usage` for streaming sends so token usage can be
/// recorded from the final chunk (mirrors the Provider API docs pattern).
pub fn build_chat_body(native_model: &str, body: &serde_json::Value, stream: bool) -> serde_json::Value {
    let mut out = body.clone();
    if let Some(obj) = out.as_object_mut() {
        obj.insert("model".to_string(), serde_json::Value::String(native_model.to_string()));
        if stream && !obj.contains_key("stream_options") {
            obj.insert(
                "stream_options".to_string(),
                serde_json::json!({"include_usage": true}),
            );
        }
    }
    out
}

/// POST the chat body; maps HTTP failures to [`AdapterError`] with the shared
/// retry classification (429/5xx retryable, 401/403 terminal). Single attempt
/// — the caller rolls accounts on retryable errors.
pub async fn post_chat(
    client: &reqwest::Client,
    api_key: &str,
    native_model: &str,
    body: &serde_json::Value,
    stream: bool,
) -> Result<reqwest::Response, AdapterError> {
    let url = format!("{}{}", zen_go_base(), CHAT_COMPLETIONS_PATH);
    let payload = build_chat_body(native_model, body, stream);
    let resp = client
        .post(&url)
        .bearer_auth(api_key.trim())
        .header("Content-Type", "application/json")
        .header("Accept", if stream { "text/event-stream" } else { "application/json" })
        .json(&payload)
        .send()
        .await
        .map_err(|err| AdapterError::retryable(format!("opencode upstream error: {}", truncate_snippet(&err.to_string(), 1000)), 502))?;
    let status = resp.status().as_u16();
    let retry_after = super::retry_after_secs(resp.headers());
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        let snippet = truncate_snippet(text.trim(), 2000);
        // Plan-shaped 403s (EntitlementError / Go-plan text) roll to the next
        // account; credential 401/403 stays terminal.
        if status == 403 && super::is_plan_403(&snippet) {
            return Err(AdapterError::retryable(format!("opencode API {status}: {snippet}"), status).with_retry_after(retry_after));
        }
        if is_auth_status(status) {
            return Err(AdapterError::terminal(format!("opencode API {status}: {snippet}"), status).with_retry_after(retry_after));
        }
        return Err(AdapterError {
            message: format!("opencode API {status}: {snippet}"),
            status,
            retryable: is_retryable_status(status),
            retry_after,
        });
    }
    Ok(resp)
}

/// Native-wire passthrough (1B): `path` is `/responses` or `/messages`; the
/// body goes untouched for clients speaking that wire natively.
pub async fn post_native(
    client: &reqwest::Client,
    api_key: &str,
    path: &str,
    body: &serde_json::Value,
    stream: bool,
) -> Result<reqwest::Response, AdapterError> {
    let url = format!("{}{}", zen_go_base(), path);
    let resp = client
        .post(&url)
        .bearer_auth(api_key.trim())
        .header("Content-Type", "application/json")
        .header("Accept", if stream { "text/event-stream" } else { "application/json" })
        .json(body)
        .send()
        .await
        .map_err(|err| AdapterError::retryable(format!("opencode upstream error: {}", truncate_snippet(&err.to_string(), 1000)), 502))?;
    let status = resp.status().as_u16();
    let retry_after = super::retry_after_secs(resp.headers());
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        let snippet = truncate_snippet(text.trim(), 2000);
        if status == 403 && super::is_plan_403(&snippet) {
            return Err(AdapterError::retryable(format!("opencode API {status}: {snippet}"), status).with_retry_after(retry_after));
        }
        if is_auth_status(status) {
            return Err(AdapterError::terminal(format!("opencode API {status}: {snippet}"), status).with_retry_after(retry_after));
        }
        return Err(AdapterError {
            message: format!("opencode API {status}: {snippet}"),
            status,
            retryable: is_retryable_status(status),
            retry_after,
        });
    }
    Ok(resp)
}

/// Map a 403 upstream snippet to its attempt outcome: plan-shaped bodies
/// (EntitlementError / Go-plan text) roll (`Ok(true)` = retryable Next),
/// credential 403s stop (`Ok(false)` = terminal Stop). Non-403 statuses
/// return `Err` (not a 403 at all). Pure for unit tests; mirrors the
/// `post_chat` / `post_native` branching above.
pub fn is_rollable_plan_403(status: u16, snippet: &str) -> Result<bool, ()> {
    if status != 403 {
        return Err(());
    }
    Ok(super::is_plan_403(snippet))
}

/// Extract `(prompt, completion)` token counts from an OpenAI-style `usage`
/// object. Missing shapes map to zero (mirrors the bridge convention).
pub fn usage_counts(usage: Option<&serde_json::Value>) -> (u64, u64) {
    super::usage_counts(usage)
}

/// Rewrite the top-level `model` field to the gateway echo id (unified
/// `slug/native` or native provider-path id). Upstream returns the native id;
/// clients expect the id they sent.
pub fn rewrite_model(body: &mut serde_json::Value, echo_model: &str) {
    super::rewrite_model(body, echo_model)
}

/// Strip an SSE `data:` prefix; returns `None` for non-data lines
/// (`event:`, `:`, blanks). The `[DONE]` sentinel passes through as-is.
pub fn sse_payload(line: &str) -> Option<&str> {
    super::sse_payload(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servable_plans_gate_correctly() {
        assert!(is_servable_plan(None));
        assert!(is_servable_plan(Some("Go")));
        assert!(is_servable_plan(Some(" go ")));
        assert!(!is_servable_plan(Some("Free")));
        assert!(!is_servable_plan(Some("Pro")));
        assert!(!is_servable_plan(Some("")));
    }

    #[test]
    fn chat_body_sets_native_model_and_usage_flag() {
        let body = serde_json::json!({"model": "opencode/glm-5.1", "messages": [], "stream": true});
        let out = build_chat_body("glm-5.1", &body, true);
        assert_eq!(out["model"], "glm-5.1");
        assert_eq!(out["stream_options"]["include_usage"], true);
        let out2 = build_chat_body("glm-5.1", &body, false);
        assert!(out2.get("stream_options").is_none());
    }

    #[test]
    fn usage_counts_default_to_zero() {
        assert_eq!(usage_counts(None), (0, 0));
        assert_eq!(usage_counts(Some(&serde_json::json!({}))), (0, 0));
        assert_eq!(
            usage_counts(Some(&serde_json::json!({"prompt_tokens": 10, "completion_tokens": 20}))),
            (10, 20)
        );
    }

    #[test]
    fn sse_payload_strips_prefix() {
        assert_eq!(sse_payload("data: {\"a\":1}"), Some("{\"a\":1}"));
        assert_eq!(sse_payload("data:[DONE]"), Some("[DONE]"));
        assert_eq!(sse_payload("event: message"), None);
        assert_eq!(sse_payload(""), None);
    }

    #[test]
    fn rewrite_model_echoes_gateway_id() {
        let mut body = serde_json::json!({"model": "glm-5.1", "choices": []});
        rewrite_model(&mut body, "opencode/glm-5.1");
        assert_eq!(body["model"], "opencode/glm-5.1");
    }

    #[test]
    fn plan_403_rolls_while_credential_403_stops() {
        // Plan-shaped 403 bodies roll to the next account.
        assert_eq!(is_rollable_plan_403(403, "EntitlementError: valid key without a Go subscription"), Ok(true));
        assert_eq!(is_rollable_plan_403(403, "opencode API 403: without a subscription"), Ok(true));
        // Credential 401/403 stays terminal; non-403 is not classified here.
        assert_eq!(is_rollable_plan_403(403, "forbidden: bad key"), Ok(false));
        assert_eq!(is_rollable_plan_403(401, "unauthorized"), Err(()));
        assert_eq!(is_rollable_plan_403(429, "rate limited"), Err(()));
    }

    #[test]
    fn rewrite_model_skips_native_wires() {
        // Messages/Responses native shapes pass through untouched.
        let mut messages = serde_json::json!({"model": "m", "content": []});
        rewrite_model(&mut messages, "opencode/m2");
        assert_eq!(messages["model"], "m");
        let mut responses = serde_json::json!({"model": "m", "output": []});
        rewrite_model(&mut responses, "opencode/m2");
        assert_eq!(responses["model"], "m");
    }
}
