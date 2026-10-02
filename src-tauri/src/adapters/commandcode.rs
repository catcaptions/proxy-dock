pub const SHORT_ALIASES: [(&str, &str); 56] = [
    ("deepseek-v4-pro", "deepseek/deepseek-v4-pro"),
    ("deepseek-v4", "deepseek/deepseek-v4-pro"),
    ("deepseek-pro", "deepseek/deepseek-v4-pro"),
    ("deepseek-v4-flash", "deepseek/deepseek-v4-flash"),
    ("deepseek-flash", "deepseek/deepseek-v4-flash"),
    ("glm-5.2", "zai-org/GLM-5.2"),
    ("glm5.2", "zai-org/GLM-5.2"),
    ("glm-5.2-fast", "zai-org/GLM-5.2-Fast"),
    ("glm5.2-fast", "zai-org/GLM-5.2-Fast"),
    ("glm-5.1", "zai-org/GLM-5.1"),
    ("glm-5", "zai-org/GLM-5"),
    ("minimax-m3", "MiniMaxAI/MiniMax-M3"),
    ("minimax3", "MiniMaxAI/MiniMax-M3"),
    ("minimax-m2.7", "MiniMaxAI/MiniMax-M2.7"),
    ("minimax2.7", "MiniMaxAI/MiniMax-M2.7"),
    ("minimax-m2.5", "MiniMaxAI/MiniMax-M2.5"),
    ("minimax2.5", "MiniMaxAI/MiniMax-M2.5"),
    ("kimi-k3", "moonshotai/Kimi-K3"),
    ("kimi3", "moonshotai/Kimi-K3"),
    ("kimi-k2.7-code", "moonshotai/Kimi-K2.7-Code"),
    ("kimi2.7-code", "moonshotai/Kimi-K2.7-Code"),
    ("kimi-code", "moonshotai/Kimi-K2.7-Code"),
    ("kimi-k2.7-highspeed", "moonshotai/Kimi-K2.7-Code-Highspeed"),
    ("kimi-highspeed", "moonshotai/Kimi-K2.7-Code-Highspeed"),
    ("kimi-k2.6", "moonshotai/Kimi-K2.6"),
    ("kimi2.6", "moonshotai/Kimi-K2.6"),
    ("kimi-k2.5", "moonshotai/Kimi-K2.5"),
    ("kimi2.5", "moonshotai/Kimi-K2.5"),
    ("qwen3.7-max", "Qwen/Qwen3.7-Max"),
    ("qwen-3.7-max", "Qwen/Qwen3.7-Max"),
    ("qwen3.7-plus", "Qwen/Qwen3.7-Plus"),
    ("qwen-3.7-plus", "Qwen/Qwen3.7-Plus"),
    ("qwen3.6-max", "Qwen/Qwen3.6-Max-Preview"),
    ("qwen-3.6-plus", "Qwen/Qwen3.6-Plus"),
    ("qwen3.6-plus", "Qwen/Qwen3.6-Plus"),
    ("step-3.7-flash", "stepfun/Step-3.7-Flash"),
    ("step3.7", "stepfun/Step-3.7-Flash"),
    ("step-3.5-flash", "stepfun/Step-3.5-Flash"),
    ("step3.5", "stepfun/Step-3.5-Flash"),
    ("mimo-v2.5-pro", "xiaomi/mimo-v2.5-pro"),
    ("mimo-pro", "xiaomi/mimo-v2.5-pro"),
    ("mimo-v2.5", "xiaomi/mimo-v2.5"),
    ("mimo2.5", "xiaomi/mimo-v2.5"),
    ("grok-4.5", "xai/grok-4.5"),
    ("grok4.5", "xai/grok-4.5"),
    ("nemotron", "nvidia/nemotron-3-ultra-550b-a55b"),
    ("nemotron-3-ultra", "nvidia/nemotron-3-ultra-550b-a55b"),
    ("inkling", "thinkingmachines/inkling"),
    ("hy3", "tencent/Hy3"),
    ("claude-sonnet-4-6", "claude-sonnet-4-6"),
    ("claude-opus-4-7", "claude-opus-4-7"),
    ("claude-haiku-4-5-20251001", "claude-haiku-4-5-20251001"),
    ("gpt-5.5", "gpt-5.5"),
    ("gpt-5.4", "gpt-5.4"),
    ("antigravity-default", "antigravity-default"),
    ("opencode-default", "opencode-default"),
];

pub const BUILTIN_MODELS: [&str; 63] = [
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-fable-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-haiku-4-5-20251001",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.3-codex",
    "gpt-5.4-mini",
    "deepseek/deepseek-v4-pro",
    "deepseek/deepseek-v4-flash",
    "deepseek/deepseek-v4-flash-vision-exp",
    "moonshotai/Kimi-K3",
    "moonshotai/Kimi-K2.7-Code",
    "moonshotai/Kimi-K2.7-Code-Highspeed",
    "moonshotai/Kimi-K2.6",
    "moonshotai/Kimi-K2.5",
    "z-ai/glm-5.3-flash",
    "zai-org/GLM-5.3",
    "zai-org/GLM-5.2",
    "zai-org/GLM-5.2-Fast",
    "zai-org/GLM-5.1",
    "zai-org/GLM-5",
    "MiniMaxAI/MiniMax-M3",
    "MiniMaxAI/MiniMax-M2.7",
    "minimax/minimax-m3-free",
    "minimax/minimax-m2.7-free",
    "MiniMaxAI/MiniMax-M2.5",
    "xiaomi/mimo-v2.5-pro",
    "xiaomi/mimo-v2.5",
    "Qwen/Qwen3.8-Max",
    "Qwen/Qwen3.8-27B",
    "Qwen/Qwen3.8-Flash",
    "Qwen/Qwen3.7-Max",
    "Qwen/Qwen3.7-Plus",
    "Qwen/Qwen3.7-Flash",
    "Qwen/Qwen3.6-Max-Preview",
    "Qwen/Qwen3.6-Plus",
    "stepfun/Step-3.7-Flash",
    "stepfun/Step-3.5-Flash",
    "tencent/hy3-paid",
    "google/gemini-3.7-flash",
    "google/gemini-3.6-flash",
    "google/gemini-3.5-flash",
    "google/gemini-3.5-flash-lite",
    "google/gemini-3.1-flash-lite",
    "sakana/fugu-ultra",
    "nvidia/nemotron-3-ultra-550b-a55b",
    "thinkingmachines/inkling",
    "thinkingmachines/inkling-small",
    "poolside/laguna-s-2.1-free",
    "meta/muse-spark-1.1",
    "meta/muse-spark-1.2",
    "meta/muse-spark-1.2-contributor",
    "xai/grok-4.5",
    "xai/grok-4.6",
    "antigravity-default",
    "opencode-default",
];

pub const CONTEXT_WINDOWS: [(&str, u32); 27] = [
    ("deepseek/deepseek-v4-pro", 1048576),
    ("deepseek/deepseek-v4-flash", 1048576),
    ("zai-org/GLM-5.2", 1048576),
    ("zai-org/GLM-5.2-Fast", 1048576),
    ("zai-org/GLM-5.1", 200000),
    ("zai-org/GLM-5", 200000),
    ("MiniMaxAI/MiniMax-M3", 1048576),
    ("MiniMaxAI/MiniMax-M2.7", 204800),
    ("MiniMaxAI/MiniMax-M2.5", 200000),
    ("moonshotai/Kimi-K3", 1048576),
    ("moonshotai/Kimi-K2.7-Code", 256000),
    ("moonshotai/Kimi-K2.7-Code-Highspeed", 262000),
    ("moonshotai/Kimi-K2.6", 256000),
    ("moonshotai/Kimi-K2.5", 256000),
    ("Qwen/Qwen3.7-Max", 1048576),
    ("Qwen/Qwen3.7-Plus", 1048576),
    ("Qwen/Qwen3.6-Max-Preview", 262144),
    ("Qwen/Qwen3.6-Plus", 1048576),
    ("stepfun/Step-3.7-Flash", 256000),
    ("stepfun/Step-3.5-Flash", 1048576),
    ("xiaomi/mimo-v2.5-pro", 1048576),
    ("xiaomi/mimo-v2.5", 1048576),
    ("xai/grok-4.5", 500000),
    ("nvidia/nemotron-3-ultra-550b-a55b", 1048576),
    ("thinkingmachines/inkling", 256000),
    ("tencent/Hy3", 262144),
    ("tencent/hy3-paid", 262144),
];

