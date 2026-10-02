pub mod antigravity;
pub mod chatgpt;
pub mod claude;
pub mod commandcode;
pub mod opencode;

pub const ALPHA_GENERATE_PRIVATE_NOTE: &str =
    "Command Code CLI-style /alpha/generate is a private, subscription-specific route. Isolated and local-token-gated; verify terms before release.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    CommandCodeCliLogin,
    OpenCodeSubscriptionKey,
    ChatGptOAuth,
    AntigravitySubscription,
    ClaudeSubscription,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenPresence {
    NoToken,
    TokenPresent,
}

pub trait ProviderAdapter {
    fn slug(&self) -> &'static str;
    fn auth_kind(&self) -> AuthKind;
    fn local_token_presence(&self) -> TokenPresence;
    fn known_models(&self) -> Vec<String> {
        Vec::new()
    }
    fn not_configured_message(&self) -> String {
        format!(
            "{} adapter is not configured with a local token. {}",
            self.slug(),
            "Paste a local token to exercise translation paths. Automated auth is deferred."
        )
    }
}

/// Shared upstream error for OpenAI-passthrough + translated transports.
///
/// Mirrors the bridge's retry classification: 429 / 5xx / timeouts before any
/// content are safe to roll to the next account; auth and validation errors
/// are terminal for this attempt.
#[derive(Debug, Clone)]
pub struct AdapterError {
    pub message: String,
    pub status: u16,
    pub retryable: bool,
    /// Upstream `Retry-After` seconds, when the error carried one. The
    /// gateway forwards it on terminal 429s; callers roll regardless.
    pub retry_after: Option<u64>,
}

impl AdapterError {
    pub fn terminal(message: impl Into<String>, status: u16) -> Self {
        AdapterError { message: message.into(), status, retryable: false, retry_after: None }
    }
    pub fn retryable(message: impl Into<String>, status: u16) -> Self {
        AdapterError { message: message.into(), status, retryable: true, retry_after: None }
    }
    pub fn with_retry_after(mut self, secs: Option<u64>) -> Self {
        if self.retry_after.is_none() {
            self.retry_after = secs;
        }
        self
    }
}

/// Parse `Retry-After` (delta-seconds; HTTP dates are treated as absent —
/// sub-minute precision is all the gateway promises) from response headers.
pub fn retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|secs| *secs <= 3600)
}

/// Retryable iff upstream rate-limited us or failed server-side.
pub fn is_retryable_status(status: u16) -> bool {
    status == 429 || status >= 500
}

/// Credential failures are always terminal (never roll on auth errors).
pub fn is_auth_status(status: u16) -> bool {
    status == 401 || status == 403
}

/// True when a 403 body carries a plan/entitlement shape rather than a
/// credential rejection: valid key, wrong plan. Such 403s are rollable
/// (`Next`) so a legacy plan ahead of a valid account doesn't abort the
/// chain; credential 401/403 stays terminal (`Stop`).
/// Markers: `EntitlementError` (Zen usage gate), "without a subscription"
/// family, and Go-plan text. Case-insensitive substring match on the
/// truncated upstream snippet (already truncated by callers).
pub fn is_plan_403(snippet: &str) -> bool {
    let lower = snippet.to_lowercase();
    if lower.contains("entitlement") {
        return true;
    }
    if lower.contains("go plan") || lower.contains("go-plan") {
        return true;
    }
    if lower.contains("subscription")
        && (lower.contains("without")
            || lower.contains("no ")
            || lower.contains("missing")
            || lower.contains("required")
            || lower.contains("entitlement"))
    {
        return true;
    }
    false
}

/// Classify a 403 upstream snippet: plan-shaped 403s are retryable (roll to
/// the next account), credential 403s are terminal. Non-403 statuses are
/// left to the caller's `is_auth_status` / `is_retryable_status` rules.
pub fn classify_403(snippet: &str) -> AdapterError {
    if is_plan_403(snippet) {
        AdapterError::retryable(format!("plan 403 (rollable): {snippet}"), 403)
    } else {
        AdapterError::terminal(format!("credential 403: {snippet}"), 403)
    }
}

pub fn truncate_snippet(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Long-lived client for serving sends (300s for slow reasoning streams).
pub fn serving_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Strip an SSE `data:` prefix; `None` for non-data lines. `[DONE]` passes
/// through as-is. Shared by all OpenAI-SSE passthrough + translated streams.
pub fn sse_payload(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("data:") {
        Some(rest.trim())
    } else {
        None
    }
}

/// Rewrite the top-level `model` field to the gateway echo id (unified
/// `slug/native` or native provider-path id). OpenAI-shaped bodies only
/// (those carrying `choices`): Messages/Responses native shapes have no
/// top-level `model` echo contract and pass through untouched.
pub fn rewrite_model(body: &mut serde_json::Value, echo_model: &str) {
    let openai_shaped = body
        .as_object()
        .map(|obj| obj.contains_key("choices"))
        .unwrap_or(false);
    if !openai_shaped {
        return;
    }
    if let Some(obj) = body.as_object_mut() {
        obj.insert("model".to_string(), serde_json::Value::String(echo_model.to_string()));
    }
}

/// Extract `(prompt, completion)` counts from an OpenAI-style `usage` object.
/// Missing shapes map to zero (bridge convention).
pub fn usage_counts(usage: Option<&serde_json::Value>) -> (u64, u64) {
    let (prompt, completion, _, _) = usage_details(usage);
    (prompt, completion)
}

/// Extract `(prompt, completion, cached_read, cache_creation)` from any known
/// upstream usage shape. `prompt` stays TOTAL input (OpenAI convention — it
/// already includes the cache legs); the legs are subsets of it.
///
/// Read order per leg (first hit wins):
/// - prompt: `prompt_tokens` | `input_tokens`
/// - completion: `completion_tokens` | `output_tokens`
/// - cached_read: `cache_read_input_tokens` (Anthropic/CommandCode) |
///   `prompt_tokens_details.cached_tokens` (OpenAI) |
///   `input_tokens_details.cached_tokens` (Codex Responses)
/// - cache_creation: `cache_creation_input_tokens` (Anthropic/CommandCode) |
///   `prompt_tokens_details.cache_write_tokens` (CommandCode)
///
/// Missing shapes map to zero (bridge convention). Defensive clamp: legs that
/// exceed `prompt` are zeroed (garbage in must never produce negative
/// uncached input downstream).
pub fn usage_details(usage: Option<&serde_json::Value>) -> (u64, u64, u64, u64) {
    let usage = match usage {
        Some(u) => u,
        None => return (0, 0, 0, 0),
    };
    let prompt = usage
        .get("prompt_tokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion = usage
        .get("completion_tokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let details = |key: &str| usage.get(key).and_then(|v| v.as_object());
    let cached_read = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            details("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_u64())
        })
        .or_else(|| {
            details("input_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|v| v.as_u64())
        })
        .unwrap_or(0);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_u64())
        .or_else(|| {
            details("prompt_tokens_details")
                .and_then(|d| d.get("cache_write_tokens"))
                .and_then(|v| v.as_u64())
        })
        .unwrap_or(0);
    if cached_read.saturating_add(cache_creation) > prompt {
        return (prompt, completion, 0, 0);
    }
    (prompt, completion, cached_read, cache_creation)
}

pub struct CommandCodeAdapter;
pub struct OpenCodeAdapter;
pub struct ChatGptAdapter;
pub struct AntigravityAdapter;
pub struct ClaudeAdapter;

impl ProviderAdapter for CommandCodeAdapter {
    fn slug(&self) -> &'static str {
        "commandcode"
    }
    fn auth_kind(&self) -> AuthKind {
        AuthKind::CommandCodeCliLogin
    }
    fn local_token_presence(&self) -> TokenPresence {
        TokenPresence::NoToken
    }
}

impl ProviderAdapter for OpenCodeAdapter {
    fn slug(&self) -> &'static str {
        "opencode"
    }
    fn auth_kind(&self) -> AuthKind {
        AuthKind::OpenCodeSubscriptionKey
    }
    fn local_token_presence(&self) -> TokenPresence {
        TokenPresence::NoToken
    }
}

impl ProviderAdapter for ChatGptAdapter {
    fn slug(&self) -> &'static str {
        "chatgpt"
    }
    fn auth_kind(&self) -> AuthKind {
        AuthKind::ChatGptOAuth
    }
    fn local_token_presence(&self) -> TokenPresence {
        TokenPresence::NoToken
    }
}

impl ProviderAdapter for AntigravityAdapter {
    fn slug(&self) -> &'static str {
        "antigravity"
    }
    fn auth_kind(&self) -> AuthKind {
        AuthKind::AntigravitySubscription
    }
    fn local_token_presence(&self) -> TokenPresence {
        TokenPresence::NoToken
    }
}

impl ProviderAdapter for ClaudeAdapter {
    fn slug(&self) -> &'static str {
        "claude"
    }
    fn auth_kind(&self) -> AuthKind {
        AuthKind::ClaudeSubscription
    }
    fn local_token_presence(&self) -> TokenPresence {
        TokenPresence::NoToken
    }
}

/// Normalize a client-supplied tool input schema to JSON Schema draft
/// 2020-12 for Anthropic-strict validators (direct Claude requests and
/// Claude models served via Antigravity, which proxy to Anthropic
/// validation). Agent clients (MCP-style schemas) routinely emit shapes a
/// strict 2020-12 validator rejects with `input_schema … invalid` 400s —
/// observed live 2026-09-30 (Hermes agent → opus 4.6 via Antigravity).
/// Normalization only LOOSENS or transliterates; it never adds
/// constraints, so an already-valid schema maps to an equivalent one:
/// - missing `type` alongside `properties` → `"type": "object"`
/// - draft-04 boolean `exclusiveMinimum`/`exclusiveMaximum` → numeric form
///   (dropped when no `minimum`/`maximum` accompanies them)
/// - `dependencies` → `dependentRequired` / `dependentSchemas` split
/// - `definitions` merged into `$defs` (+ `#/definitions/…` `$ref` rewrite)
/// - `required` filtered to names present in `properties`, and only when
///   `properties` is an object (untouched otherwise)
/// - non-schema `properties` values pruned; non-object `properties` dropped
/// - draft-04 tuple `items` array → `prefixItems` (+ `additionalItems`
///   pairing); `id` → `$id`; `$schema` normalized to the 2020-12 identifier
/// - OpenAI-ism `strict` removed (not a JSON Schema keyword)
/// Recursion is depth-capped; deeper nodes pass through as `{}` (loosest).
pub fn sanitize_tool_schema(schema: &serde_json::Value) -> serde_json::Value {
    sanitize_schema_depth(schema, 0)
}

const SANITIZE_MAX_DEPTH: u32 = 32;
const DRAFT_2020_12_ID: &str = "https://json-schema.org/draft/2020-12/schema";