pub const REASONING_EFFORTS: [(&str, &[&str]); 4] = [
    ("deepseek/deepseek-v4-pro", &["high", "max"]),
    ("deepseek/deepseek-v4-flash", &["high", "max"]),
    ("zai-org/GLM-5.2", &["high", "max"]),
    ("xai/grok-4.5", &["low", "medium", "high"]),
];

pub const EFFORT_RANK: [(&str, u8); 5] = [("low", 0), ("medium", 1), ("high", 2), ("xhigh", 3), ("max", 4)];

pub const PLACEHOLDER_TOKENS: [&str; 33] = [
    "x", "none", "null", "nil", "undefined", "empty", "missing", "unset", "dummy", "test", "test-key",
    "testkey", "changeme", "your-api-key", "api-key", "apikey", "key", "demo", "fake", "invalid", "default",
    "example", "sample", "secret", "password", "123", "12345", "abc", "xxx", "no-key-required", "no-key",
    "nokey", "not-required",
];

pub const PLACEHOLDER_PREFIXES: [&str; 10] = [
    "no-key", "nokey", "placeholder", "dummy", "example", "sk-placeholder", "test-key", "testkey",
    "your-api-key", "changeme",
];

/// Request-time alias resolution only (`SHORT_ALIASES` + bare-name match
/// against `BUILTIN_MODELS`). Never invents a choice: empty/`"default"` pass
/// through unchanged so the gateway rejects them with `proxy_dock_bad_model`
/// instead of silently serving `BUILTIN_MODELS[0]`. `BUILTIN_MODELS` never
/// leaks into `GET /v1/models` (the catalog comes only from live 1A).
pub fn resolve_model(model: &str) -> String {
    if model.is_empty() || model.eq_ignore_ascii_case("default") {
        return model.to_string();
    }
    let lower = model.to_lowercase();
    if let Some((_, canonical)) = SHORT_ALIASES.iter().find(|(alias, _)| alias.eq_ignore_ascii_case(model)) {
        return canonical.to_string();
    }
    if model.contains('/') {
        return model.to_string();
    }
    for builtin in BUILTIN_MODELS {
        if builtin.split('/').last().unwrap_or("").eq_ignore_ascii_case(&lower) {
            return builtin.to_string();
        }
    }
    model.to_string()
}

pub fn resolve_alias(model: &str) -> String {
    resolve_model(model)
}

fn effort_rank(effort: &str) -> u8 {
    EFFORT_RANK.iter().find(|(name, _)| *name == effort).map(|(_, rank)| *rank).unwrap_or(2)
}

pub fn resolve_effort(canonical: &str, requested: Option<&str>) -> Option<String> {
    let requested = requested?;
    let supported = REASONING_EFFORTS.iter().find(|(model, _)| *model == canonical).map(|(_, efforts)| *efforts)?;
    if supported.contains(&requested) {
        return Some(requested.to_string());
    }
    let req_rank = effort_rank(requested);
    let mut best: Option<&str> = None;
    for effort in supported {
        if effort_rank(effort) <= req_rank && best.map(|b| effort_rank(effort) >= effort_rank(b)).unwrap_or(true) {
            best = Some(effort);
        }
    }
    if let Some(found) = best {
        return Some(found.to_string());
    }
    supported.iter().min_by_key(|effort| effort_rank(effort)).map(|effort| effort.to_string())
}

pub fn is_placeholder_token(token: &str) -> bool {
    let norm = token.trim().to_lowercase().replace('_', "-");
    if norm.is_empty() || PLACEHOLDER_TOKENS.contains(&norm.as_str()) {
        return true;
    }
    PLACEHOLDER_PREFIXES.iter().any(|prefix| norm.starts_with(prefix))
}

/// True when an OpenAI chat body carries image/file parts. The
/// `/alpha/generate` image shape (`{"type":"image"}`) is unverified, so the
/// gateway rejects these honestly instead of mistranslating them.
pub fn has_unsupported_parts(body: &serde_json::Value) -> bool {
    let Some(messages) = body.get("messages").and_then(|v| v.as_array()) else {
        return false;
    };
    for msg in messages {
        let items = match msg.get("content") {
            Some(serde_json::Value::Array(items)) => items,
            _ => continue,
        };
        for item in items {
            let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if kind != "text" && item.get("text").and_then(|v| v.as_str()).is_none() {
                return true;
            }
        }
    }
    false
}

pub fn context_window(model: &str) -> Option<u32> {
    CONTEXT_WINDOWS.iter().find(|(name, _)| *name == model).map(|(_, window)| *window)
}