fn sanitize_schema_depth(schema: &serde_json::Value, depth: u32) -> serde_json::Value {
    use serde_json::{json, Map, Value};
    if depth > SANITIZE_MAX_DEPTH {
        return json!({});
    }
    let Some(obj) = schema.as_object() else {
        // Boolean (and other) schemas are valid 2020-12 as-is.
        return schema.clone();
    };
    let mut map = obj.clone();
    // OpenAI-isms that are not JSON Schema keywords at all.
    map.remove("strict");
    // The output we emit is 2020-12-compatible, so label it as such when a
    // (possibly stale) dialect identifier is present.
    if map.get("$schema").and_then(|v| v.as_str()).is_some_and(|s| s != DRAFT_2020_12_ID) {
        map.insert("$schema".to_string(), json!(DRAFT_2020_12_ID));
    }
    // Legacy `id` → `$id` (existing `$id` wins).
    if let Some(id) = map.remove("id") {
        if id.is_string() && !map.contains_key("$id") {
            map.insert("$id".to_string(), id);
        }
    }
    // `definitions` → `$defs` merge (existing `$defs` keys win); values are
    // schemas, so they are normalized too.
    if let Some(defs) = map.remove("definitions") {
        if let Some(defs_obj) = defs.as_object() {
            let entry = map.entry("$defs".to_string()).or_insert_with(|| json!({}));
            if let Some(target) = entry.as_object_mut() {
                for (k, v) in defs_obj {
                    target
                        .entry(k.clone())
                        .or_insert_with(|| sanitize_schema_depth(v, depth + 1));
                }
            }
        }
    }
    // Rewrite refs that pointed at the pre-merge location.
    if let Some(rest) = map.get("$ref").and_then(|v| v.as_str()).and_then(|r| r.strip_prefix("#/definitions/")).map(str::to_string) {
        map.insert("$ref".to_string(), json!(format!("#/$defs/{rest}")));
    }
    // Draft-04 boolean bounds → numeric 2020-12 bounds (or dropped).
    for (excl, bound) in [("exclusiveMinimum", "minimum"), ("exclusiveMaximum", "maximum")] {
        match map.get(excl) {
            Some(v) if v.is_number() => {}
            Some(v) if v.as_bool() == Some(true) => {
                if let Some(n) = map.get(bound).and_then(|b| b.as_f64()) {
                    map.remove(bound);
                    map.insert(excl.to_string(), json!(n));
                } else {
                    map.remove(excl);
                }
            }
            Some(_) => {
                map.remove(excl);
            }
            None => {}
        }
    }
    // `dependencies` split by value shape.
    if let Some(deps) = map.remove("dependencies") {
        if let Some(deps_obj) = deps.as_object() {
            for (k, v) in deps_obj {
                let all_strings = v
                    .as_array()
                    .map(|a| a.iter().all(|e| e.is_string()))
                    .unwrap_or(false);
                let key = if all_strings { "dependentRequired" } else { "dependentSchemas" };
                let entry = map.entry(key.to_string()).or_insert_with(|| json!({}));
                if let Some(target) = entry.as_object_mut() {
                    let value = if all_strings { v.clone() } else { sanitize_schema_depth(v, depth + 1) };
                    target.insert(k.clone(), value);
                }
            }
        }
    }
    // `dependentRequired` values must be string arrays; prune the rest.
    if let Some(dr) = map.get("dependentRequired").and_then(|v| v.as_object()).cloned() {
        let clean: Map<String, Value> = dr
            .into_iter()
            .filter(|(_, v)| v.as_array().map(|a| a.iter().all(|e| e.is_string())).unwrap_or(false))
            .collect();
        map.insert("dependentRequired".to_string(), Value::Object(clean));
    }
    // `properties`: prune non-schema values, drop non-object wholesale.
    let mut have_props_object = false;
    if let Some(props) = map.remove("properties") {
        if let Some(props_obj) = props.as_object() {
            let mut clean = Map::new();
            for (k, v) in props_obj {
                if v.is_object() || v.is_boolean() {
                    clean.insert(k.clone(), sanitize_schema_depth(v, depth + 1));
                }
            }
            map.insert("properties".to_string(), Value::Object(clean));
            have_props_object = true;
        }
    }
    // `required` filtered to declared properties — only when properties exist.
    if have_props_object {
        let names: Vec<String> = map
            .get("properties")
            .and_then(|v| v.as_object())
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default();
        match map.get("required") {
            Some(v) if v.is_array() => {
                let filtered: Vec<Value> = v
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|e| e.as_str().map(|s| names.iter().any(|n| n == s)).unwrap_or(false))
                    .collect();
                map.insert("required".to_string(), Value::Array(filtered));
            }
            Some(_) => {
                map.remove("required");
            }
            None => {}
        }
    }
    // Tool schemas are objects: default the type when properties declare it.
    if !map.contains_key("type") && map.contains_key("properties") {
        map.insert("type".to_string(), json!("object"));
    }
    // Draft-04 tuple `items` → `prefixItems` (+ `additionalItems` pairing).
    if let Some(items) = map.remove("items") {
        if items.is_object() || items.is_boolean() {
            map.insert("items".to_string(), sanitize_schema_depth(&items, depth + 1));
        } else if let Some(arr) = items.as_array() {
            let mut prefix: Vec<Value> = map
                .remove("prefixItems")
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            for e in arr {
                if e.is_object() || e.is_boolean() {
                    prefix.push(sanitize_schema_depth(e, depth + 1));
                }
            }
            map.insert("prefixItems".to_string(), Value::Array(prefix));
            if let Some(addl) = map.remove("additionalItems") {
                if addl.is_object() || addl.is_boolean() {
                    map.insert("items".to_string(), sanitize_schema_depth(&addl, depth + 1));
                }
            }
        }
    } else {
        map.remove("additionalItems");
    }
    // Single-subschema keywords, recursed.
    for key in [
        "contains",
        "propertyNames",
        "if",
        "then",
        "else",
        "not",
        "additionalProperties",
        "unevaluatedItems",
        "unevaluatedProperties",
    ] {
        if let Some(sub) = map.get(key).cloned() {
            if sub.is_object() || sub.is_boolean() {
                map.insert(key.to_string(), sanitize_schema_depth(&sub, depth + 1));
            } else {
                map.remove(key);
            }
        }
    }
    // Map-of-subschema keywords, recursed per value.
    for key in ["patternProperties", "dependentSchemas", "$defs"] {
        if let Some(obj) = map.get(key).and_then(|v| v.as_object()).cloned() {
            let clean: Map<String, Value> = obj
                .into_iter()
                .filter_map(|(k, v)| {
                    if v.is_object() || v.is_boolean() {
                        Some((k, sanitize_schema_depth(&v, depth + 1)))
                    } else {
                        None
                    }
                })
                .collect();
            map.insert(key.to_string(), Value::Object(clean));
        } else if map.contains_key(key) {
            map.remove(key);
        }
    }
    // Combinators: element-wise recursion; non-array forms dropped.
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(arr) = map.get(key).and_then(|v| v.as_array()).cloned() {
            let clean: Vec<Value> = arr
                .into_iter()
                .filter_map(|e| {
                    if e.is_object() || e.is_boolean() {
                        Some(sanitize_schema_depth(&e, depth + 1))
                    } else {
                        None
                    }
                })
                .collect();
            map.insert(key.to_string(), Value::Array(clean));
        } else if map.contains_key(key) {
            map.remove(key);
        }
    }
    Value::Object(map)
}

/// Normalize a tool input schema to the OpenAPI-3.0-subset `Schema` proto
/// that Google's Gemini `functionDeclarations.parameters` accepts.
/// Every rule below was probed live against the gateway 2026-09-30
/// (~40 tiny `gemini-3-flash` calls): unknown field names INVALID_ARGUMENT
/// the whole request, so the sanitizer strips to an allowlist and
/// transliterates the rest. Like `sanitize_tool_schema` it only loosens or
/// transliterates, never adds constraints:
/// - kept (recursed): type (single string), format, title, description,
///   nullable, enum (strings only), maxItems/minItems, properties,
///   required (filtered to declared properties), min/maxProperties, items
///   (single), additionalProperties, minimum/maximum, min/maxLength,
///   pattern, default, anyOf/oneOf/allOf/not/prefixItems (recursed),
///   ref/defs, propertyOrdering
/// - transliterated: `$ref`→`ref` (+`#/definitions/`/`#/$defs/`→`#/defs/`),
///   `definitions`/`$defs`→`defs` (merged), union `type` arrays→`nullable`
///   / `anyOf`, `const`→single `enum`, tuple `items` arrays→`prefixItems`
/// - dropped: everything else (`$schema`, `$id`, `exclusiveMinimum`,
///   `dependencies`, `patternProperties`, `if/then/else`, non-string enums…)
pub fn sanitize_gemini_schema(schema: &serde_json::Value) -> serde_json::Value {
    gemini_depth(schema, 0)
}

fn gemini_depth(schema: &serde_json::Value, depth: u32) -> serde_json::Value {
    use serde_json::{json, Map, Value};
    if depth > SANITIZE_MAX_DEPTH {
        return json!({});
    }
    let Some(obj) = schema.as_object() else {
        return schema.clone();
    };
    let mut map = obj.clone();
    // Proto-unknown keywords: each proven live to INVALID_ARGUMENT.
    for key in [
        "strict", "$schema", "$id", "id", "$comment", "dependencies",
        "dependentRequired", "dependentSchemas", "exclusiveMinimum",
        "exclusiveMaximum", "multipleOf", "uniqueItems", "examples",
        "readOnly", "writeOnly", "deprecated", "patternProperties",
        "propertyNames", "contains", "if", "then", "else",
        "unevaluatedItems", "unevaluatedProperties", "minContains",
        "maxContains", "contentMediaType", "contentEncoding", "contentSchema",
    ] {
        map.remove(key);
    }
    // `const` → single `enum` (non-string enums are rejected outright).
    if let Some(c) = map.remove("const") {
        if map.get("enum").is_none() {
            map.insert("enum".to_string(), json!([c]));
        }
    }
    // `enum` must be strings; filter (drop the key when nothing survives a
    // non-empty original — an unsatisfiable enum is worse than none).
    if let Some(arr) = map.get("enum").and_then(|v| v.as_array()).cloned() {
        if arr.iter().all(|e| e.is_string()) {
            // keep verbatim
        } else {
            let strs: Vec<Value> = arr.into_iter().filter(|e| e.is_string()).collect();
            if strs.is_empty() {
                map.remove("enum");
            } else {
                map.insert("enum".to_string(), Value::Array(strs));
            }
        }
    }
    // `$ref` → proto-native `ref` (+ prefix rewrite); unresolvable dropped.
    if let Some(r) = map.remove("$ref") {
        if let Some(s) = r.as_str() {
            let mut rewritten = s.to_string();
            for prefix in ["#/definitions/", "#/$defs/"] {
                if let Some(rest) = s.strip_prefix(prefix) {
                    rewritten = format!("#/defs/{rest}");
                    break;
                }
            }
            map.insert("ref".to_string(), json!(rewritten));
        }
    }
    // `definitions`/`$defs` → proto-native `defs` (existing wins).
    for key in ["definitions", "$defs"] {
        if let Some(defs) = map.remove(key) {
            if let Some(defs_obj) = defs.as_object() {
                let entry = map.entry("defs".to_string()).or_insert_with(|| json!({}));
                if let Some(target) = entry.as_object_mut() {
                    for (k, v) in defs_obj {
                        target.entry(k.clone()).or_insert_with(|| gemini_depth(v, depth + 1));
                    }
                }
            }
        }
    }
    // Union `type` arrays: proto Type is a single enum — `["X","null"]`
    // becomes `nullable`, wider unions become `anyOf` (+ siblings allowed).
    if let Some(arr) = map.get("type").and_then(|v| v.as_array()).cloned() {
        let mut names: Vec<String> = arr.iter().filter_map(|e| e.as_str().map(str::to_string)).collect();
        let nullable = names.iter().any(|n| n == "null");
        names.retain(|n| n != "null");
        map.remove("type");
        if nullable {
            map.insert("nullable".to_string(), json!(true));
        }
        match names.as_slice() {
            [] => {}
            [single] => {
                map.insert("type".to_string(), json!(single));
            }
            _ => {
                let variants: Vec<Value> = names.into_iter().map(|n| json!({"type": n})).collect();
                map.insert("anyOf".to_string(), Value::Array(variants));
            }
        }
    }
    // `required` filtered to declared properties (only when present).
    if map.get("properties").and_then(|v| v.as_object()).is_some() {
        if let Some(arr) = map.get("required").and_then(|v| v.as_array()).cloned() {
            let names: Vec<String> = map
                .get("properties")
                .and_then(|v| v.as_object())
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            let filtered: Vec<Value> = arr
                .into_iter()
                .filter(|e| e.as_str().map(|s| names.iter().any(|n| n == s)).unwrap_or(false))
                .collect();
            map.insert("required".to_string(), Value::Array(filtered));
        } else if map.contains_key("required") {
            map.remove("required");
        }
    }
    if !map.contains_key("type") && map.contains_key("properties") {
        map.insert("type".to_string(), json!("object"));
    }
    // `properties`: prune non-schema values, drop non-object wholesale.
    if let Some(props) = map.remove("properties") {
        if let Some(props_obj) = props.as_object() {
            let mut clean = Map::new();
            for (k, v) in props_obj {
                if v.is_object() || v.is_boolean() {
                    clean.insert(k.clone(), gemini_depth(v, depth + 1));
                }
            }
            map.insert("properties".to_string(), Value::Object(clean));
        }
    }
    // `items`: single schema recursed; tuple arrays → `prefixItems`.
    if let Some(items) = map.remove("items") {
        if items.is_object() || items.is_boolean() {
            map.insert("items".to_string(), gemini_depth(&items, depth + 1));
        } else if let Some(arr) = items.as_array() {
            let mut prefix: Vec<Value> = map
                .remove("prefixItems")
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            for e in arr {
                if e.is_object() || e.is_boolean() {
                    prefix.push(gemini_depth(e, depth + 1));
                }
            }
            map.insert("prefixItems".to_string(), Value::Array(prefix));
        }
    }
    map.remove("additionalItems");
    // Recursed single-subschema keywords (all probed accepted).
    for key in ["additionalProperties", "not"] {
        if let Some(sub) = map.get(key).cloned() {
            if sub.is_object() || sub.is_boolean() {
                map.insert(key.to_string(), gemini_depth(&sub, depth + 1));
            } else {
                map.remove(key);
            }
        }
    }
    // Recursed map-of-subschema keywords.
    for key in ["defs"] {
        if let Some(obj) = map.get(key).and_then(|v| v.as_object()).cloned() {
            let clean: Map<String, Value> = obj
                .into_iter()
                .filter_map(|(k, v)| {
                    if v.is_object() || v.is_boolean() {
                        Some((k, gemini_depth(&v, depth + 1)))
                    } else {
                        None
                    }
                })
                .collect();
            map.insert(key.to_string(), Value::Object(clean));
        } else if map.contains_key(key) {
            map.remove(key);
        }
    }
    // Combinators (all probed accepted, incl. anyOf with siblings).
    for key in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(arr) = map.get(key).and_then(|v| v.as_array()).cloned() {
            let clean: Vec<Value> = arr
                .into_iter()
                .filter_map(|e| {
                    if e.is_object() || e.is_boolean() {
                        Some(gemini_depth(&e, depth + 1))
                    } else {
                        None
                    }
                })
                .collect();
            map.insert(key.to_string(), Value::Array(clean));
        } else if map.contains_key(key) {
            map.remove(key);
        }
    }
    // `propertyOrdering` kept verbatim when an array, else dropped.
    if map.get("propertyOrdering").and_then(|v| v.as_array()).is_none() && map.contains_key("propertyOrdering") {
        map.remove("propertyOrdering");
    }
    Value::Object(map)
}