pub fn text_from_content(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                let kind = item.get("type")?.as_str()?;
                if kind == "text" || kind == "input_text" {
                    item.get("text")?.as_str().map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub fn map_finish_reason(reason: Option<&str>, has_tool_calls: bool) -> &'static str {
    if has_tool_calls {
        return "tool_calls";
    }
    match reason {
        Some("tool-calls" | "tool_calls" | "tool-call" | "tool_use" | "toolUse") => "tool_calls",
        Some("length" | "max_tokens" | "max-tokens" | "max_output_tokens") => "length",
        _ => "stop",
    }
}

pub fn map_usage_from_finish_event(finish_event: Option<&serde_json::Value>) -> serde_json::Value {
    let usage = finish_event.and_then(|event| event.get("totalUsage").or_else(|| event.get("usage")));
    let Some(usage) = usage.and_then(|value| value.as_object()) else {
        return serde_json::json!({"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0});
    };
    let number = |keys: &[&str]| -> u64 {
        keys.iter()
            .filter_map(|key| usage.get(*key))
            .filter_map(serde_json::Value::as_u64)
            .next()
            .unwrap_or(0)
    };
    let input = number(&["inputTokens", "promptTokens", "prompt_tokens"]);
    let completion = number(&["outputTokens", "completionTokens", "completion_tokens"]);
    let (cache_read, cache_write) = usage
        .get("inputTokenDetails")
        .and_then(serde_json::Value::as_object)
        .map(|details| {
            let read = ["cacheReadTokens", "cachedTokens"]
                .iter()
                .filter_map(|key| details.get(*key))
                .filter_map(serde_json::Value::as_u64)
                .next()
                .unwrap_or(0);
            let write = details.get("cacheWriteTokens").and_then(serde_json::Value::as_u64).unwrap_or(0);
            (read, write)
        })
        .unwrap_or((0, 0));
    let prompt = input + cache_read + cache_write;
    serde_json::json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": prompt + completion,
        "prompt_tokens_details": {"cached_tokens": cache_read, "cache_write_tokens": cache_write},
        "cache_read_input_tokens": cache_read,
        "cache_creation_input_tokens": cache_write,
    })
}

pub fn map_usage_to_openai(input_tokens: u64, output_tokens: u64) -> serde_json::Value {
    serde_json::json!({
        "prompt_tokens": input_tokens,
        "completion_tokens": output_tokens,
        "total_tokens": input_tokens + output_tokens,
    })
}

pub fn slugify_working_dir(working_dir: &str) -> String {
    let base = working_dir.replace('\\', "/").rsplit('/').next().unwrap_or("").to_string();
    let base = if base.is_empty() { "commandcode-proxy".to_string() } else { base };
    let slug: String = base
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    slug.chars().take(40).collect::<String>()
}

// ---------------------------------------------------------------------------
// Go-plan bridge: OpenAI-compatible <-> CLI-style `/alpha/generate`.
//
// Ported from the approved local bridge
// (`C:/projects/commandcode-go-proxy/commandcode_proxy.py`). Go-plan keys get
// 403 on the documented Provider API, so their traffic rides this private,
// subscription-specific route instead. Isolated in this module, gated behind
// an explicit local token plus the `PROXYDOCK_GO_BRIDGE` flag (old `PROXYHUB_GO_BRIDGE` still works), and verified
// only via user-directed smoke tests. The `:8788` Python bridge stays
// untouched until this adapter is proven.
// ---------------------------------------------------------------------------

/// Base URL override, mirroring the Python bridge (`COMMANDCODE_API_BASE`).
pub const BRIDGE_API_BASE_DEFAULT: &str = "https://api.commandcode.ai";
/// CLI version sent upstream — the value proven against `/alpha/generate`.
pub const BRIDGE_CC_VERSION: &str = "0.40.3";
pub const BRIDGE_GENERATE_PATH: &str = "/alpha/generate";
pub const BRIDGE_MAX_TOKENS_DEFAULT: u32 = 32000;
pub const BRIDGE_MAX_RETRIES: u32 = 2;
/// Upper bound for a single non-streaming bridge response body (folded
/// incrementally, never buffered whole).
pub const BRIDGE_GENERATE_MAX_BYTES: usize = 32 * 1024 * 1024;

pub fn bridge_api_base() -> String {
    std::env::var("COMMANDCODE_API_BASE")
        .ok()
        .map(|base| base.trim_end_matches('/').to_string())
        .filter(|base| !base.is_empty())
        .unwrap_or_else(|| BRIDGE_API_BASE_DEFAULT.to_string())
}

/// Feature flag for Go-leg sends. `PROXYDOCK_GO_BRIDGE=0` (or `false`)
/// disables; enabled by default once a local token is present.
/// The old `PROXYHUB_GO_BRIDGE` name still works as a fallback.
pub fn go_bridge_enabled() -> bool {
    let raw = std::env::var("PROXYDOCK_GO_BRIDGE")
        .or_else(|_| std::env::var("PROXYHUB_GO_BRIDGE"))
        .map(|v| v.trim().to_lowercase());
    match raw {
        Ok(v) if v == "0" || v == "false" || v == "off" || v == "no" => false,
        _ => true,
    }
}

/// True only for the Go display plan (or its `individual-go` plan id).
/// Exact match — `GOAT` must NOT match.
pub fn is_go_plan(plan: Option<&str>) -> bool {
    match plan {
        None => false,
        Some(raw) => {
            let key = raw.trim().to_lowercase().replace('_', "-");
            key == "go" || key == "individual-go"
        }
    }
}

#[derive(Debug, Clone)]
pub struct BridgeError {
    pub message: String,
    pub status: u16,
    /// 429 / 5xx / timeouts before any content: safe to roll to the next
    /// account. Auth and validation errors are terminal for this account.
    pub retryable: bool,
}

impl BridgeError {
    fn terminal(message: impl Into<String>, status: u16) -> Self {
        BridgeError { message: message.into(), status, retryable: false }
    }
}

/// Retryable iff upstream rate-limited us or failed server-side.
pub fn is_retryable_status(status: u16) -> bool {
    status == 429 || status >= 500
}

/// Credential failures are always terminal (never roll on auth errors).
pub fn is_auth_status(status: u16) -> bool {
    status == 401 || status == 403
}

/// Request-scoped values baked into the alpha payload / headers.
pub struct AlphaContext {
    pub working_dir: String,
    pub date: String,
    pub environment: String,
    pub client_version: String,
}

impl AlphaContext {
    pub fn local() -> Self {
        let dir = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
        AlphaContext {
            working_dir: dir,
            date: chrono::Local::now().format("%Y-%m-%d").to_string(),
            environment: format!("{}-{}, proxy-dock", std::env::consts::OS, std::env::consts::ARCH),
            client_version: BRIDGE_CC_VERSION.to_string(),
        }
    }
}

fn parse_args(value: Option<&serde_json::Value>) -> serde_json::Value {
    match value {
        Some(serde_json::Value::Object(_)) => value.cloned().unwrap_or(serde_json::Value::Object(Default::default())),
        Some(serde_json::Value::String(text)) if !text.trim().is_empty() => {
            serde_json::from_str(text).unwrap_or(serde_json::Value::Object(Default::default()))
        }
        _ => serde_json::Value::Object(Default::default()),
    }
}

/// OpenAI messages -> (system text, Command Code messages), mirroring
/// `_openai_messages_to_commandcode` including tool-result pairing prune.
pub fn openai_messages_to_cc(messages: &serde_json::Value) -> (String, Vec<serde_json::Value>) {
    let items = messages.as_array().cloned().unwrap_or_default();
    let mut tool_name_by_id: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for m in &items {
        if let Some(calls) = m.get("tool_calls").and_then(|v| v.as_array()) {
            for tc in calls {
                if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                    let name = tc.get("function").and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("");
                    tool_name_by_id.insert(id.to_string(), name.to_string());
                }
            }
        }
    }
    let mut system_parts: Vec<String> = Vec::new();
    let mut converted: Vec<serde_json::Value> = Vec::new();
    for message in &items {
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let content = message.get("content").cloned().unwrap_or(serde_json::Value::Null);
        match role {
            "system" | "developer" => {
                let text = text_from_content(&content);
                if !text.is_empty() {
                    system_parts.push(text);
                }
            }
            "user" => {
                if let Some(parts) = content.as_array() {
                    let mut cc_parts: Vec<serde_json::Value> = Vec::new();
                    for item in parts {
                        match item.get("type").and_then(|v| v.as_str()) {
                            Some("image_url") => {
                                let url = item.get("image_url").and_then(|u| u.get("url")).and_then(|v| v.as_str()).unwrap_or("");
                                if !url.is_empty() {
                                    cc_parts.push(serde_json::json!({"type": "image", "image": url}));
                                }
                            }
                            Some("text") => {
                                if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                                    if !t.is_empty() {
                                        cc_parts.push(serde_json::json!({"type": "text", "text": t}));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    if cc_parts.len() == 1 && cc_parts[0].get("type").and_then(|v| v.as_str()) == Some("text") {
                        converted.push(serde_json::json!({"role": "user", "content": cc_parts[0].get("text").cloned().unwrap_or_default()}));
                    } else if !cc_parts.is_empty() {
                        converted.push(serde_json::json!({"role": "user", "content": cc_parts}));
                    } else {
                        converted.push(serde_json::json!({"role": "user", "content": text_from_content(&content)}));
                    }
                } else {
                    converted.push(serde_json::json!({"role": "user", "content": text_from_content(&content)}));
                }
            }
            "assistant" => {
                let mut parts: Vec<serde_json::Value> = Vec::new();
                let text = text_from_content(&content);
                if !text.is_empty() {
                    parts.push(serde_json::json!({"type": "text", "text": text}));
                }
                let reasoning = message
                    .get("reasoning_content")
                    .or_else(|| message.get("reasoning"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !reasoning.is_empty() {
                    parts.push(serde_json::json!({"type": "reasoning", "text": reasoning}));
                }
                if let Some(calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
                    for call in calls {
                        let func = call.get("function").cloned().unwrap_or(serde_json::Value::Null);
                        parts.push(serde_json::json!({
                            "type": "tool-call",
                            "toolCallId": call.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                            "toolName": func.get("name").and_then(|v| v.as_str()).or_else(|| call.get("name").and_then(|v| v.as_str())).unwrap_or(""),
                            "input": parse_args(func.get("arguments").or_else(|| call.get("arguments"))),
                        }));
                    }
                    // Fill in generated ids where the client omitted them.
                    for part in parts.iter_mut().filter(|p| p.get("type").and_then(|v| v.as_str()) == Some("tool-call")) {
                        if part.get("toolCallId").and_then(|v| v.as_str()).map(|s| s.is_empty()).unwrap_or(true) {
                            part["toolCallId"] = serde_json::Value::String(format!("call_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]));
                        }
                    }
                }
                if !parts.is_empty() {
                    converted.push(serde_json::json!({"role": "assistant", "content": parts}));
                }
            }
            "tool" => {
                let tool_call_id = message
                    .get("tool_call_id")
                    .or_else(|| message.get("toolCallId"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let tool_name = message
                    .get("name")
                    .or_else(|| message.get("tool_name"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| tool_name_by_id.get(tool_call_id).cloned().unwrap_or_default());
                converted.push(serde_json::json!({
                    "role": "tool",
                    "content": [{"type": "tool-result", "toolCallId": tool_call_id, "toolName": tool_name, "output": {"type": "text", "value": text_from_content(&content)}}],
                }));
            }
            _ => {}
        }
    }
    // Prune dangling tool calls/results — CC requires pairing.
    let mut call_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut result_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for msg in &converted {
        if let Some(parts) = msg.get("content").and_then(|v| v.as_array()) {
            for p in parts {
                match p.get("type").and_then(|v| v.as_str()) {
                    Some("tool-call") => {
                        if let Some(id) = p.get("toolCallId").and_then(|v| v.as_str()) {
                            call_ids.insert(id.to_string());
                        }
                    }
                    Some("tool-result") => {
                        if let Some(id) = p.get("toolCallId").and_then(|v| v.as_str()) {
                            result_ids.insert(id.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let valid: std::collections::HashSet<String> = call_ids.intersection(&result_ids).cloned().collect();
    if valid != call_ids || valid != result_ids {
        let mut pruned: Vec<serde_json::Value> = Vec::new();
        for msg in converted {
            match msg.get("content").and_then(|v| v.as_array()) {
                Some(_) => {
                    let filtered: Vec<serde_json::Value> = msg["content"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|p| match p.get("type").and_then(|v| v.as_str()) {
                            Some("tool-call") | Some("tool-result") => {
                                p.get("toolCallId").and_then(|v| v.as_str()).map(|id| valid.contains(id)).unwrap_or(false)
                            }
                            _ => true,
                        })
                        .collect();
                    if !filtered.is_empty() {
                        pruned.push(serde_json::json!({"role": msg.get("role").cloned().unwrap_or_default(), "content": filtered}));
                    }
                }
                None => pruned.push(msg),
            }
        }
        converted = pruned;
    }
    (system_parts.join("\n\n"), converted)
}

/// OpenAI tools -> Command Code function tools.
pub fn openai_tools_to_cc(tools: Option<&serde_json::Value>) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let Some(items) = tools.and_then(|v| v.as_array()) else { return out };
    for tool in items {
        let func = match tool.get("function") {
            Some(f @ serde_json::Value::Object(_)) => f.clone(),
            _ => tool.clone(),
        };
        let Some(name) = func.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) else { continue };
        out.push(serde_json::json!({
            "type": "function",
            "name": name,
            "description": func.get("description").and_then(|v| v.as_str()).unwrap_or(""),
            "input_schema": func.get("parameters").cloned().unwrap_or_else(|| serde_json::json!({})),
        }));
    }
    out
}

/// OpenAI tool_choice -> Command Code tool_choice.
pub fn map_tool_choice(tool_choice: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    match tool_choice {
        Some(serde_json::Value::String(mode)) if mode == "required" => Some(serde_json::json!({"type": "any"})),
        Some(serde_json::Value::Object(_)) => {
            let kind = tool_choice.and_then(|v| v.get("type")).and_then(|v| v.as_str()).unwrap_or("");
            if kind == "function" {
                let name = tool_choice.and_then(|v| v.get("function")).and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("");
                Some(serde_json::json!({"type": "tool", "name": name}))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn clamp_max_tokens(request: &serde_json::Value) -> u32 {
    request
        .get("max_tokens")
        .or_else(|| request.get("max_completion_tokens"))
        .and_then(serde_json::Value::as_u64)
        .map(|n| n.clamp(1, 200000) as u32)
        .unwrap_or(BRIDGE_MAX_TOKENS_DEFAULT)
}

/// Full OpenAI chat request -> `/alpha/generate` payload, mirroring
/// `_build_alpha_payload` (model/alias + effort + tool_choice + the
/// chat-only no-tools safeguard).
pub fn build_alpha_payload(request: &serde_json::Value, ctx: &AlphaContext, thread_id: &str) -> serde_json::Value {
    let (mut system, mut messages) = openai_messages_to_cc(request.get("messages").unwrap_or(&serde_json::Value::Null));
    let model_raw = request.get("model").and_then(|v| v.as_str()).unwrap_or("");
    // No silent default: an empty model passes through and the upstream 400
    // surfaces honestly (the gateway already rejects empty models first).
    let model = resolve_model(model_raw);
    let requested_effort = request
        .get("reasoning_effort")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            request.get("reasoning").and_then(|r| r.get("effort")).and_then(|v| v.as_str()).map(str::to_string)
        });
    // Mirror the bridge: pass the requested effort through when the model
    // has no effort table, otherwise clamp into the supported set.
    let effort = requested_effort.map(|r| resolve_effort(&model, Some(r.as_str())).unwrap_or(r));
    let cc_tool_choice = map_tool_choice(request.get("tool_choice"));
    let has_tools = request.get("tools").and_then(|v| v.as_array()).map(|a| !a.is_empty()).unwrap_or(false);
    let effective_max_tokens = clamp_max_tokens(request);
    tracing::trace!(max_tokens = effective_max_tokens, has_tools, "bridge payload");
    if !has_tools {
        // Observability only: trace when the chat-only safeguard fires.
        // No secrets, no message content — fires + effective cap only.
        tracing::trace!("bridge NO_TOOLS safeguard");
        const NO_TOOLS: &str = "CRITICAL: You are running in a chat-only environment. Tool execution is disabled. Do not generate or call any tools. Respond only with plain text.";
        system = if system.is_empty() { NO_TOOLS.to_string() } else { format!("{system}\n\n{NO_TOOLS}") };
        const SUFFIX: &str = "\n\n[System Note: Tool execution is disabled. Do not output any tool calls. Answer directly in plain text.]";
        for i in (0..messages.len()).rev() {
            if messages[i].get("role").and_then(|v| v.as_str()) != Some("user") {
                continue;
            }
            match messages[i].get("content") {
                Some(serde_json::Value::String(_)) => {
                    let text = messages[i]["content"].as_str().unwrap_or("").to_string();
                    messages[i]["content"] = serde_json::Value::String(format!("{text}{SUFFIX}"));
                }
                Some(serde_json::Value::Array(_)) => {
                    let parts = messages[i]["content"].as_array_mut().unwrap();
                    match parts.iter_mut().rev().find(|p| p.get("type").and_then(|v| v.as_str()) == Some("text")) {
                        Some(p) => {
                            let text = p.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                            p["text"] = serde_json::Value::String(format!("{text}{SUFFIX}"));
                        }
                        None => parts.push(serde_json::json!({"type": "text", "text": SUFFIX.trim_start()})),
                    }
                }
                _ => {}
            }
            break;
        }
    }
    let tools = openai_tools_to_cc(request.get("tools"));
    let mut params = serde_json::json!({
        "model": model,
        "messages": messages,
        "tools": tools,
        "system": system,
        "max_tokens": effective_max_tokens,
        "stream": true,
    });
    if tools.is_empty() {
        params.as_object_mut().and_then(|o| o.remove("tools"));
    }
    if system.is_empty() {
        // `system` was moved into params; re-check via the local copy.
        params.as_object_mut().and_then(|o| o.remove("system"));
    }
    if let Some(effort) = effort {
        params["reasoning_effort"] = serde_json::Value::String(effort);
    }
    if let Some(choice) = cc_tool_choice {
        params["tool_choice"] = choice;
    }
    serde_json::json!({
        "config": {
            "workingDir": ctx.working_dir,
            "date": ctx.date,
            "environment": ctx.environment,
            "structure": [],
            "isGitRepo": false,
            "currentBranch": "",
            "mainBranch": "",
            "gitStatus": "",
            "recentCommits": [],
        },
        "memory": "",
        "taste": "",
        "skills": null,
        "permissionMode": "standard",
        "params": params,
        "threadId": thread_id,
    })
}

/// Headers for `/alpha/generate`, mirroring `_build_headers` (CLI-shaped
/// UA/version/slug/traceparent so the private route does not flag the call).
/// `Accept-Encoding` is intentionally omitted — reqwest decodes transparently.
pub fn alpha_headers(api_key: &str, thread_id: &str, working_dir: &str, client_version: &str) -> Vec<(String, String)> {
    let trace_id = uuid::Uuid::new_v4().simple().to_string();
    let parent_id = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
    vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Accept".to_string(), "application/json, */*".to_string()),
        ("Accept-Language".to_string(), "en-US,en;q=0.9".to_string()),
        ("Connection".to_string(), "keep-alive".to_string()),
        ("User-Agent".to_string(), format!("commandcode-cli/{client_version}")),
        ("Authorization".to_string(), format!("Bearer {}", api_key.trim())),
        ("x-cli-environment".to_string(), "production".to_string()),
        ("x-command-code-version".to_string(), client_version.to_string()),
        ("x-session-id".to_string(), thread_id.to_string()),
        ("x-co-flag".to_string(), "false".to_string()),
        ("x-taste-learning".to_string(), "false".to_string()),
        ("x-project-slug".to_string(), slugify_working_dir(working_dir)),
        ("traceparent".to_string(), format!("00-{trace_id}-{parent_id}-01")),
    ]
}

/// Long-lived client for the generate stream (the bridge allows 300s).
pub fn bridge_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// POST the payload; maps HTTP failures to [`BridgeError`] with the bridge's
/// retry classification (429/5xx retryable, 401/403 terminal).
/// POST the payload and return the upstream response once headers arrive,
/// retrying retryable pre-stream failures with backoff (mirrors the bridge).
/// Callers roll accounts on retryable errors and stop on terminal ones.
pub async fn open_alpha_stream(
    client: &reqwest::Client,
    api_key: &str,
    payload: &serde_json::Value,
    thread_id: &str,
    working_dir: &str,
) -> Result<reqwest::Response, BridgeError> {
    post_alpha_generate(client, api_key, payload, thread_id, working_dir).await
}

async fn post_alpha_generate(
    client: &reqwest::Client,
    api_key: &str,
    payload: &serde_json::Value,
    thread_id: &str,
    working_dir: &str,
) -> Result<reqwest::Response, BridgeError> {
    let url = format!("{}{}", bridge_api_base(), BRIDGE_GENERATE_PATH);
    // Single retry layer: transport timeouts retry here (bounded); HTTP
    // statuses return immediately so the gateway's caller-roll is the ONLY
    // retry layer for 429/5xx (no double-retry pressure on upstream).
    let mut attempt = 0u32;
    loop {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in alpha_headers(api_key, thread_id, working_dir, BRIDGE_CC_VERSION) {
            use std::str::FromStr;
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_str(&name),
                reqwest::header::HeaderValue::from_str(&value),
            ) {
                map.insert(name, value);
            }
        }
        let builder = client.post(&url).headers(map).json(payload);
        match tokio::time::timeout(std::time::Duration::from_secs(300), builder.send()).await {
            Err(_) if attempt < BRIDGE_MAX_RETRIES => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
                continue;
            }
            Err(_) => {
                return Err(BridgeError { message: "Command Code upstream timed out".to_string(), status: 502, retryable: true });
            }
            Ok(Err(err)) => {
                let msg = err.to_string().to_lowercase();
                if (msg.contains("timed out") || msg.contains("timeout")) && attempt < BRIDGE_MAX_RETRIES {
                    attempt += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
                    continue;
                }
                return Err(BridgeError { message: format!("Command Code upstream error: {}", truncate(&err.to_string(), 1000)), status: 502, retryable: true });
            }
            Ok(Ok(resp)) => {
                let status = resp.status().as_u16();
                if status >= 400 {
                    let text = resp.text().await.unwrap_or_default();
                    let snippet = truncate(text.trim(), 2000);
                    if is_auth_status(status) {
                        return Err(BridgeError::terminal(format!("Command Code API {status}: {snippet}"), status));
                    }
                    // No inner retry on HTTP statuses: the gateway rolls to
                    // the next account on retryable errors (single layer).
                    return Err(BridgeError {
                        message: format!("Command Code API {status}: {snippet}"),
                        status,
                        retryable: is_retryable_status(status),
                    });
                }
                return Ok(resp);
            }
        }
    }
}

/// Accumulated non-streaming result.
#[derive(Debug, Clone, Default)]
pub struct CompletionResult {
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<serde_json::Value>,
    pub finish_reason: String,
    pub usage: serde_json::Value,
}

/// Fold one normalized event into the accumulator. `Err` = stream `error`
/// event (retryable — nothing deliverable has been produced by itself).
pub fn fold_bridge_event(acc: &mut CompletionResult, event: &CcEvent) -> Result<(), BridgeError> {
    let data = &event.data;
    let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
    match event.event_type.as_str() {
        "text-delta" if !text.is_empty() => acc.text.push_str(text),
        "reasoning-delta" if !text.is_empty() => acc.reasoning.push_str(text),
        "reasoning-end" | "reasoning-start" | "start" => {}
        "tool-call" | "tool_call" => {
            let arguments = data.get("input").or_else(|| data.get("args")).or_else(|| data.get("arguments")).cloned().unwrap_or(serde_json::Value::Object(Default::default()));
            let id = data.get("toolCallId").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| format!("call_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]));
            let name = data.get("toolName").or_else(|| data.get("name")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let entry = serde_json::json!({"id": id, "type": "function", "function": {"name": name, "arguments": arguments_string(&arguments)}});
            match acc.tool_calls.iter_mut().find(|tc| tc.get("id").and_then(|v| v.as_str()) == Some(id.as_str())) {
                Some(existing) => *existing = entry,
                None => acc.tool_calls.push(entry),
            }
        }
        "tool-call-delta" => {
            let id = data.get("toolCallId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let piece = data.get("arguments").or_else(|| data.get("text")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            match acc.tool_calls.iter_mut().find(|tc| tc.get("id").and_then(|v| v.as_str()) == Some(id.as_str())) {
                Some(existing) => {
                    let args = existing["function"]["arguments"].as_str().unwrap_or("").to_string();
                    existing["function"]["arguments"] = serde_json::Value::String(format!("{args}{piece}"));
                    if !name.is_empty() && existing["function"]["name"].as_str().unwrap_or("").is_empty() {
                        existing["function"]["name"] = serde_json::Value::String(name);
                    }
                }
                None => acc.tool_calls.push(serde_json::json!({
                    "id": if id.is_empty() { format!("call_{}", &uuid::Uuid::new_v4().simple().to_string()[..12]) } else { id },
                    "type": "function",
                    "function": {"name": name, "arguments": piece},
                })),
            }
        }
        "finish" => {
            acc.finish_event(data);
        }
        "error" => {
            let message = match data.get("error") {
                Some(serde_json::Value::Object(map)) => map.get("message").and_then(|v| v.as_str()).unwrap_or("Command Code stream error").to_string(),
                Some(v) => v.as_str().unwrap_or("Command Code stream error").to_string(),
                None => "Command Code stream error".to_string(),
            };
            return Err(BridgeError { message, status: 502, retryable: true });
        }
        _ => {}
    }
    Ok(())
}

fn arguments_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()),
    }
}

impl CompletionResult {
    fn finish_event(&mut self, data: &serde_json::Value) {
        self.finish_reason = map_finish_reason(data.get("finishReason").and_then(|v| v.as_str()), !self.tool_calls.is_empty()).to_string();
        self.usage = map_usage_from_finish_event(Some(data));
    }

    /// True when the bridge produced no deliverable content (empty text and
    /// no tool calls). Thought-only streams hit this.
    pub fn is_empty_content(&self) -> bool {
        self.text.trim().is_empty() && self.tool_calls.is_empty()
    }

    /// OpenAI `chat.completion` object, mirroring `_non_stream`. Returns an
    /// honest rollable 502 when the bridge yields no content instead of a
    /// 200-empty.
    pub fn to_openai(&self, completion_id: &str, model: &str, created: u64) -> Result<serde_json::Value, BridgeError> {
        if self.is_empty_content() {
            return Err(BridgeError { message: "upstream returned no content".to_string(), status: 502, retryable: true });
        }
        let mut message = serde_json::json!({"role": "assistant", "content": self.text});
        if !self.reasoning.is_empty() {
            message["reasoning_content"] = serde_json::Value::String(self.reasoning.clone());
        }
        if !self.tool_calls.is_empty() {
            message["tool_calls"] = serde_json::Value::Array(self.tool_calls.clone());
        }
        let usage = if self.usage.is_null() { map_usage_to_openai(0, 0) } else { self.usage.clone() };
        Ok(serde_json::json!({
            "id": completion_id,
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [{"index": 0, "message": message, "finish_reason": self.finish_reason}],
            "usage": usage,
        }))
    }
}

/// Inline `success: false` event bodies are upstream errors, mirroring the
/// bridge's pre-stream error check.
pub fn bridge_inline_error(event: &CcEvent) -> Option<BridgeError> {
    let success = event.data.get("success").and_then(|v| v.as_bool()).unwrap_or(true);
    if success {
        return None;
    }
    let message = match event.data.get("error") {
        Some(serde_json::Value::Object(map)) => map.get("message").and_then(|v| v.as_str()).unwrap_or("Command Code upstream error").to_string(),
        Some(v) => v.as_str().unwrap_or("Command Code upstream error").to_string(),
        None => "Command Code upstream error".to_string(),
    };
    Some(BridgeError { message, status: 502, retryable: true })
}

/// Non-streaming generate: POST, fold the event stream incrementally with a
/// byte cap, return the accumulated result. Retryable failures throw — the
/// caller rolls accounts.
pub async fn generate_once(
    client: &reqwest::Client,
    api_key: &str,
    payload: &serde_json::Value,
    thread_id: &str,
    working_dir: &str,
) -> Result<CompletionResult, BridgeError> {
    let mut resp = post_alpha_generate(client, api_key, payload, thread_id, working_dir).await?;
    let mut acc = CompletionResult { finish_reason: "stop".to_string(), usage: map_usage_to_openai(0, 0), ..Default::default() };
    let mut buffer: Vec<u8> = Vec::new();
    let mut total: usize = 0;
    loop {
        let chunk = resp.chunk().await.map_err(|err| BridgeError {
            message: format!("Command Code stream read failed: {err}"),
            status: 502,
            retryable: true,
        })?;
        let Some(bytes) = chunk else { break };
        total = total.saturating_add(bytes.len());
        if total > BRIDGE_GENERATE_MAX_BYTES {
            return Err(BridgeError { message: "upstream response too large".to_string(), status: 502, retryable: true });
        }
        buffer.extend_from_slice(&bytes);
        while let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=pos).collect();
            let text = String::from_utf8_lossy(&line);
            let Some(event) = parse_cc_line(&text) else { continue };
            if let Some(err) = bridge_inline_error(&event) {
                return Err(err);
            }
            fold_bridge_event(&mut acc, &event)?;
        }
    }
    // Trailing partial line (upstream closed without newline).
    if !buffer.is_empty() {
        let text = String::from_utf8_lossy(&buffer);
        if let Some(event) = parse_cc_line(&text) {
            if let Some(err) = bridge_inline_error(&event) {
                return Err(err);
            }
            fold_bridge_event(&mut acc, &event)?;
        }
    }
    Ok(acc)
}

/// SSE streaming state shared with the router's forwarding task.
#[derive(Debug, Default)]
pub struct SseState {
    pub tool_index: u32,
    pub tool_index_by_id: std::collections::HashMap<String, u32>,
    pub has_tool_calls: bool,
}

/// One normalized event -> zero or more OpenAI SSE data lines (no framing).
/// Mirrors the bridge's `handle_event`, minus terminal framing.
pub fn translate_stream_event(
    completion_id: &str,
    model: &str,
    created: u64,
    event: &CcEvent,
    state: &mut SseState,
    acc: &mut CompletionResult,
) -> Result<Vec<String>, BridgeError> {
    let data = &event.data;
    let chunk = |delta: serde_json::Value| -> String {
        serde_json::json!({"id": completion_id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{"index": 0, "delta": delta, "finish_reason": null}]}).to_string()
    };
    match event.event_type.as_str() {
        "text-delta" => {
            let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if text.is_empty() {
                return Ok(vec![]);
            }
            acc.text.push_str(text);
            Ok(vec![chunk(serde_json::json!({"content": text}))])
        }
        "reasoning-delta" => {
            let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if text.is_empty() {
                return Ok(vec![]);
            }
            acc.reasoning.push_str(text);
            Ok(vec![chunk(serde_json::json!({"reasoning_content": text}))])
        }
        "reasoning-end" | "reasoning-start" | "start" => Ok(vec![]),
        "tool-call" | "tool_call" => {
            fold_bridge_event(acc, event)?;
            state.has_tool_calls = true;
            let last = acc.tool_calls.last().cloned().unwrap_or(serde_json::Value::Null);
            let id = last.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let idx = match state.tool_index_by_id.get(&id) {
                Some(i) => *i,
                None => {
                    let i = state.tool_index;
                    state.tool_index += 1;
                    state.tool_index_by_id.insert(id.clone(), i);
                    i
                }
            };
            Ok(vec![chunk(serde_json::json!({"tool_calls": [{"index": idx, "id": id, "type": "function", "function": last.get("function").cloned().unwrap_or_default()}] }))])
        }
        "tool-call-delta" => {
            let before = acc.tool_calls.clone();
            fold_bridge_event(acc, event)?;
            state.has_tool_calls = true;
            let id = data.get("toolCallId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let idx = match state.tool_index_by_id.get(&id) {
                Some(i) => *i,
                None => {
                    let i = state.tool_index;
                    state.tool_index += 1;
                    if !id.is_empty() {
                        state.tool_index_by_id.insert(id.clone(), i);
                    }
                    i
                }
            };
            // Emit only the newly arrived argument piece.
            let piece = data.get("arguments").or_else(|| data.get("text")).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let name = if before.len() != acc.tool_calls.len() { data.get("name").and_then(|v| v.as_str()).unwrap_or("") } else { "" };
            let mut tc = serde_json::json!({"index": idx, "function": {"arguments": piece}});
            if !id.is_empty() {
                tc["id"] = serde_json::Value::String(id);
                tc["type"] = serde_json::Value::String("function".to_string());
            }
            if !name.is_empty() {
                tc["function"]["name"] = serde_json::Value::String(name.to_string());
            }
            Ok(vec![chunk(serde_json::json!({"tool_calls": [tc]}))])
        }
        "finish" => {
            fold_bridge_event(acc, event)?;
            Ok(vec![])
        }
        "error" => {
            fold_bridge_event(acc, event).map(|()| vec![])
        }
        _ => Ok(vec![]),
    }
}

pub fn sse_role_chunk(completion_id: &str, model: &str, created: u64) -> String {
    serde_json::json!({"id": completion_id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{"index": 0, "delta": {"role": "assistant"}, "finish_reason": null}]}).to_string()
}

pub fn sse_finish_chunk(completion_id: &str, model: &str, created: u64, acc: &CompletionResult) -> String {
    let usage = if acc.usage.is_null() { map_usage_to_openai(0, 0) } else { acc.usage.clone() };
    serde_json::json!({"id": completion_id, "object": "chat.completion.chunk", "created": created, "model": model, "choices": [{"index": 0, "delta": {}, "finish_reason": acc.finish_reason}], "usage": usage}).to_string()
}

pub fn new_completion_id() -> String {
    format!("chatcmpl-{}", uuid::Uuid::new_v4().simple())
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcEvent {
    pub event_type: String,
    pub data: serde_json::Value,
}

pub fn parse_cc_line(line: &str) -> Option<CcEvent> {
    let mut text = line.trim();
    if text.is_empty() || text.starts_with(':') || text.starts_with("event:") {
        return None;
    }
    if let Some(stripped) = text.strip_prefix("data:") {
        text = stripped.trim();
    }
    if text.is_empty() || text == "[DONE]" {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    let event_type = parsed.get("type")?.as_str()?.to_string();
    let mut data = parsed.get("data").and_then(|value| value.as_object()).cloned().unwrap_or_default();
    if let Some(object) = parsed.as_object() {
        for (key, value) in object {
            if key != "type" && !data.contains_key(key) {
                data.insert(key.clone(), value.clone());
            }
        }
    }
    Some(CcEvent { event_type, data: serde_json::Value::Object(data) })
}

// ---------------------------------------------------------------------------
// Non-Go Provider API: OpenAI-compatible passthrough.
//
// Verified against official docs (`commandcode.ai/docs/provider`, Provider API
// launch notes): `POST https://api.commandcode.ai/provider/v1/chat/completions`
// (OpenAI), `/responses` (Responses), `/messages` (Anthropic), Bearer
// `CMD_API_KEY`. Requires Pro plan or higher — Go-plan keys get 403 (no
// Provider API), which is why Go rides the `/alpha/generate` bridge above.
// This module serves the `/chat/completions` surface; Claude models sent here
// surface an honest upstream 400 pointing at `/messages`.
// ---------------------------------------------------------------------------

/// Provider API base (override via `COMMANDCODE_PROVIDER_BASE`).
pub const PROVIDER_API_BASE_DEFAULT: &str = "https://api.commandcode.ai/provider/v1";
pub const PROVIDER_CHAT_PATH: &str = "/chat/completions";

pub fn provider_api_base() -> String {
    std::env::var("COMMANDCODE_PROVIDER_BASE")
        .ok()
        .map(|base| base.trim_end_matches('/').to_string())
        .filter(|base| !base.is_empty())
        .unwrap_or_else(|| PROVIDER_API_BASE_DEFAULT.to_string())
}

/// True when this account should ride the Provider API: any known plan except
/// Go/Free, or legacy `None` (unknown — best-effort attempt; Go keys surface
/// an honest upstream 403 telling the user to refresh quota to detect the
/// plan). `Free`/empty never attempt upstream.
pub fn is_provider_routable(plan: Option<&str>) -> bool {
    match plan {
        None => true,
        Some(raw) => {
            let key = raw.trim().to_lowercase().replace('_', "-");
            !(key.is_empty() || key == "go" || key == "individual-go" || key == "free")
        }
    }
}

/// OpenAI chat body with the native model id + usage flag for streaming sends.
pub fn build_provider_body(native_model: &str, body: &serde_json::Value, stream: bool) -> serde_json::Value {
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

/// POST the chat body to the Provider API. Single attempt — the caller rolls
/// accounts on retryable errors.
pub async fn post_provider_chat(
    client: &reqwest::Client,
    api_key: &str,
    native_model: &str,
    body: &serde_json::Value,
    stream: bool,
) -> Result<reqwest::Response, crate::adapters::AdapterError> {
    use crate::adapters::{is_auth_status, is_retryable_status, truncate_snippet, AdapterError};
    let url = format!("{}{}", provider_api_base(), PROVIDER_CHAT_PATH);
    let payload = build_provider_body(native_model, body, stream);
    let resp = client
        .post(&url)
        .bearer_auth(api_key.trim())
        .header("Content-Type", "application/json")
        .header("Accept", if stream { "text/event-stream" } else { "application/json" })
        .header("x-command-code-version", BRIDGE_CC_VERSION)
        .json(&payload)
        .send()
        .await
        .map_err(|err| AdapterError::retryable(format!("commandcode provider upstream error: {}", truncate_snippet(&err.to_string(), 1000)), 502))?;
    let status = resp.status().as_u16();
    let retry_after = crate::adapters::retry_after_secs(resp.headers());
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        let snippet = truncate_snippet(text.trim(), 2000);
        // Plan-shaped 403s (Go key on the Provider API) roll to the next
        // account; credential 403s stay terminal. Mirrors opencode.rs.
        if status == 403 && crate::adapters::is_plan_403(&snippet) {
            return Err(AdapterError::retryable(format!("commandcode provider API {status}: {snippet}"), status).with_retry_after(retry_after));
        }
        if is_auth_status(status) {
            return Err(AdapterError::terminal(format!("commandcode provider API {status}: {snippet}"), status).with_retry_after(retry_after));
        }
        return Err(AdapterError {
            message: format!("commandcode provider API {status}: {snippet}"),
            status,
            retryable: is_retryable_status(status),
            retry_after,
        });
    }
    Ok(resp)
}

/// Native-wire passthrough (1B): `path` is `/responses` or `/messages`; the
/// body goes untouched for clients speaking that wire natively.
pub async fn post_provider_native(
    client: &reqwest::Client,
    api_key: &str,
    path: &str,
    body: &serde_json::Value,
    stream: bool,
) -> Result<reqwest::Response, crate::adapters::AdapterError> {
    use crate::adapters::{is_auth_status, is_retryable_status, truncate_snippet, AdapterError};
    let url = format!("{}{}", provider_api_base(), path);
    let resp = client
        .post(&url)
        .bearer_auth(api_key.trim())
        .header("Content-Type", "application/json")
        .header("Accept", if stream { "text/event-stream" } else { "application/json" })
        .header("x-command-code-version", BRIDGE_CC_VERSION)
        .json(body)
        .send()
        .await
        .map_err(|err| AdapterError::retryable(format!("commandcode provider upstream error: {}", truncate_snippet(&err.to_string(), 1000)), 502))?;
    let status = resp.status().as_u16();
    let retry_after = crate::adapters::retry_after_secs(resp.headers());
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        let snippet = truncate_snippet(text.trim(), 2000);
        if status == 403 && crate::adapters::is_plan_403(&snippet) {
            return Err(AdapterError::retryable(format!("commandcode provider API {status}: {snippet}"), status).with_retry_after(retry_after));
        }
        if is_auth_status(status) {
            return Err(AdapterError::terminal(format!("commandcode provider API {status}: {snippet}"), status).with_retry_after(retry_after));
        }
        return Err(AdapterError {
            message: format!("commandcode provider API {status}: {snippet}"),
            status,
            retryable: is_retryable_status(status),
            retry_after,
        });
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_resolution_is_case_insensitive() {
        assert_eq!(resolve_alias("GLM-5.2"), "zai-org/GLM-5.2");
        assert_eq!(resolve_alias("Qwen/Qwen3.7-Max"), "Qwen/Qwen3.7-Max");
        assert_eq!(resolve_alias("deepseek-v4"), "deepseek/deepseek-v4-pro");
        assert_eq!(resolve_alias("kimi-highspeed"), "moonshotai/Kimi-K2.7-Code-Highspeed");
    }

    #[test]
    fn unsupported_parts_gate() {
        let text_only = serde_json::json!({"messages": [{"role": "user", "content": "hi"}]});
        assert!(!has_unsupported_parts(&text_only));
        let text_parts = serde_json::json!({"messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]});
        assert!(!has_unsupported_parts(&text_parts));
        let image = serde_json::json!({"messages": [{"role": "user", "content": [{"type": "text", "text": "see"}, {"type": "image_url", "image_url": {"url": "data:x"}}]}]});
        assert!(has_unsupported_parts(&image));
        assert!(!has_unsupported_parts(&serde_json::json!({})));
    }

    #[test]
    fn resolve_model_never_invents_a_default() {
        // Empty/"default" pass through unchanged: the gateway rejects them
        // with proxy_dock_bad_model instead of silently serving BUILTIN_MODELS.
        assert_eq!(resolve_model("default"), "default");
        assert_eq!(resolve_model(""), "");
        assert_eq!(resolve_model("z-ai/glm-5.3-flash"), "z-ai/glm-5.3-flash");
        assert_eq!(resolve_model("grok-4.5"), "xai/grok-4.5");
    }

    #[test]
    fn missing_usage_maps_to_zero() {
        let usage = map_usage_to_openai(0, 0);
        assert_eq!(usage["total_tokens"], 0);
        let extended = map_usage_from_finish_event(None);
        assert_eq!(extended["total_tokens"], 0);
    }

    #[test]
    fn placeholder_tokens_never_forward() {
        assert!(is_placeholder_token("no-key-required"));
        assert!(is_placeholder_token("  DUMMY-key "));
        assert!(!is_placeholder_token("sk-live-real-token"));
    }

    #[test]
    fn effort_falls_back_within_supported() {
        assert_eq!(resolve_effort("xai/grok-4.5", Some("max")), Some("high".to_string()));
        assert_eq!(resolve_effort("xai/grok-4.5", Some("low")), Some("low".to_string()));
        assert_eq!(resolve_effort("unknown/model", Some("high")), None);
    }

    #[test]
    fn parses_flat_and_nested_cc_events() {
        let flat = parse_cc_line(r#"data: {"type":"text-delta","text":"hi"}"#).expect("flat event");
        assert_eq!(flat.event_type, "text-delta");
        assert_eq!(flat.data["text"], "hi");
        let nested = parse_cc_line(r#"{"type":"text-delta","data":{"text":"yo"}}"#).expect("nested event");
        assert_eq!(nested.data["text"], "yo");
        assert!(parse_cc_line("data: [DONE]").is_none());
        assert!(parse_cc_line(": keep-alive").is_none());
    }

    #[test]
    fn maps_finish_and_usage_shapes() {
        assert_eq!(map_finish_reason(Some("tool_calls"), false), "tool_calls");
        assert_eq!(map_finish_reason(Some("length"), false), "length");
        assert_eq!(map_finish_reason(None, false), "stop");
        let event = serde_json::json!({"totalUsage": {"inputTokens": 10, "outputTokens": 5, "inputTokenDetails": {"cacheReadTokens": 2}}});
        let usage = map_usage_from_finish_event(Some(&event));
        assert_eq!(usage["prompt_tokens"], 12);
        assert_eq!(usage["completion_tokens"], 5);
    }

    #[test]
    fn extracts_text_and_slugifies() {
        let content = serde_json::json!([{"type": "text", "text": "a"}, {"type": "image_url"}]);
        assert_eq!(text_from_content(&content), "a");
        assert_eq!(slugify_working_dir("C:\\projects\\proxy-dock"), "proxy-dock");
    }

    #[test]
    fn go_plan_matches_exactly_not_goat() {
        assert!(is_go_plan(Some("Go")));
        assert!(is_go_plan(Some("go")));
        assert!(is_go_plan(Some("individual-go")));
        assert!(!is_go_plan(Some("GOAT")));
        assert!(!is_go_plan(Some("Pro")));
        assert!(!is_go_plan(Some("Max")));
        assert!(!is_go_plan(None));
        assert!(!is_go_plan(Some("")));
    }

    #[test]
    fn provider_routing_covers_non_go_plans() {
        assert!(is_provider_routable(None));
        assert!(is_provider_routable(Some("Pro")));
        assert!(is_provider_routable(Some("individual-pro")));
        assert!(is_provider_routable(Some("GOAT")));
        assert!(is_provider_routable(Some("Max")));
        assert!(is_provider_routable(Some("Provider")));
        assert!(!is_provider_routable(Some("Go")));
        assert!(!is_provider_routable(Some("individual-go")));
        assert!(!is_provider_routable(Some("Free")));
        assert!(!is_provider_routable(Some("")));
    }

    #[test]
    fn provider_body_sets_native_model() {
        let body = serde_json::json!({"model": "commandcode/deepseek/deepseek-v4-flash", "messages": [], "stream": true});
        let out = build_provider_body("deepseek/deepseek-v4-flash", &body, true);
        assert_eq!(out["model"], "deepseek/deepseek-v4-flash");
        assert_eq!(out["stream_options"]["include_usage"], true);
    }

    #[test]
    fn retry_classification_mirrors_bridge() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
        assert!(!is_retryable_status(403));
        assert!(is_auth_status(401));
        assert!(is_auth_status(403));
        assert!(!is_auth_status(429));
    }

    #[test]
    fn alpha_payload_maps_openai_request() {
        let ctx = AlphaContext {
            working_dir: "C:\\projects\\proxy-dock".to_string(),
            date: "2026-09-27".to_string(),
            environment: "windows-x86_64, proxy-dock".to_string(),
            client_version: BRIDGE_CC_VERSION.to_string(),
        };
        let request = serde_json::json!({
            "model": "grok-4.5",
            "messages": [
                {"role": "system", "content": "be nice"},
                {"role": "user", "content": "hi"},
            ],
            "max_tokens": 500000,
            "reasoning_effort": "max",
            "tool_choice": "required",
        });
        let payload = build_alpha_payload(&request, &ctx, "thread-1");
        assert_eq!(payload["params"]["model"], "xai/grok-4.5");
        // max_tokens clamps to 200000
        assert_eq!(payload["params"]["max_tokens"], 200000);
        // effort clamps into the supported set (max -> high for grok-4.5)
        assert_eq!(payload["params"]["reasoning_effort"], "high");
        assert_eq!(payload["params"]["tool_choice"], serde_json::json!({"type": "any"}));
        // no tools in request -> chat-only safeguard appends to the system
        let system = payload["params"]["system"].as_str().unwrap();
        assert!(system.starts_with("be nice"));
        assert!(system.contains("CRITICAL"));
        assert_eq!(payload["threadId"], "thread-1");
        assert_eq!(payload["permissionMode"], "standard");
        // no tools in request -> tools key removed, safeguard appended
        assert!(payload["params"].get("tools").is_none());
        assert!(payload["params"]["system"].as_str().unwrap().contains("CRITICAL"));
    }

    #[test]
    fn alpha_payload_keeps_tools_and_prunes_dangling() {
        let ctx = AlphaContext {
            working_dir: "/tmp/x".to_string(),
            date: "2026-09-27".to_string(),
            environment: "linux-x86_64, proxy-dock".to_string(),
            client_version: BRIDGE_CC_VERSION.to_string(),
        };
        let request = serde_json::json!({
            "model": "gpt-5.5",
            "messages": [
                {"role": "assistant", "content": "x", "tool_calls": [{"id": "dangling", "type": "function", "function": {"name": "f", "arguments": "{}"}}]},
                {"role": "user", "content": "go"},
            ],
            "tools": [{"type": "function", "function": {"name": "f", "description": "d", "parameters": {"type": "object"}}}],
        });
        let payload = build_alpha_payload(&request, &ctx, "t");
        // dangling tool call (no paired result) is pruned but the text part survives
        let messages = payload["params"]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        let assistant = &messages[0];
        assert_eq!(assistant["role"], "assistant");
        let parts = assistant["content"].as_array().unwrap();
        assert!(parts.iter().all(|p| p.get("type").and_then(|v| v.as_str()) != Some("tool-call")));
        assert_eq!(payload["params"]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(payload["params"]["tools"][0]["name"], "f");
        // effort passes through for models without an effort table
        assert!(payload["params"].get("reasoning_effort").is_none());
    }

    #[test]
    fn alpha_headers_carry_cli_shape() {
        let headers = alpha_headers("sk-test", "thread-9", "C:\\projects\\proxy-dock", BRIDGE_CC_VERSION);
        let get = |name: &str| headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).unwrap_or_default();
        assert_eq!(get("Authorization"), "Bearer sk-test");
        assert_eq!(get("x-command-code-version"), BRIDGE_CC_VERSION);
        assert_eq!(get("x-project-slug"), "proxy-dock");
        assert_eq!(get("x-session-id"), "thread-9");
        let trace = get("traceparent");
        let parts: Vec<&str> = trace.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "00");
        assert_eq!(parts[1].len(), 32);
        assert_eq!(parts[2].len(), 16);
    }

    #[test]
    fn fold_events_into_completion() {
        let mut acc = CompletionResult { finish_reason: "stop".to_string(), usage: map_usage_to_openai(0, 0), ..Default::default() };
        let text = parse_cc_line(r#"data: {"type":"text-delta","text":"hello"}"#).unwrap();
        fold_bridge_event(&mut acc, &text).unwrap();
        let tool = parse_cc_line(r#"{"type":"tool-call","data":{"toolCallId":"c1","toolName":"read","input":{"path":"x"}}}"#).unwrap();
        fold_bridge_event(&mut acc, &tool).unwrap();
        let finish = parse_cc_line(r#"{"type":"finish","data":{"finishReason":"tool-calls","totalUsage":{"inputTokens":10,"outputTokens":5}}}"#).unwrap();
        fold_bridge_event(&mut acc, &finish).unwrap();
        assert_eq!(acc.text, "hello");
        assert_eq!(acc.tool_calls.len(), 1);
        let obj = acc.to_openai("chatcmpl-1", "x/y", 1).expect("content present");
        assert_eq!(obj["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(obj["usage"]["prompt_tokens"], 10);
        assert_eq!(obj["usage"]["completion_tokens"], 5);
        assert_eq!(obj["choices"][0]["message"]["tool_calls"][0]["id"], "c1");
    }

    #[test]
    fn empty_bridge_yields_honest_502() {
        let acc = CompletionResult { finish_reason: "stop".to_string(), usage: map_usage_to_openai(0, 0), ..Default::default() };
        assert!(acc.is_empty_content());
        let err = acc.to_openai("chatcmpl-1", "x/y", 1).expect_err("empty must fail");
        assert_eq!(err.status, 502);
        assert!(err.retryable);
    }

    #[test]
    fn stream_chunks_mirror_bridge_framing() {
        let mut acc = CompletionResult { finish_reason: "stop".to_string(), usage: map_usage_to_openai(0, 0), ..Default::default() };
        let mut state = SseState::default();
        let text = parse_cc_line(r#"data: {"type":"text-delta","text":"hi"}"#).unwrap();
        let chunks = translate_stream_event("chatcmpl-1", "m", 7, &text, &mut state, &mut acc).unwrap();
        assert_eq!(chunks.len(), 1);
        let parsed: serde_json::Value = serde_json::from_str(chunks[0].as_str()).unwrap();
        assert_eq!(parsed["choices"][0]["delta"]["content"], "hi");
        assert_eq!(parsed["choices"][0]["finish_reason"], serde_json::Value::Null);
        let err = parse_cc_line(r#"{"type":"error","data":{"error":"boom"}}"#).unwrap();
        let failed = translate_stream_event("chatcmpl-1", "m", 7, &err, &mut state, &mut acc);
        assert!(failed.is_err());
        assert!(failed.unwrap_err().retryable);
    }

    #[test]
    fn inline_success_false_is_upstream_error() {
        let event = CcEvent { event_type: "update".to_string(), data: serde_json::json!({"success": false, "error": {"message": "nope"}}) };
        let err = bridge_inline_error(&event).expect("must surface");
        assert_eq!(err.message, "nope");
        let ok_event = CcEvent { event_type: "update".to_string(), data: serde_json::json!({"success": true}) };
        assert!(bridge_inline_error(&ok_event).is_none());
    }
}