/// Normalize a tool input schema for Claude models served THROUGH
/// Antigravity's Gemini envelope. Proven live 2026-09-30: Google parses
/// `parameters` as its Schema proto (unknown names → INVALID_ARGUMENT)
/// even when the model routes to Anthropic, which then applies strict
/// draft 2020-12 validation. Output must satisfy BOTH gates:
/// - Google-name-safe like `sanitize_gemini_schema` (no `$defs`/`$ref`/
///   `dependentRequired`/`exclusiveMinimum`/`$schema`…), except references
///   are INLINED from a harvested side table instead of renamed (Anthropic
///   would not understand proto-native `ref`/`defs`)
/// - Anthropic-valid like `sanitize_tool_schema` (`required` filtered,
///   missing `type` defaulted, `oneOf`/`anyOf`/`allOf`/`not` preserved)
/// Only loosens or transliterates, never adds constraints.
pub fn sanitize_claude_via_google(schema: &serde_json::Value) -> serde_json::Value {
    use serde_json::Map;
    let mut defs = Map::new();
    harvest_ref_targets(schema, &mut defs);
    cvj_depth(schema, 0, &defs)
}

fn harvest_ref_targets(schema: &serde_json::Value, table: &mut serde_json::Map<String, serde_json::Value>) {
    if let Some(obj) = schema.as_object() {
        for key in ["definitions", "$defs", "defs"] {
            if let Some(d) = obj.get(key).and_then(|v| v.as_object()) {
                for (k, v) in d {
                    table.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
        }
    }
}

fn cvj_depth(
    schema: &serde_json::Value,
    depth: u32,
    defs: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    use serde_json::{json, Map, Value};
    if depth > SANITIZE_MAX_DEPTH {
        return json!({});
    }
    let Some(obj) = schema.as_object() else {
        return schema.clone();
    };
    // Nested definition blocks join the side table (small schemas: clone).
    let mut local = defs.clone();
    harvest_ref_targets(schema, &mut local);
    let mut map = obj.clone();
    for key in [
        "strict", "$schema", "$id", "id", "$comment", "definitions", "$defs",
        "defs", "dependencies", "dependentRequired", "dependentSchemas",
        "exclusiveMinimum", "exclusiveMaximum", "multipleOf", "uniqueItems",
        "examples", "readOnly", "writeOnly", "deprecated", "patternProperties",
        "propertyNames", "contains", "if", "then", "else", "additionalItems",
        "unevaluatedItems", "unevaluatedProperties", "minContains",
        "maxContains", "contentMediaType", "contentEncoding", "contentSchema",
    ] {
        map.remove(key);
    }
    if let Some(c) = map.remove("const") {
        if map.get("enum").is_none() {
            map.insert("enum".to_string(), json!([c]));
        }
    }
    // References are inlined (neither `$ref` nor proto `ref` survives both
    // gates); node siblings win over inlined keys. Missing targets drop the
    // key and the node continues without it.
    let ref_key = map.remove("$ref").or_else(|| map.remove("ref"));
    if let Some(r) = ref_key.and_then(|v| v.as_str().map(str::to_string)) {
        for prefix in ["#/definitions/", "#/$defs/", "#/defs/"] {
            if let Some(name) = r.strip_prefix(prefix) {
                if let Some(target) = local.get(name) {
                    let t = cvj_depth(&target.clone(), depth + 1, &local);
                    if let Some(tobj) = t.as_object() {
                        for (k, v) in tobj {
                            map.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
                }
                break;
            }
        }
    }
    // Union `type` arrays (proto Type is single-valued; Anthropic accepts
    // the transliteration).
    if let Some(arr) = map.get("type").and_then(|v| v.as_array()).cloned() {
        let mut names: Vec<String> = arr.iter().filter_map(|e| e.as_str().map(str::to_string)).collect();
        let nullable = names.iter().any(|n| n == "null");
        names.retain(|n| n != "null");
        map.remove("type");
        if nullable {
            map.insert("nullable".to_string(), json!(true));
        }
        match names.as_slice() {
            [] => {}
            [single] => {
                map.insert("type".to_string(), json!(single));
            }
            _ => {
                let variants: Vec<Value> = names.into_iter().map(|n| json!({"type": n})).collect();
                map.insert("anyOf".to_string(), Value::Array(variants));
            }
        }
    }
    if map.get("properties").and_then(|v| v.as_object()).is_some() {
        if let Some(arr) = map.get("required").and_then(|v| v.as_array()).cloned() {
            let names: Vec<String> = map
                .get("properties")
                .and_then(|v| v.as_object())
                .map(|o| o.keys().cloned().collect())
                .unwrap_or_default();
            let filtered: Vec<Value> = arr
                .into_iter()
                .filter(|e| e.as_str().map(|s| names.iter().any(|n| n == s)).unwrap_or(false))
                .collect();
            map.insert("required".to_string(), Value::Array(filtered));
        } else if map.contains_key("required") {
            map.remove("required");
        }
    }
    if !map.contains_key("type") && map.contains_key("properties") {
        map.insert("type".to_string(), json!("object"));
    }
    if let Some(props) = map.remove("properties") {
        if let Some(props_obj) = props.as_object() {
            let mut clean = Map::new();
            for (k, v) in props_obj {
                if v.is_object() || v.is_boolean() {
                    clean.insert(k.clone(), cvj_depth(v, depth + 1, &local));
                }
            }
            map.insert("properties".to_string(), Value::Object(clean));
        }
    }
    if let Some(items) = map.remove("items") {
        if items.is_object() || items.is_boolean() {
            map.insert("items".to_string(), cvj_depth(&items, depth + 1, &local));
        } else if let Some(arr) = items.as_array() {
            let clean: Vec<Value> = arr
                .iter()
                .filter_map(|e| {
                    if e.is_object() || e.is_boolean() {
                        Some(cvj_depth(e, depth + 1, &local))
                    } else {
                        None
                    }
                })
                .collect();
            if clean.is_empty() {
                // Tuple with no valid elements: drop (an empty anyOf is
                // itself invalid 2020-12).
            } else {
                map.insert("anyOf".to_string(), Value::Array(clean));
            }
        }
    }
    for key in ["additionalProperties", "not"] {
        if let Some(sub) = map.get(key).cloned() {
            if sub.is_object() || sub.is_boolean() {
                map.insert(key.to_string(), cvj_depth(&sub, depth + 1, &local));
            } else {
                map.remove(key);
            }
        }
    }
    for key in ["oneOf", "anyOf", "allOf", "prefixItems"] {
        if let Some(arr) = map.get(key).and_then(|v| v.as_array()).cloned() {
            let clean: Vec<Value> = arr
                .into_iter()
                .filter_map(|e| {
                    if e.is_object() || e.is_boolean() {
                        Some(cvj_depth(&e, depth + 1, &local))
                    } else {
                        None
                    }
                })
                .collect();
            map.insert(key.to_string(), Value::Array(clean));
        } else if map.contains_key(key) {
            map.remove(key);
        }
    }
    if map.get("propertyOrdering").and_then(|v| v.as_array()).is_none() && map.contains_key("propertyOrdering") {
        map.remove("propertyOrdering");
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_tool_schema_normalizes_draft_violations() {
        // Missing top-level type with properties → object.
        let out = sanitize_tool_schema(&serde_json::json!({"properties": {"p": {"type": "string"}}}));
        assert_eq!(out["type"], "object");
        // Draft-04 boolean bounds → numeric form.
        let out = sanitize_tool_schema(&serde_json::json!({"type": "number", "minimum": 5.0, "exclusiveMinimum": true}));
        assert_eq!(out["exclusiveMinimum"], 5.0);
        assert!(out.get("minimum").is_none());
        let out = sanitize_tool_schema(&serde_json::json!({"type": "number", "exclusiveMaximum": false}));
        assert!(out.get("exclusiveMaximum").is_none());
        // `dependencies` split by value shape.
        let out = sanitize_tool_schema(&serde_json::json!({"dependencies": {"a": ["b"], "c": {"properties": {}}}}));
        assert_eq!(out["dependentRequired"]["a"], serde_json::json!(["b"]));
        assert!(out["dependentSchemas"].get("c").is_some());
        assert!(out.get("dependencies").is_none());
        // `definitions` merge + $ref rewrite.
        let out = sanitize_tool_schema(&serde_json::json!({
            "definitions": {"Name": {"type": "string"}},
            "properties": {"n": {"$ref": "#/definitions/Name"}},
        }));
        assert_eq!(out["$defs"]["Name"]["type"], "string");
        assert_eq!(out["properties"]["n"]["$ref"], "#/$defs/Name");
        assert!(out.get("definitions").is_none());
        // `required` filtered to declared properties only when present…
        let out = sanitize_tool_schema(&serde_json::json!({
            "type": "object",
            "properties": {"p": {"type": "string"}},
            "required": ["p", "ghost"],
        }));
        assert_eq!(out["required"], serde_json::json!(["p"]));
        // …untouched when properties are absent (existing passthrough).
        let out = sanitize_tool_schema(&serde_json::json!({"type": "object", "required": ["p"]}));
        assert_eq!(out["required"], serde_json::json!(["p"]));
        // OpenAI-isms and stale dialect markers normalized.
        let out = sanitize_tool_schema(&serde_json::json!({
            "type": "object", "strict": true, "id": "x",
            "$schema": "http://json-schema.org/draft-07/schema#",
        }));
        assert!(out.get("strict").is_none());
        assert_eq!(out["$id"], "x");
        assert_eq!(out["$schema"], "https://json-schema.org/draft/2020-12/schema");
        // Draft-04 tuple items → prefixItems.
        let out = sanitize_tool_schema(&serde_json::json!({"items": [{"type": "string"}, true]}));
        assert_eq!(out["prefixItems"][0]["type"], "string");
        assert_eq!(out["prefixItems"][1], true);
        assert!(out.get("items").is_none());
        // Non-schema property values pruned; valid schemas map to themselves.
        let out = sanitize_tool_schema(&serde_json::json!({
            "type": "object", "properties": {"ok": {"type": "string"}, "bad": "string"},
        }));
        assert!(out["properties"].get("ok").is_some());
        assert!(out["properties"].get("bad").is_none());
        let valid = serde_json::json!({
            "type": "object",
            "properties": {"p": {"type": "string", "minLength": 1}},
            "required": ["p"], "additionalProperties": false,
        });
        assert_eq!(sanitize_tool_schema(&valid), valid);
        // Boolean schemas pass through; deep nesting terminates.
        assert_eq!(sanitize_tool_schema(&serde_json::json!(true)), serde_json::json!(true));
        let mut deep = serde_json::json!({"type": "string"});
        for _ in 0..60 {
            deep = serde_json::json!({"properties": {"x": deep}});
        }
        let _ = sanitize_tool_schema(&deep);
    }

    /// Recursive key walker for asserting sanitizer output carries no
    /// validator-rejected field names anywhere in the tree.
    fn has_key(v: &serde_json::Value, key: &str) -> bool {
        match v {
            serde_json::Value::Object(m) => {
                m.contains_key(key) || m.values().any(|e| has_key(e, key))
            }
            serde_json::Value::Array(a) => a.iter().any(|e| has_key(e, key)),
            _ => false,
        }
    }

    #[test]
    fn sanitize_gemini_schema_strips_to_proto_subset() {
        // The exact hostile shape from the live 2026-09-30 probe.
        let hostile = serde_json::json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "properties": {
                "expr": {"type": "string", "minimum": 1, "exclusiveMinimum": true},
                "prec": {"$ref": "#/definitions/Prec"},
            },
            "required": ["expr", "ghost"],
            "dependencies": {"expr": ["prec"]},
            "definitions": {"Prec": {"type": "integer"}},
            "id": "calc-schema",
            "strict": true,
        });
        let out = sanitize_gemini_schema(&hostile);
        for banned in ["$schema", "$id", "id", "strict", "dependencies",
            "dependentRequired", "dependentSchemas", "exclusiveMinimum",
            "definitions", "$defs", "$ref", "ghost"] {
            assert!(!has_key(&out, banned), "leaked {banned}: {out}");
        }
        // Faithful transliterations survived.
        assert_eq!(out["properties"]["prec"], serde_json::json!({"ref": "#/defs/Prec"}));
        assert_eq!(out["defs"]["Prec"]["type"], "integer");
        assert_eq!(out["properties"]["expr"]["minimum"], 1);
        assert_eq!(out["required"], serde_json::json!(["expr"]));
        assert_eq!(out["type"], "object");
        // Every name Google's proto rejected is gone from a kitchen-sink schema.
        let kitchen = serde_json::json!({
            "type": ["string", "null"],
            "const": "a",
            "examples": ["a"], "readOnly": true, "multipleOf": 2,
            "uniqueItems": true, "patternProperties": {"^x": {}},
            "propertyNames": {}, "contains": {}, "if": {}, "then": {},
            "else": {}, "additionalItems": false, "$comment": "c",
            "unevaluatedProperties": {}, "contentSchema": {},
            "items": [{"type": "string"}],
            "oneOf": [{"type": "string"}], "not": {"type": "integer"},
            "anyOf": [{"type": "string"}], "allOf": [{"type": "string"}],
        });
        let clean = sanitize_gemini_schema(&kitchen);
        for banned in ["const", "examples", "readOnly", "multipleOf",
            "uniqueItems", "patternProperties", "propertyNames", "contains",
            "if", "then", "else", "additionalItems", "$comment",
            "unevaluatedProperties", "contentSchema"] {
            assert!(!has_key(&clean, banned), "leaked {banned}: {clean}");
        }
        assert_eq!(clean["enum"], serde_json::json!(["a"]));
        assert_eq!(clean["nullable"], true);
        assert!(clean["prefixItems"][0].get("type").is_some());
        for kept in ["oneOf", "anyOf", "allOf", "not"] {
            assert!(clean.get(kept).is_some(), "lost {kept}: {clean}");
        }
        // Non-string enums filter to strings; unsatisfiable enums drop.
        let e = sanitize_gemini_schema(&serde_json::json!({"enum": ["a", 1]}));
        assert_eq!(e["enum"], serde_json::json!(["a"]));
        let e = sanitize_gemini_schema(&serde_json::json!({"enum": [1]}));
        assert!(e.get("enum").is_none());
    }

    #[test]
    fn sanitize_claude_via_google_inlines_refs_and_stays_valid() {
        let schema = serde_json::json!({
            "properties": {
                "x": {"$ref": "#/$defs/X"},
                "y": {"type": "string", "exclusiveMinimum": 1},
            },
            "required": ["x", "ghost"],
            "$defs": {"X": {"type": "string", "minimum": 2}},
            "dependencies": {"x": ["y"]},
            "$schema": "http://json-schema.org/draft-07/schema#",
        });
        let out = sanitize_claude_via_google(&schema);
        // Google gate: no proto-unknown names anywhere…
        for banned in ["$defs", "$ref", "definitions", "dependentRequired",
            "dependentSchemas", "dependencies", "exclusiveMinimum", "$schema"] {
            assert!(!has_key(&out, banned), "leaked {banned}: {out}");
        }
        // …Anthropic gate: the reference is inlined, still valid 2020-12.
        assert_eq!(out["properties"]["x"]["type"], "string");
        assert_eq!(out["properties"]["x"]["minimum"], 2);
        assert_eq!(out["properties"]["y"], serde_json::json!({"type": "string"}));
        assert_eq!(out["required"], serde_json::json!(["x"]));
        assert_eq!(out["type"], "object");
        // Missing targets degrade to the node without $ref (never a 400).
        let missing = sanitize_claude_via_google(&serde_json::json!({
            "properties": {"x": {"$ref": "#/$defs/Nope", "description": "d"}},
        }));
        assert_eq!(missing["properties"]["x"], serde_json::json!({"description": "d"}));
        // Combinators survive for Anthropic.
        let combo = sanitize_claude_via_google(&serde_json::json!({
            "anyOf": [{"type": "string"}], "description": "pick one",
        }));
        assert!(combo.get("anyOf").is_some());
    }

    #[test]
    fn slugs_are_stable_and_distinct() {        assert_eq!(CommandCodeAdapter.slug(), "commandcode");
        assert_eq!(OpenCodeAdapter.slug(), "opencode");
        assert_eq!(ChatGptAdapter.slug(), "chatgpt");
        assert_eq!(AntigravityAdapter.slug(), "antigravity");
        assert_eq!(ClaudeAdapter.slug(), "claude");
    }

    #[test]
    fn usage_details_reads_every_known_shape() {
        // OpenAI chat shape with prompt details.
        assert_eq!(
            usage_details(Some(&serde_json::json!({
                "prompt_tokens": 100, "completion_tokens": 20,
                "prompt_tokens_details": {"cached_tokens": 60},
            }))),
            (100, 20, 60, 0)
        );
        // Codex Responses shape.
        assert_eq!(
            usage_details(Some(&serde_json::json!({
                "input_tokens": 100, "output_tokens": 20,
                "input_tokens_details": {"cached_tokens": 30},
            }))),
            (100, 20, 30, 0)
        );
        // Anthropic / Command Code top-level legs (creation included).
        assert_eq!(
            usage_details(Some(&serde_json::json!({
                "prompt_tokens": 100, "completion_tokens": 20,
                "cache_read_input_tokens": 50, "cache_creation_input_tokens": 10,
            }))),
            (100, 20, 50, 10)
        );
        // Missing shapes map to zero.
        assert_eq!(usage_details(None), (0, 0, 0, 0));
        assert_eq!(usage_details(Some(&serde_json::json!({}))), (0, 0, 0, 0));
        // Legs that exceed prompt are garbage: zero them, keep totals.
        assert_eq!(
            usage_details(Some(&serde_json::json!({
                "prompt_tokens": 10, "completion_tokens": 5,
                "cache_read_input_tokens": 50, "cache_creation_input_tokens": 10,
            }))),
            (10, 5, 0, 0)
        );
    }

    #[test]
    fn plan_403_rolls_while_credential_403_stops() {
        // EntitlementError: valid key without a Go subscription -> rollable.
        assert!(is_plan_403("opencode API 403: {\"error\":\"EntitlementError: no subscription\"}"));
        assert!(is_plan_403("without a Go subscription"));
        assert!(is_plan_403("Go plan required for this model"));
        // Credential rejections stay terminal.
        assert!(!is_plan_403("opencode API 403: forbidden"));
        assert!(!is_plan_403("invalid api key"));
        assert!(!is_plan_403(""));
        let roll = classify_403("EntitlementError");
        assert!(roll.retryable);
        assert_eq!(roll.status, 403);
        let stop = classify_403("bad credentials");
        assert!(!stop.retryable);
        assert_eq!(stop.status, 403);
    }

    #[test]
    fn rewrite_model_only_touches_openai_shapes() {
        // OpenAI chat completion: rewritten (byte-level: only the model value changes).
        let mut openai = serde_json::json!({"model": "glm-5.1", "choices": [{"index": 0}]});
        let before = serde_json::to_vec(&openai).expect("bytes");
        rewrite_model(&mut openai, "opencode/glm-5.1");
        let after = serde_json::to_vec(&openai).expect("bytes");
        assert_eq!(openai["model"], "opencode/glm-5.1");
        assert_eq!(openai["choices"][0]["index"], 0);
        assert_ne!(before, after);
        // Streaming chunk shape (choices, no usage): rewritten too.
        let mut chunk = serde_json::json!({"model": "m", "choices": [{"delta": {}}]});
        rewrite_model(&mut chunk, "opencode/m2");
        assert_eq!(chunk["model"], "opencode/m2");
        // Messages native shape: untouched.
        let mut messages = serde_json::json!({"model": "claude-x", "content": [{"type": "text"}]});
        let messages_before = messages.clone();
        rewrite_model(&mut messages, "claude/y");
        assert_eq!(messages, messages_before);
        // Responses native shape: untouched.
        let mut responses = serde_json::json!({"model": "gpt-x", "output": []});
        let responses_before = responses.clone();
        rewrite_model(&mut responses, "chatgpt/y");
        assert_eq!(responses, responses_before);
        // Model-less usage-only chunk: untouched.
        let mut usage = serde_json::json!({"usage": {"prompt_tokens": 1}});
        let usage_before = usage.clone();
        rewrite_model(&mut usage, "opencode/m");
        assert_eq!(usage, usage_before);
    }

    #[test]
    fn adapters_start_without_tokens() {
        assert_eq!(CommandCodeAdapter.local_token_presence(), TokenPresence::NoToken);
        assert_eq!(OpenCodeAdapter.local_token_presence(), TokenPresence::NoToken);
        assert_eq!(ChatGptAdapter.local_token_presence(), TokenPresence::NoToken);
        assert_eq!(AntigravityAdapter.local_token_presence(), TokenPresence::NoToken);
        assert_eq!(ClaudeAdapter.local_token_presence(), TokenPresence::NoToken);
    }
}
