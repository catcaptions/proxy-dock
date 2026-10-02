//! Live quota/limit fetching per provider.
//!
//! Sources (all researched, none invented):
//! - ChatGPT/Codex: `GET https://chatgpt.com/backend-api/wham/usage`
//!   (Bearer + `ChatGPT-Account-Id`). The undocumented schema drifts, so the
//!   parser tries documented aliases and classifies windows by
//!   `limit_window_seconds` (>= 172800 = weekly, else 5-hour).
//! - Antigravity: Google cloudcode-pa `loadCodeAssist` (tier) +
//!   `fetchAvailableModels` (per-model remainingFraction/resetTime) +
//!   `retrieveUserQuotaSummary` (weekly/5h buckets), Bearer token, daily ->
//!   sandbox -> prod fallback.
//! - Command Code: `/alpha/whoami?limits=1`, `/alpha/billing/credits`,
//!   `/alpha/billing/subscriptions`, `/alpha/usage/summary` (Bearer `user_`
//!   key), mirroring the official CLI's fetchUsageData mapping.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaRow {
    pub id: String,
    pub label: String,
    pub remaining_pct: Option<f64>,
    pub reset_text: String,
    pub reset_in_text: String,
    pub urgent: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaResult {
    pub plan: Option<String>,
    pub quota: Vec<QuotaRow>,
    pub refreshed: bool,
    pub model_count: Option<usize>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// "MM/DD, HH:mm" in local time, like the reference blocks.
fn fmt_reset(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%m/%d, %H:%M").to_string())
        .unwrap_or_default()
}

fn fmt_in(ms: i64, now: i64) -> String {
    let mins = ms.saturating_sub(now).max(0) / 60000;
    if mins < 1 {
        return "in under a minute".to_string();
    }
    if mins < 60 {
        return format!("in {mins} minute{}", if mins == 1 { "" } else { "s" });
    }
    let hours = mins / 60;
    if hours < 48 {
        return format!("in {hours} hour{}", if hours == 1 { "" } else { "s" });
    }
    let days = hours / 24;
    format!("in {days} day{}", if days == 1 { "" } else { "s" })
}

/// Defensive reset-time parse: epoch ms, epoch seconds, or RFC-3339 string.
fn parse_reset(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(n) => n.as_i64().map(|t| {
            if t > 1_000_000_000_000 {
                t
            } else if t > 1_000_000_000 {
                t * 1000
            } else {
                t
            }
        }),
        serde_json::Value::String(s) => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.timestamp_millis()),
        _ => None,
    }
}

fn clamp_pct(value: f64) -> f64 {
    value.max(0.0).min(100.0)
}

/// True when an error string signals bad credentials (401/403 in any shape).
fn is_auth_failure(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("401") || lower.contains("403") || lower.contains("forbidden") || lower.contains("unauthorized")
}

fn row(id: String, label: String, remaining_pct: Option<f64>, reset_ms: Option<i64>, now: i64) -> QuotaRow {
    let (reset_text, reset_in_text, urgent) = match reset_ms {
        Some(ms) => (fmt_reset(ms), fmt_in(ms, now), ms.saturating_sub(now) < 60 * 60 * 1000),
        None => (String::new(), String::new(), false),
    };
    QuotaRow { id, label, remaining_pct: remaining_pct.map(clamp_pct), reset_text, reset_in_text, urgent }
}

// ---------------------------------------------------------------------------
// ChatGPT / Codex (wham/usage)
// ---------------------------------------------------------------------------

const WHAM_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

fn wham_first_object<'a>(root: &'a serde_json::Value, keys: &[&str]) -> Option<&'a serde_json::Value> {
    keys.iter().filter_map(|k| root.get(*k)).find(|v| v.is_object())
}

fn wham_used(window: &serde_json::Value) -> Option<f64> {
    window
        .get("percent_left")
        .and_then(serde_json::Value::as_f64)
        .map(|p| 100.0 - p)
        .or_else(|| window.get("used_percent").and_then(serde_json::Value::as_f64))
        .or_else(|| window.get("usedPercent").and_then(serde_json::Value::as_f64))
}

fn wham_reset_ms(window: &serde_json::Value, now: i64) -> Option<i64> {
    if let Some(ms) = window.get("reset_time_ms").and_then(serde_json::Value::as_i64) {
        return Some(ms);
    }
    for key in ["reset_at", "resetsAt"] {
        if let Some(v) = window.get(key).and_then(parse_reset) {
            return Some(v);
        }
    }
    window
        .get("reset_after_seconds")
        .and_then(serde_json::Value::as_i64)
        .map(|s| now + s * 1000)
}

fn wham_window_seconds(window: &serde_json::Value) -> Option<i64> {
    window.get("limit_window_seconds").and_then(serde_json::Value::as_i64)
}

/// Classify by duration (>= 2 days = weekly), falling back to the slot name.
fn wham_slot_name(seconds: Option<i64>, fallback: &str) -> String {
    match seconds {
        Some(s) if s >= 172800 => "Weekly".to_string(),
        Some(_) => "5-hour".to_string(),
        None => fallback.to_string(),
    }
}

fn wham_plan_label(plan_type: &str) -> String {
    match plan_type.trim().to_lowercase().as_str() {
        "free" => "Free".to_string(),
        "go" => "Go".to_string(),
        "plus" => "Plus".to_string(),
        "pro" => "Pro".to_string(),
        "team" => "Team".to_string(),
        "enterprise" => "Enterprise".to_string(),
        "edu" => "Edu".to_string(),
        other if other.is_empty() => String::new(),
        other => other.to_string(),
    }
}

fn wham_window_rows(
    prefix: &str,
    title: Option<&str>,
    primary: Option<&serde_json::Value>,
    secondary: Option<&serde_json::Value>,
    now: i64,
    out: &mut Vec<QuotaRow>,
) {
    let mut used_labels: Vec<String> = Vec::new();
    for (slot, window, fallback) in [("primary", primary, "5-hour"), ("secondary", secondary, "Weekly")] {
        let Some(window) = window else { continue };
        let Some(used) = wham_used(window) else { continue };
        let mut label = wham_slot_name(wham_window_seconds(window), fallback).to_string();
        if used_labels.iter().any(|l| l == &label) {
            label = format!("{label} · {slot}");
        }
        used_labels.push(label.clone());
        let full_label = match title {
            Some(t) => format!("{t} · {label}"),
            None => label,
        };
        out.push(row(
            format!("{prefix}-{slot}"),
            full_label,
            Some(100.0 - used),
            wham_reset_ms(window, now),
            now,
        ));
    }
}

async fn wham_get(access_token: &str, account_id: &str) -> Result<serde_json::Value, String> {
    let mut req = client()
        .get(WHAM_USAGE_URL)
        .bearer_auth(access_token.trim())
        .header("Accept", "application/json")
        .header("Origin", "https://chatgpt.com")
        .header("Referer", "https://chatgpt.com/");
    if !account_id.trim().is_empty() {
        req = req.header("ChatGPT-Account-Id", account_id.trim());
    }
    let resp = req.send().await.map_err(|err| format!("wham usage check failed: {err}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("unauthorized".to_string());
    }
    if !resp.status().is_success() {
        return Err(format!("wham usage check failed ({})", resp.status()));
    }
    resp.json().await.map_err(|err| format!("wham parse failed: {err}"))
}

fn wham_map(body: &serde_json::Value, now: i64) -> Result<(Option<String>, Vec<QuotaRow>), String> {
    let root = wham_first_object(body, &["rate_limit", "rate_limits"])
        .ok_or_else(|| "no rate-limit data in wham response".to_string())?;
    let mut rows = Vec::new();
    let primary = wham_first_object(root, &["five_hour", "primary_window", "primary"]);
    let secondary = wham_first_object(root, &["weekly", "secondary_window", "secondary"]);
    wham_window_rows("window", None, primary, secondary, now, &mut rows);
    if let Some(items) = root.get("additional_rate_limits").and_then(|v| v.as_array()) {
        for item in items {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("extra");
            let title = item.get("title").and_then(|v| v.as_str()).unwrap_or(id);
            let p = wham_first_object(item, &["primary_window", "primary"]);
            let s = wham_first_object(item, &["secondary_window", "secondary"]);
            if p.is_some() || s.is_some() {
                wham_window_rows(&format!("extra-{id}"), Some(title), p, s, now, &mut rows);
            }
        }
    }
    if let Some(review) = root.get("code_review_rate_limit").and_then(|v| v.as_object()) {
        let owned = serde_json::Value::Object(review.clone());
        let p = wham_first_object(&owned, &["primary_window", "primary"]);
        if let Some(window) = p {
            if let Some(used) = wham_used(window) {
                rows.push(row(
                    "code-review".to_string(),
                    "Code review".to_string(),
                    Some(100.0 - used),
                    wham_reset_ms(window, now),
                    now,
                ));
            }
        }
    }
    if rows.is_empty() {
        return Err("no rate-limit windows in wham response".to_string());
    }
    let plan = body.get("plan_type").and_then(|v| v.as_str()).map(wham_plan_label).filter(|s| !s.is_empty());
    Ok((plan, rows))
}

const RESET_CREDITS_PATHS: [&str; 2] = ["/codex/rate-limit-reset-credits", "/wham/rate-limit-reset-credits"];

/// Banked rate-limit reset inventory (best effort — shape is undocumented).
/// Returns the raw body of the first path that answers 2xx, else None.
async fn fetch_reset_details(access_token: &str, account_id: &str) -> Option<serde_json::Value> {
    for path in RESET_CREDITS_PATHS {
        let mut req = client()
            .get(format!("https://chatgpt.com/backend-api{path}"))
            .bearer_auth(access_token.trim())
            .header("Accept", "application/json")
            .header("Origin", "https://chatgpt.com")
            .header("Referer", "https://chatgpt.com/");
        if !account_id.trim().is_empty() {
            req = req.header("ChatGPT-Account-Id", account_id.trim());
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    return Some(body);
                }
            }
            _ => continue,
        }
    }
    None
}

/// Renewal row + banked-reset rows, prepended ahead of the limit bars.
/// Everything is optional: unknown shapes simply contribute nothing.
fn wham_extra_rows(
    body: &serde_json::Value,
    details: Option<&serde_json::Value>,
    now: i64,
) -> Vec<QuotaRow> {
    let mut rows = Vec::new();
    // Renewal time from the weekly window, when present.
    if let Some(root) = wham_first_object(body, &["rate_limit", "rate_limits"]) {
        let primary = wham_first_object(root, &["five_hour", "primary_window", "primary"]);
        let secondary = wham_first_object(root, &["weekly", "secondary_window", "secondary"]);
        let mut weekly_ms: Option<i64> = None;
        for window in [primary, secondary].into_iter().flatten() {
            if wham_slot_name(wham_window_seconds(window), "") == "Weekly" {
                weekly_ms = wham_reset_ms(window, now).or(weekly_ms);
            }
        }
        if let Some(ms) = weekly_ms {
            rows.push(row("renewal".to_string(), "Renewal time".to_string(), None, Some(ms), now));
        }
    }
    // Banked manual resets from wham itself…
    let count = body
        .get("rate_limit_reset_credits")
        .and_then(|v| v.get("available_count"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
        .max(0);
    if count > 0 {
        rows.push(QuotaRow {
            id: "manual-resets".to_string(),
            label: "Manual resets".to_string(),
            remaining_pct: None,
            reset_text: String::new(),
            reset_in_text: format!("{count} available"),
            urgent: false,
        });
    }
    // …with per-credit expiry rows from the details endpoint when parseable.
    if let Some(details) = details {
        let items = ["credits", "rows", "items", "data"]
            .into_iter()
            .filter_map(|k| details.get(k).and_then(|v| v.as_array()))
            .next();
        if let Some(items) = items {
            for (index, item) in items.iter().enumerate() {
                let credit_id = ["id", "credit_id", "creditId"]
                    .into_iter()
                    .filter_map(|k| item.get(k).and_then(|v| v.as_str()))
                    .next()
                    .unwrap_or("");
                let title = ["title", "name", "description"]
                    .into_iter()
                    .filter_map(|k| item.get(k).and_then(|v| v.as_str()))
                    .next()
                    .filter(|t| !t.trim().is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("Reset {}", index + 1));
                let expiry = ["expires_at", "expiresAt", "expiry", "expires", "grant_expiry", "expiry_time"]
                    .into_iter()
                    .filter_map(|k| item.get(k).and_then(parse_reset))
                    .next();
                // Row id carries the real credit id for the consume call;
                // "auto-N" means the server should pick (see consume handler).
                let row_id = if credit_id.is_empty() {
                    format!("reset-credit:auto-{index}")
                } else {
                    format!("reset-credit:{credit_id}")
                };
                rows.push(row(row_id, title, None, expiry, now));
            }
        }
    }
    rows
}

#[derive(Debug, Deserialize)]
struct CodexTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
}

async fn refresh_codex_token(refresh_token: &str) -> Result<CodexTokenResponse, String> {
    let resp = client()
        .post(CODEX_TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", CODEX_CLIENT_ID),
            ("refresh_token", refresh_token),
        ])
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| format!("codex token refresh failed: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!("codex token refresh failed ({})", resp.status()));
    }
    resp.json().await.map_err(|err| format!("refresh parse failed: {err}"))
}

async fn codex_quota(
    cred: &crate::oauth::StoredCredential,
    provider_slug: &str,
    account_id: &str,
) -> Result<QuotaResult, String> {
    let now = now_ms();
    match wham_get(&cred.access_token, &cred.account_id).await {
        Ok(body) => {
            let (plan, mut quota) = wham_map(&body, now)?;
            let details = fetch_reset_details(&cred.access_token, &cred.account_id).await;
            let mut extras = wham_extra_rows(&body, details.as_ref(), now);
            extras.append(&mut quota);
            Ok(QuotaResult { plan, quota: extras, refreshed: false, model_count: None })
        }
        Err(err) if err == "unauthorized" => {
            // Singleflight: concurrent 401s serialize; waiters re-read the
            // stored secret first (the leader may have rotated already).
            let lock = crate::oauth::refresh_lock_for(provider_slug, account_id);
            let _guard = lock.lock().await;
            let stored_now = crate::secrets::read_raw_token(provider_slug, account_id)
                .ok()
                .and_then(|s| serde_json::from_str::<crate::oauth::StoredCredential>(&s).ok());
            if let Some(fresh) = stored_now {
                if fresh.access_token != cred.access_token {
                    if let Ok(body) = wham_get(&fresh.access_token, &fresh.account_id).await {
                        let now = now_ms();
                        let (plan, mut quota) = wham_map(&body, now)?;
                        let details = fetch_reset_details(&fresh.access_token, &fresh.account_id).await;
                        let mut extras = wham_extra_rows(&body, details.as_ref(), now);
                        extras.append(&mut quota);
                        return Ok(QuotaResult { plan, quota: extras, refreshed: true, model_count: None });
                    }
                }
            }
            let refresh = cred.refresh_token.clone().unwrap_or_default();
            if refresh.trim().is_empty() {
                return Err("ChatGPT session expired — sign in again.".to_string());
            }
            let tokens = refresh_codex_token(refresh.trim()).await.map_err(|_| "ChatGPT session expired — sign in again.".to_string())?;
            // id_token drops after exchange-time email extraction; refresh kept.
            let updated = crate::oauth::StoredCredential {
                access_token: tokens.access_token.clone(),
                refresh_token: tokens.refresh_token.or_else(|| cred.refresh_token.clone()),
                id_token: None,
                account_id: cred.account_id.clone(),
                email: cred.email.clone(),
                expires_in: tokens.expires_in,
            };
            let payload = serde_json::to_string(&updated).map_err(|err| err.to_string())?;
            crate::secrets::store_raw_token(provider_slug, account_id, &payload)?;
            let body = wham_get(&tokens.access_token, &cred.account_id)
                .await
                .map_err(|_| "ChatGPT session expired — sign in again.".to_string())?;
            let (plan, mut quota) = wham_map(&body, now)?;
            let details = fetch_reset_details(&tokens.access_token, &cred.account_id).await;
            let mut extras = wham_extra_rows(&body, details.as_ref(), now);
            extras.append(&mut quota);
            Ok(QuotaResult { plan, quota: extras, refreshed: true, model_count: None })
        }
        Err(err) => Err(err),
    }
}

// ---------------------------------------------------------------------------
// Antigravity (Google cloudcode-pa)
// ---------------------------------------------------------------------------

const ANTIGRAVITY_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const ANTIGRAVITY_CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
// NOTE: the client secret lives in oauth_secret (vault/env) — never here.

const LOAD_PROJECT_URLS: [&str; 3] = [
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
    "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
    "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:loadCodeAssist",
];
const MODELS_URLS: [&str; 3] = [
    "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
    "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
    "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:fetchAvailableModels",
];
const SUMMARY_URLS: [&str; 3] = [
    "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
    "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:retrieveUserQuotaSummary",
];

async fn cloudcode_post(
    label: &str,
    urls: &[&str],
    access_token: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    // Mirrors the proven antigravity-usage tool: prod first, `User-Agent:
    // antigravity` (generic UAs get rejected by these internal endpoints).
    // Auth failures are terminal; anything else falls through (regional 400s
    // included — another host may serve the account).
    let mut last_error = format!("{label}: all endpoints exhausted");
    for url in urls {
        let resp = match client()
            .post(*url)
            .bearer_auth(access_token.trim())
            .header("Content-Type", "application/json")
            .header("User-Agent", "antigravity")
            .json(body)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(err) => {
                last_error = format!("{label}: request failed: {err}");
                continue;
            }
        };
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(status.to_string());
        }
        if status.is_success() {
            return resp.json().await.map_err(|err| format!("quota parse failed: {err}"));
        }
        last_error = format!("{label}: quota check failed ({status})");
    }
    Err(last_error)
}

fn antigravity_tier_label(body: &serde_json::Value) -> Option<String> {
    fn pick(tier: Option<&serde_json::Value>) -> String {
        match tier {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(obj) => obj
                .get("id")
                .or_else(|| obj.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            None => String::new(),
        }
    }
    let tier = {
        let paid = pick(body.get("paidTier").or_else(|| body.get("paid_tier")));
        if !paid.is_empty() {
            paid
        } else {
            ["currentTier", "current_tier", "subscriptionTier", "subscription_tier", "tier"]
                .into_iter()
                .map(|key| pick(body.get(key)))
                .find(|value| !value.is_empty())
                .unwrap_or_else(|| "free-tier".to_string())
        }
    };
    if tier.to_lowercase().contains("free") {
        return Some("Free".to_string());
    }
    Some(tier)
}

fn interesting_model(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with("gemini") || lower.starts_with("claude") || lower.starts_with("gpt") || lower.starts_with("image") || lower.starts_with("imagen")
}

/// Per-model Antigravity quota rows from a `fetchAvailableModels` body
/// (object table or array shape). Pure mapping, extracted for tests.
/// Colliding display names are suffixed with their native id — upstream
/// reuses one display name across several model ids, and bare duplicates
/// read as a rendering bug.
fn antigravity_model_rows(models_body: &serde_json::Value, now: i64) -> Vec<QuotaRow> {
    let mut pending: Vec<(String, String, f64, Option<i64>)> = Vec::new();
    let mut push_model_row = |name: String, info: &serde_json::Value| {
        if !interesting_model(&name) {
            return;
        }
        let Some(quota) = info.get("quotaInfo") else { return };
        let fraction = quota
            .get("remainingFraction")
            .and_then(serde_json::Value::as_f64)
            .or_else(|| info.get("remainingFraction").and_then(serde_json::Value::as_f64));
        let Some(fraction) = fraction else { return };
        let label = info
            .get("displayName")
            .and_then(|v| v.as_str())
            .unwrap_or(&name)
            .to_string();
        let reset = quota.get("resetTime").and_then(parse_reset).or_else(|| info.get("resetTime").and_then(parse_reset));
        pending.push((name, label, fraction * 100.0, reset));
    };
    if let Some(models) = models_body.get("models") {
        if let Some(table) = models.as_object() {
            for (name, info) in table {
                push_model_row(name.clone(), info);
            }
        } else if let Some(list) = models.as_array() {
            for (index, info) in list.iter().enumerate() {
                let name = info
                    .get("name")
                    .or_else(|| info.get("id"))
                    .or_else(|| info.get("model"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("model-{index}"));
                push_model_row(name, info);
            }
        }
    }
    let mut label_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (_, label, _, _) in &pending {
        *label_counts.entry(label.clone()).or_insert(0) += 1;
    }
    pending
        .into_iter()
        .map(|(name, label, pct, reset)| {
            let label = if label_counts.get(&label).copied().unwrap_or(0) > 1 {
                format!("{label} · {name}")
            } else {
                label
            };
            row(format!("model-{name}"), label, Some(pct), reset, now)
        })
        .collect()
}

async fn antigravity_quota(access_token: &str) -> Result<QuotaResult, String> {
    let now = now_ms();
    let project_body = cloudcode_post(
        "project",
        &LOAD_PROJECT_URLS,
        access_token,
        &serde_json::json!({"metadata": {"ideType": "ANTIGRAVITY", "platform": "PLATFORM_UNSPECIFIED", "pluginType": "GEMINI"}}),
    )
    .await;
    let (project_id, tier) = match project_body {
        Ok(body) => {
            let project = body.get("cloudaicompanionProject").and_then(|v| v.as_str()).map(str::to_string);
            (project, antigravity_tier_label(&body))
        }
        Err(err) if is_auth_failure(&err) => {
            return Err("Google rejected the credential — sign in again.".to_string())
        }
        Err(_) => (None, None),
    };
    let quota_body_arg = project_id.clone().map(|pid| serde_json::json!({"project": pid})).unwrap_or_else(|| serde_json::json!({}));
    let mut rows: Vec<QuotaRow> = Vec::new();
    // Grouped weekly/5h buckets first — the curated, screenshot-like rows.
    if let Ok(summary) = cloudcode_post("summary", &SUMMARY_URLS, access_token, &quota_body_arg).await {
        if let Some(groups) = summary.get("groups").and_then(|v| v.as_array()) {
            for group in groups {
                let group_name = group.get("displayName").and_then(|v| v.as_str()).unwrap_or("Quota");
                if let Some(buckets) = group.get("buckets").and_then(|v| v.as_array()) {
                    for bucket in buckets {
                        let fraction = match bucket.get("remainingFraction").and_then(serde_json::Value::as_f64) {
                            Some(f) => f,
                            None => continue,
                        };
                        let window = bucket
                            .get("displayName")
                            .or_else(|| bucket.get("window"))
                            .or_else(|| bucket.get("bucketId"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let label = if window.is_empty() { group_name.to_string() } else { format!("{group_name} · {window}") };
                        let reset = bucket.get("resetTime").and_then(parse_reset);
                        rows.push(row(
                            format!("bucket-{}-{}", group_name.to_lowercase().replace(' ', "-"), window.to_lowercase().replace(' ', "-")),
                            label,
                            Some(fraction * 100.0),
                            reset,
                            now,
                        ));
                    }
                }
            }
        }
    }
    // Fallback: per-model fractions when no grouped summary exists.
    // Upstream reuses one display name for several native model ids (e.g.
    // three "Gemini 3.5 Flash Lite" rows) — collect first, then suffix
    // colliding labels with their native id so identical-looking rows stay
    // distinguishable. Nothing is merged or hidden: every reported quota
    // still renders exactly once.
    if rows.is_empty() {
        match cloudcode_post("models", &MODELS_URLS, access_token, &quota_body_arg).await {
            Ok(models_body) => {
                rows.extend(antigravity_model_rows(&models_body, now));
            }
            Err(err) if is_auth_failure(&err) => {
                return Err("Google rejected the credential — sign in again.".to_string())
            }
            Err(err) => {
                if tier.is_none() {
                    return Err(err);
                }
            }
        }
    }
    if rows.is_empty() && tier.is_none() {
        return Err("no quota data returned — try again later.".to_string());
    }
    Ok(QuotaResult { plan: tier, quota: rows, refreshed: false, model_count: None })
}

#[derive(Debug, Deserialize)]
pub(crate) struct GoogleTokenResponse {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_in: Option<i64>,
}

/// Google OAuth refresh, shared with the catalog module so a stale access
/// token doesn't fail a catalog refresh the quota path would have saved.
pub(crate) async fn refresh_google_token(refresh_token: &str) -> Result<GoogleTokenResponse, String> {
    let client_secret = crate::oauth_secret::read()?;
    let resp = client()
        .post(ANTIGRAVITY_TOKEN_URL)
        .form(&[
            ("refresh_token", refresh_token),
            ("client_id", ANTIGRAVITY_CLIENT_ID),
            ("client_secret", client_secret.as_str()),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|err| format!("google token refresh failed: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!("google token refresh failed ({})", resp.status()));
    }
    resp.json().await.map_err(|err| format!("refresh parse failed: {err}"))
}

async fn antigravity_quota_for(
    cred: &crate::oauth::StoredCredential,
    provider_slug: &str,
    account_id: &str,
) -> Result<QuotaResult, String> {
    match antigravity_quota(&cred.access_token).await {
        Ok(result) => Ok(result),
        Err(err) if is_auth_failure(&err) => {
            // Singleflight around refresh+persist (see codex path).
            let lock = crate::oauth::refresh_lock_for(provider_slug, account_id);
            let _guard = lock.lock().await;
            let stored_now = crate::secrets::read_raw_token(provider_slug, account_id)
                .ok()
                .and_then(|s| serde_json::from_str::<crate::oauth::StoredCredential>(&s).ok());
            if let Some(fresh) = stored_now {
                if fresh.access_token != cred.access_token {
                    if let Ok(result) = antigravity_quota(&fresh.access_token).await {
                        let mut result = result;
                        result.refreshed = true;
                        return Ok(result);
                    }
                }
            }
            let refresh = cred.refresh_token.clone().unwrap_or_default();
            if refresh.trim().is_empty() {
                return Err("Google session expired — sign in again.".to_string());
            }
            let tokens = refresh_google_token(refresh.trim())
                .await
                .map_err(|_| "Google session expired — sign in again.".to_string())?;
            let updated = crate::oauth::StoredCredential {
                access_token: tokens.access_token.clone(),
                refresh_token: tokens.refresh_token.or_else(|| cred.refresh_token.clone()),
                id_token: None,
                account_id: cred.account_id.clone(),
                email: cred.email.clone(),
                expires_in: tokens.expires_in,
            };
            let payload = serde_json::to_string(&updated).map_err(|err| err.to_string())?;
            crate::secrets::store_raw_token(provider_slug, account_id, &payload)?;
            let mut result = antigravity_quota(&tokens.access_token).await.map_err(|_| "Google session expired — sign in again.".to_string())?;
            result.refreshed = true;
            Ok(result)
        }
        Err(err) => Err(err),
    }
}

// ---------------------------------------------------------------------------
// Claude (`GET https://api.anthropic.com/api/oauth/usage`, Bearer +
// `anthropic-beta: oauth-2025-04-20` + `User-Agent: claude-code/<version>`).
// Response (community-verified, mirrors Claude Code's `/usage`):
// `{five_hour: {utilization, resets_at}, seven_day: {...},
// seven_day_opus: {...}|null, seven_day_sonnet: {...}|null}`.
// `utilization` is percent USED, so remaining = 100 - utilization. Null
// windows (unused model buckets) render no row — never fabricated.
// The endpoint names no subscription plan, so `plan` stays None (unknown
// means unknown). API keys have no usage endpoint: key refresh re-verifies
// via the Models API and returns the model count with no rows.
// ---------------------------------------------------------------------------

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

fn claude_usage_window(rows: &mut Vec<QuotaRow>, id: &str, label: &str, window: Option<&serde_json::Value>, now: i64) {
    let Some(window) = window else { return };
    let Some(used) = window.get("utilization").and_then(serde_json::Value::as_f64) else { return };
    if !used.is_finite() {
        return;
    }
    let reset = window.get("resets_at").or_else(|| window.get("resetsAt")).and_then(parse_reset);
    rows.push(row(id.to_string(), label.to_string(), Some(100.0 - used), reset, now));
}

fn map_claude_usage(body: &serde_json::Value, now: i64) -> Vec<QuotaRow> {
    let mut rows = Vec::new();
    claude_usage_window(&mut rows, "claude-five-hour", "5-hour", body.get("five_hour"), now);
    claude_usage_window(&mut rows, "claude-seven-day", "Weekly", body.get("seven_day"), now);
    claude_usage_window(&mut rows, "claude-seven-day-opus", "Weekly · Opus", body.get("seven_day_opus"), now);
    claude_usage_window(&mut rows, "claude-seven-day-sonnet", "Weekly · Sonnet", body.get("seven_day_sonnet"), now);
    rows
}

async fn claude_usage_get(access_token: &str) -> Result<serde_json::Value, String> {
    let resp = client()
        .get(CLAUDE_USAGE_URL)
        .bearer_auth(access_token.trim())
        .header("anthropic-beta", crate::adapters::claude::OAUTH_BETA_SINGLE)
        .header("User-Agent", crate::adapters::claude::USAGE_USER_AGENT)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| format!("claude usage check failed: {err}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED || resp.status() == reqwest::StatusCode::FORBIDDEN {
        return Err("unauthorized".to_string());
    }
    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err("claude usage is rate-limited right now — try again in a few minutes".to_string());
    }
    if !resp.status().is_success() {
        return Err(format!("claude usage check failed ({})", resp.status()));
    }
    resp.json().await.map_err(|err| format!("usage parse failed: {err}"))
}

async fn claude_quota_for(
    cred: &crate::oauth::StoredCredential,
    provider_slug: &str,
    account_id: &str,
) -> Result<QuotaResult, String> {
    match claude_usage_get(&cred.access_token).await {
        Ok(body) => Ok(QuotaResult { plan: None, quota: map_claude_usage(&body, now_ms()), refreshed: false, model_count: None }),
        Err(err) if is_auth_failure(&err) => {
            // Singleflight around refresh+persist (see codex path).
            let lock = crate::oauth::refresh_lock_for(provider_slug, account_id);
            let _guard = lock.lock().await;
            let stored_now = crate::secrets::read_raw_token(provider_slug, account_id)
                .ok()
                .and_then(|s| serde_json::from_str::<crate::oauth::StoredCredential>(&s).ok());
            if let Some(fresh) = stored_now {
                if fresh.access_token != cred.access_token {
                    if let Ok(body) = claude_usage_get(&fresh.access_token).await {
                        return Ok(QuotaResult { plan: None, quota: map_claude_usage(&body, now_ms()), refreshed: true, model_count: None });
                    }
                }
            }
            let refresh = cred.refresh_token.clone().unwrap_or_default();
            if refresh.trim().is_empty() {
                return Err("Claude session expired — sign in again.".to_string());
            }
            let (access, rotated) = crate::adapters::claude::refresh_access_token(refresh.trim())
                .await
                .map_err(|_| "Claude session expired — sign in again.".to_string())?;
            let updated = crate::oauth::StoredCredential {
                access_token: access.clone(),
                refresh_token: rotated.or_else(|| cred.refresh_token.clone()),
                id_token: None,
                account_id: cred.account_id.clone(),
                email: cred.email.clone(),
                expires_in: cred.expires_in,
            };
            let payload = serde_json::to_string(&updated).map_err(|err| err.to_string())?;
            crate::secrets::store_raw_token(provider_slug, account_id, &payload)?;
            let body = claude_usage_get(&access).await.map_err(|_| "Claude session expired — sign in again.".to_string())?;
            Ok(QuotaResult { plan: None, quota: map_claude_usage(&body, now_ms()), refreshed: true, model_count: None })
        }
        Err(err) => Err(err),
    }
}

// ---------------------------------------------------------------------------
// OpenCode (`/zen/go/v1/usage`, shape verified against the open-source
// console route): `{usage: {rolling|weekly|monthly: {status, percent,
// resetsAt}}}`. `percent` is usage consumed, so remaining = 100 - percent.
// Unknown shapes yield no rows — never fabricated bars.
// ---------------------------------------------------------------------------

/// Plan from the usage-check HTTP status: 200 with windows = "Go",
/// 403 EntitlementError = valid key without a subscription ("Free").
pub(crate) fn opencode_plan_for_status(status: u16) -> Option<&'static str> {
    match status {
        200 => Some("Go"),
        403 => Some("Free"),
        _ => None,
    }
}

fn map_opencode_usage(body: &serde_json::Value, now: i64) -> Vec<QuotaRow> {
    let mut rows = Vec::new();
    let Some(usage) = body.get("usage") else { return rows };
    for (key, label) in [("rolling", "5-hour limit"), ("weekly", "Weekly limit"), ("monthly", "Monthly limit")] {
        let Some(window) = usage.get(key) else { continue };
        let Some(percent) = window.get("percent").and_then(serde_json::Value::as_f64) else { continue };
        if !percent.is_finite() {
            continue;
        }
        let reset = window.get("resetTime").or_else(|| window.get("resetsAt")).and_then(parse_reset);
        let mut quota_row = row(format!("opencode-{key}"), label.to_string(), Some(100.0 - percent), reset, now);
        if window.get("status").and_then(|v| v.as_str()) == Some("rate-limited") {
            quota_row.urgent = true;
        }
        rows.push(quota_row);
    }
    rows
}

// ---------------------------------------------------------------------------
// Command Code (`/alpha/*`, mirroring the official CLI mapping)
// ---------------------------------------------------------------------------

const COMMANDCODE_API_BASE: &str = "https://api.commandcode.ai";
const COMMANDCODE_CLIENT_VERSION: &str = "0.24.1";

fn commandcode_plan(plan_id: &str) -> Option<(String, f64)> {
    let key = plan_id.trim().to_lowercase().replace('_', "-");
    // Longest-prefix match, like the CLI.
    let mut best: Option<(&str, &str, f64)> = None;
    for (id, name, monthly) in [
        ("individual-go", "Go", 10.0),
        ("individual-goat", "GOAT", 70.0),
        ("individual-pro-v1", "Pro", 80.0),
        ("individual-pro", "Pro", 30.0),
        ("individual-provider", "Provider", 15.0),
        ("individual-max", "Max", 150.0),
        ("individual-ultra", "Ultra", 300.0),
        ("teams-pro", "Teams Pro", 40.0),
    ] {
        if key.starts_with(id) && best.map(|(b, _, _)| id.len() > b.len()).unwrap_or(true) {
            best = Some((id, name, monthly));
        }
    }
    best.map(|(_, name, monthly)| (name.to_string(), monthly))
}

/// Limit window from any nesting. Accepts unambiguous direct-remaining
/// shapes first (`remainingPercent` / `remaining` + cap), then the classic
/// `{used, cap}` pair. None when nothing usable is present.
fn limit_window_row(id: &str, label: &str, window: &serde_json::Value, now: i64) -> Option<QuotaRow> {
    let num = |keys: &[&str]| -> Option<f64> {
        keys.iter().filter_map(|k| window.get(*k)).filter_map(serde_json::Value::as_f64).next()
    };
    let reset = window.get("resetAt").or_else(|| window.get("reset")).or_else(|| window.get("expiry")).and_then(parse_reset);
    if let Some(pct) = num(&["remainingPercent", "percentLeft", "percent_left"]) {
        if pct.is_finite() {
            return Some(row(id.to_string(), label.to_string(), Some(pct), reset, now));
        }
    }
    if let Some(remaining) = num(&["remaining"]) {
        if let Some(cap) = num(&["cap", "limit", "total"]) {
            if cap > 0.0 {
                return Some(row(id.to_string(), label.to_string(), Some(100.0 * remaining / cap), reset, now));
            }
        }
    }
    let used = num(&["used"])?;
    let cap = num(&["cap"])?;
    if !used.is_finite() || !cap.is_finite() || cap <= 0.0 {
        return None;
    }
    Some(row(id.to_string(), label.to_string(), Some(100.0 * (1.0 - used / cap)), reset, now))
}

/// Push a limit-window row unless that id already rendered (dedupe across
/// the primary mapping and fallback nestings).
fn push_missing_window(rows: &mut Vec<QuotaRow>, id: &str, label: &str, window: Option<&serde_json::Value>, now: i64) {
    if rows.iter().any(|r| r.id == id) {
        return;
    }
    if let Some(window) = window {
        if let Some(rendered) = limit_window_row(id, label, window, now) {
            rows.push(rendered);
        }
    }
}

/// Day-level timestamp: epoch ms/s and RFC-3339 via [`parse_reset`], plus
/// plain `YYYY-MM-DD` dates.
fn parse_day(value: &serde_json::Value) -> Option<i64> {
    if let Some(ms) = parse_reset(value) {
        return Some(ms);
    }
    if let Some(text) = value.as_str() {
        if let Ok(day) = chrono::NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d") {
            return day.and_hms_opt(0, 0, 0).map(|t| t.and_utc().timestamp_millis());
        }
    }
    None
}

/// Week-to-date spend from the already-fetched usage summary. Accepts an
/// explicit weekly key or a daily breakdown (summing entries dated within
/// the last 7 days). None when the shape carries nothing weekly — the caller
/// then shows no row rather than a fabricated one.
fn weekly_spend(summary: Option<&serde_json::Value>, now: i64) -> Option<f64> {
    let summary = summary?;
    for key in ["weeklyCost", "weekToDate", "weeklyUsage", "totalWeeklyCost", "weekSpend"] {
        if let Some(value) = summary.get(key).and_then(serde_json::Value::as_f64) {
            if value.is_finite() {
                return Some(value.max(0.0));
            }
        }
    }
    for key in ["daily", "byDay", "days", "breakdown", "history"] {
        let Some(items) = summary.get(key).and_then(|v| v.as_array()) else { continue };
        let mut sum = 0.0;
        let mut dated = 0u32;
        for item in items {
            let Some(cost) = ["cost", "totalCost", "amount", "spent", "value"]
                .iter()
                .filter_map(|k| item.get(*k))
                .filter_map(serde_json::Value::as_f64)
                .next()
            else {
                continue;
            };
            if !cost.is_finite() {
                continue;
            }
            // Undated entries can't be scoped to the week — skip them rather
            // than misattributing period spend as weekly spend.
            let day = item
                .get("date")
                .or_else(|| item.get("day"))
                .or_else(|| item.get("period"))
                .and_then(parse_day);
            match day {
                Some(ms) if ms <= now && now - ms <= 7 * 86_400_000 => {
                    sum += cost.max(0.0);
                    dated += 1;
                }
                _ => {}
            }
        }
        if dated > 0 {
            return Some(sum);
        }
    }
    None
}

async fn commandcode_get(api_key: &str, path: &str, query: &[(&str, &str)]) -> Result<serde_json::Value, String> {    let mut url = format!("{COMMANDCODE_API_BASE}{path}");
    if !query.is_empty() {
        let qs: Vec<String> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
        url = format!("{url}?{}", qs.join("&"));
    }
    let resp = client()
        .get(&url)
        .bearer_auth(api_key.trim())
        .header("Accept", "application/json")
        .header("x-command-code-version", COMMANDCODE_CLIENT_VERSION)
        .send()
        .await
        .map_err(|err| format!("commandcode request failed: {err}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED || resp.status() == reqwest::StatusCode::FORBIDDEN {
        return Err("unauthorized".to_string());
    }
    if !resp.status().is_success() {
        return Err(format!("commandcode request failed ({})", resp.status()));
    }
    resp.json().await.map_err(|err| format!("commandcode parse failed: {err}"))
}

async fn commandcode_quota(api_key: &str) -> Result<QuotaResult, String> {
    let now = now_ms();
    let whoami = commandcode_get(api_key, "/alpha/whoami", &[("limits", "1")])
        .await
        .map_err(|err| {
            if err == "unauthorized" {
                "Command Code rejected the key (unauthorized) — check it and try again".to_string()
            } else {
                err
            }
        })?;
    let org_id = whoami.get("org").and_then(|o| o.get("id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let org_param;
    let org_query: &[(&str, &str)] = if org_id.is_empty() {
        &[]
    } else {
        org_param = org_id.clone();
        &[("orgId", &org_param)]
    };
    // Credits + subscription in parallel; either may 403 on plans without
    // billing access — proceed with whatever succeeds.
    let (credits, subscription) = tokio::join!(
        commandcode_get(api_key, "/alpha/billing/credits", org_query),
        commandcode_get(api_key, "/alpha/billing/subscriptions", org_query)
    );
    let credits_body = credits.ok();
    let subscription_body = subscription.ok();
    let credits_obj = credits_body.as_ref().and_then(|b| b.get("credits"));
    let sub_obj = subscription_body.as_ref().and_then(|b| b.get("data"));
    let plan_info = sub_obj
        .and_then(|s| s.get("planId"))
        .and_then(|v| v.as_str())
        .and_then(commandcode_plan)
        .or_else(|| {
            credits_obj
                .and_then(|c| c.get("planId"))
                .and_then(|v| v.as_str())
                .and_then(commandcode_plan)
        });
    let plan = plan_info.clone().map(|(name, _)| name);
    let mut rows = Vec::new();
    if let Some(windows) = credits_obj.and_then(|c| c.get("windowLimits")) {
        for (id, label) in [("fiveHour", "5-hour"), ("weekly", "Weekly")] {
            if let Some(window) = windows.get(id) {
                let used = window.get("used").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
                let cap = window.get("cap").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
                if cap > 0.0 {
                    rows.push(row(
                        format!("window-{id}"),
                        label.to_string(),
                        Some(100.0 * (1.0 - used / cap)),
                        window.get("resetAt").and_then(parse_reset),
                        now,
                    ));
                }
            }
        }
    }
    // Fallback: the `limits=1` whoami response itself carries limit windows
    // on several plans, and some variants nest the weekly window outside
    // `windowLimits`. Each id renders at most once — the id doubles as the
    // dedupe key, with the primary `windowLimits` mapping winning.
    {
        // whoami nests first (this is the live meter source on plans where
        // billing endpoints stay silent).
        for nest in ["limits", "windowLimits", "usage", "quotas", "rateLimits"] {
            if let Some(group) = whoami.get(nest) {
                for (key, id, label) in [
                    ("fiveHour", "window-fiveHour", "5-hour"),
                    ("five_hour", "window-fiveHour", "5-hour"),
                    ("weekly", "window-weekly", "Weekly"),
                    ("monthly", "window-monthly", "Monthly"),
                ] {
                    push_missing_window(&mut rows, id, label, group.get(key), now);
                }
            }
        }
        // Alternative weekly nestings on the credits object.
        if let Some(credits) = credits_obj {
            for alt in [
                credits.get("limits").and_then(|l| l.get("weekly")),
                credits.get("weeklyLimit"),
            ] {
                if rows.iter().any(|r| r.id == "window-weekly") {
                    break;
                }
                push_missing_window(&mut rows, "window-weekly", "Weekly", alt, now);
            }
        }
    }
    // Credits pool row, like the CLI's usage percent + renewal countdown:
    // spent comes from the usage summary, pool from plan + remaining credits.
    let monthly = credits_obj.and_then(|c| c.get("monthlyCredits")).and_then(serde_json::Value::as_f64).unwrap_or(0.0).max(0.0);
    let purchased = credits_obj.and_then(|c| c.get("purchasedCredits")).and_then(serde_json::Value::as_f64).unwrap_or(0.0).max(0.0);
    let free = credits_obj.and_then(|c| c.get("freeCredits")).and_then(serde_json::Value::as_f64).unwrap_or(0.0).max(0.0);
    let plan_monthly = plan_info.map(|(_, monthly)| monthly).unwrap_or(0.0);
    let pool = plan_monthly.max(monthly) + purchased + free;
    let since = sub_obj
        .and_then(|s| s.get("currentPeriodStart"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut summary_query: Vec<(&str, &str)> = org_query.to_vec();
    if !since.is_empty() {
        summary_query.push(("since", since));
    }
    let summary_body = commandcode_get(api_key, "/alpha/usage/summary", &summary_query).await.ok();
    let spent = summary_body
        .as_ref()
        .and_then(|b| b.get("totalCost"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0)
        .max(0.0);
    // Week-to-date spend (bar-less info row) from the same summary body.
    if let Some(week) = weekly_spend(summary_body.as_ref(), now) {
        rows.push(QuotaRow {
            id: "weekly-usage".to_string(),
            label: "Weekly usage".to_string(),
            remaining_pct: None,
            reset_text: format!("${week:.2}"),
            reset_in_text: "this week".to_string(),
            urgent: false,
        });
    }
    if pool > 0.0 && (spent > 0.0 || pool - spent > 0.0) {
        let period_end = sub_obj.and_then(|s| s.get("currentPeriodEnd")).and_then(parse_reset);
        let reset_text = period_end.map(fmt_reset).unwrap_or_default();
        let (reset_in_text, urgent) = match period_end {
            Some(ms) => {
                let days = ((ms.saturating_sub(now).max(0)) as f64 / 86_400_000.0).ceil() as i64;
                (format!("in {days} day{}", if days == 1 { "" } else { "s" }), days < 3)
            }
            None => (String::new(), false),
        };
        rows.push(QuotaRow {
            id: "credits".to_string(),
            label: "Credits".to_string(),
            remaining_pct: Some(clamp_pct(100.0 * (pool - spent).max(0.0) / pool)),
            reset_text,
            reset_in_text,
            urgent,
        });
    }
    if rows.is_empty() && plan.is_none() {
        return Err("No quota data returned for this key — the plan may not expose usage.".to_string());
    }
    Ok(QuotaResult { plan, quota: rows, refreshed: false, model_count: None })
}

// ---------------------------------------------------------------------------
// Tauri command
// ---------------------------------------------------------------------------

/// Refresh live quota for one linked account. Reads the stored credential
/// (keyring on desktop), refreshes OAuth tokens on 401 where possible,
/// persists refreshed tokens, and returns plan + quota rows for display.
/// The observed plan is also persisted for gateway transport selection.
#[tauri::command]
pub async fn refresh_quota(
    pool: tauri::State<'_, sqlx::SqlitePool>,
    provider_slug: String,
    account_id: String,
) -> Result<QuotaResult, String> {
    refresh_and_record(&pool, &provider_slug, &account_id).await
}

/// Shared refresh path for the Tauri command and `POST
/// /api/accounts/:provider/:id/quota/refresh` (single quota path). Runs the
/// provider fetch, registers routing metadata, and appends a
/// `quota_observations` row — all persistence best-effort, never failing the
/// refresh itself. `remaining` stores canonical `Vec<QuotaRow>` JSON;
/// `reset_at` stays NULL (per-row reset text lives inside the JSON).
pub(crate) async fn refresh_and_record(
    pool: &sqlx::SqlitePool,
    provider_slug: &str,
    account_id: &str,
) -> Result<QuotaResult, String> {
    let result = refresh_quota_inner(provider_slug, account_id).await?;
    // Best-effort routing metadata (never fails the refresh itself).
    let _ = crate::db::register_account(pool, provider_slug, account_id, account_id, result.plan.as_deref()).await;
    // A successful quota fetch validates the account for routing (see the
    // `verified` column, migration 011): pasted OAuth that never validated
    // stays ineligible until this path succeeds.
    let _ = crate::db::mark_account_verified(pool, provider_slug, account_id).await;
    let remaining = serde_json::to_string(&result.quota).unwrap_or_else(|_| "[]".to_string());
    let _ = sqlx::query("INSERT INTO quota_observations (provider_slug, account_id, remaining, reset_at) VALUES (?, ?, ?, NULL)")
        .bind(provider_slug)
        .bind(account_id)
        .bind(remaining)
        .execute(pool)
        .await;
    // Retention cap (best-effort, never fails the refresh): keep the newest
    // 10k observation rows; per-account history stays bounded on busy DBs.
    let _ = sqlx::query(
        "DELETE FROM quota_observations WHERE id NOT IN (SELECT id FROM quota_observations ORDER BY id DESC LIMIT 10000)",
    )
    .execute(pool)
    .await;
    Ok(result)
}

async fn refresh_quota_inner(provider_slug: &str, account_id: &str) -> Result<QuotaResult, String> {
    let secret = crate::secrets::read_raw_token(provider_slug, account_id)
        .map_err(|_| "No saved credential found — sign in again.".to_string())?;
    if secret.trim().is_empty() {
        return Err("No saved credential found — sign in again.".to_string());
    }
    if let Ok(cred) = serde_json::from_str::<crate::oauth::StoredCredential>(&secret) {
        return match provider_slug {
            "chatgpt" => codex_quota(&cred, provider_slug, account_id).await,
            "antigravity" => antigravity_quota_for(&cred, provider_slug, account_id).await,
            "claude" => claude_quota_for(&cred, provider_slug, account_id).await,
            _ => Err("Quota is not available for this credential — sign in via browser instead.".to_string()),
        };
    }
    match provider_slug {
        "commandcode" => commandcode_quota(secret.trim()).await,
        "opencode" => {
            // Re-verify the key: the usage body carries the real quota
            // windows plus the provider-reported plan ("Go" vs "Free").
            // Never deletes anything on failure.
            match crate::oauth::verify_opencode_key(secret.trim()).await {
                Ok(status) => Ok(QuotaResult {
                    plan: Some(status.plan),
                    quota: status.usage.as_ref().map(|body| map_opencode_usage(body, now_ms())).unwrap_or_default(),
                    refreshed: false,
                    model_count: Some(status.models.len()),
                }),
                Err(err) => Err(err),
            }
        }
        "chatgpt" => {
            // Possibly a pasted raw OAuth access token: best effort, no
            // account id, no refresh.
            let now = now_ms();
            match wham_get(secret.trim(), "").await {
                Ok(body) => {
                    let (plan, mut quota) = wham_map(&body, now)?;
                    let details = fetch_reset_details(secret.trim(), "").await;
                    let mut extras = wham_extra_rows(&body, details.as_ref(), now);
                    extras.append(&mut quota);
                    Ok(QuotaResult { plan, quota: extras, refreshed: false, model_count: None })
                }
                Err(err) if err == "unauthorized" => Err("Token rejected or expired — sign in again.".to_string()),
                Err(err) => Err(err),
            }
        }
        "antigravity" => match antigravity_quota(secret.trim()).await {
            Ok(result) => Ok(result),
            Err(err) if is_auth_failure(&err) => {
                Err("Google rejected the credential — sign in again.".to_string())
            }
            Err(err) => Err(err),
        },
        "claude" => {
            // Raw credentials: API keys expose no usage endpoint (re-verify
            // via the Models API, returning the model count with no rows);
            // pasted `sk-ant-oat-*` setup tokens are OAuth bearers without
            // refresh, so the usage endpoint is tried directly. Never
            // deletes anything on failure.
            use crate::adapters::claude as cl;
            let Some(cred) = cl::parse_serving_cred(secret.trim()) else {
                return Err("No saved credential found — sign in again.".to_string());
            };
            match cred.kind {
                cl::CredKind::ApiKey => match cl::fetch_models(&cred.access_token, cl::CredKind::ApiKey).await {
                    Ok(entries) => Ok(QuotaResult { plan: None, quota: Vec::new(), refreshed: false, model_count: Some(entries.len()) }),
                    Err(err) => Err(err),
                },
                cl::CredKind::OAuth => match claude_usage_get(&cred.access_token).await {
                    Ok(body) => Ok(QuotaResult { plan: None, quota: map_claude_usage(&body, now_ms()), refreshed: false, model_count: None }),
                    Err(err) if is_auth_failure(&err) => Err("Claude session expired — sign in again.".to_string()),
                    Err(err) => Err(err),
                },
            }
        }
        _ => Err("Quota is not available for this provider.".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opencode_body() -> serde_json::Value {
        serde_json::json!({
            "usage": {
                "rolling": {"status": "ok", "percent": 13, "resetsAt": "2026-09-28T02:00:00.000Z"},
                "weekly": {"status": "rate-limited", "percent": 100, "resetsAt": "2026-10-01T00:00:00.000Z"},
                "monthly": {"status": "ok", "percent": 42.5, "resetsAt": "2026-11-01T00:00:00.000Z"},
            }
        })
    }

    #[test]
    fn claude_usage_maps_windows_and_skips_nulls() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-27T12:00:00Z").unwrap().timestamp_millis();
        let body = serde_json::json!({
            "five_hour": {"utilization": 33.0, "resets_at": "2026-09-27T14:00:00.000Z"},
            "seven_day": {"utilization": 13.0, "resets_at": "2026-10-01T00:00:00.000Z"},
            "seven_day_opus": null,
            "seven_day_sonnet": {"utilization": 1.0, "resets_at": "2026-09-30T00:00:00.000Z"},
        });
        let rows = map_claude_usage(&body, now);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, "claude-five-hour");
        assert_eq!(rows[0].label, "5-hour");
        // utilization is percent USED — remaining is the complement.
        assert_eq!(rows[0].remaining_pct, Some(67.0));
        assert_eq!(rows[1].id, "claude-seven-day");
        assert_eq!(rows[1].remaining_pct, Some(87.0));
        assert_eq!(rows[2].id, "claude-seven-day-sonnet");
        assert_eq!(rows[2].remaining_pct, Some(99.0));
        assert!(!rows[0].reset_text.is_empty());
        // Unknown shapes yield no rows — never fabricated.
        assert!(map_claude_usage(&serde_json::json!({}), now).is_empty());
        assert!(map_claude_usage(&serde_json::json!({"five_hour": {"utilization": "lots"}}), now).is_empty());
    }

    #[test]
    fn antigravity_model_rows_disambiguate_colliding_labels() {
        let now = now_ms();
        let body = serde_json::json!({
            "models": {
                "gemini-3.5-flash-lite": {"displayName": "Gemini 3.5 Flash Lite", "quotaInfo": {"remainingFraction": 1.0, "resetTime": "2026-09-30T13:55:00.000Z"}},
                "gemini-3.5-flash-lite-low": {"displayName": "Gemini 3.5 Flash Lite", "quotaInfo": {"remainingFraction": 1.0, "resetTime": "2026-09-30T13:55:00.000Z"}},
                "gemini-2.5-pro": {"displayName": "Gemini 2.5 Pro", "quotaInfo": {"remainingFraction": 0.5, "resetTime": "2026-09-30T13:55:00.000Z"}},
                "some-internal-thing": {"displayName": "Internal", "quotaInfo": {"remainingFraction": 1.0}},
                "gemini-no-quota": {"displayName": "Gemini No Quota"}
            }
        });
        let rows = antigravity_model_rows(&body, now);
        // "some-internal-thing" filtered (uninteresting prefix), "gemini-no-quota" skipped (no fraction).
        assert_eq!(rows.len(), 3);
        // Colliding labels carry their native ids; unique labels stay bare.
        assert!(rows.iter().any(|r| r.label == "Gemini 3.5 Flash Lite · gemini-3.5-flash-lite"), "{rows:?}");
        assert!(rows.iter().any(|r| r.label == "Gemini 3.5 Flash Lite · gemini-3.5-flash-lite-low"), "{rows:?}");
        assert!(rows.iter().any(|r| r.label == "Gemini 2.5 Pro"), "{rows:?}");
        // Stable distinct ids, values preserved 1:1 — nothing merged.
        assert!(rows.iter().any(|r| r.id == "model-gemini-3.5-flash-lite" && r.remaining_pct == Some(100.0)));
        assert!(rows.iter().any(|r| r.id == "model-gemini-2.5-pro" && r.remaining_pct == Some(50.0)));
        // Array shape works too.
        let arr = serde_json::json!({"models": [{"name": "gemini-x", "displayName": "Gemini X", "quotaInfo": {"remainingFraction": 0.5}}]});
        let arr_rows = antigravity_model_rows(&arr, now);
        assert_eq!(arr_rows.len(), 1);
        assert_eq!(arr_rows[0].label, "Gemini X");
        // No models section, no rows — never fabricated.
        assert!(antigravity_model_rows(&serde_json::json!({}), now).is_empty());
    }

    #[test]
    fn opencode_plan_from_status() {        assert_eq!(opencode_plan_for_status(200), Some("Go"));
        assert_eq!(opencode_plan_for_status(403), Some("Free"));
        assert_eq!(opencode_plan_for_status(401), None);
        assert_eq!(opencode_plan_for_status(500), None);
    }

    #[test]
    fn opencode_usage_maps_real_windows() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-27T12:00:00Z").unwrap().timestamp_millis();
        let rows = map_opencode_usage(&opencode_body(), now);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].id, "opencode-rolling");
        assert_eq!(rows[0].label, "5-hour limit");
        assert_eq!(rows[0].remaining_pct, Some(87.0));
        assert!(!rows[0].urgent);
        assert_eq!(rows[1].label, "Weekly limit");
        assert_eq!(rows[1].remaining_pct, Some(0.0));
        assert!(rows[1].urgent);
        assert_eq!(rows[2].label, "Monthly limit");
        assert_eq!(rows[2].remaining_pct, Some(57.5));
        // Reset text is local-timezone dependent; the relative text is not.
        assert!(!rows[0].reset_text.is_empty());
        assert_eq!(rows[0].reset_in_text, "in 14 hours");
    }

    #[test]
    fn opencode_usage_never_fabricates() {
        let now = now_ms();
        assert!(map_opencode_usage(&serde_json::json!({}), now).is_empty());
        assert!(map_opencode_usage(&serde_json::json!({"usage": {"rolling": {"status": "ok"}}}), now).is_empty());
        assert!(map_opencode_usage(&serde_json::json!({"usage": {"weekly": {"status": "ok", "percent": "lots"}}}), now).is_empty());
    }

    #[test]
    fn weekly_window_fallback_shapes() {
        let now = now_ms();
        // Classic used/cap under an alternative nesting.
        let alt = serde_json::json!({"used": 3.0, "cap": 30.0, "resetAt": now + 86_400_000});
        let row = limit_window_row("window-weekly", "Weekly", &alt, now).expect("alt weekly shape");
        assert_eq!(row.id, "window-weekly");
        assert_eq!(row.label, "Weekly");
        assert_eq!(row.remaining_pct, Some(90.0));
        // Unambiguous direct-remaining shape wins over used/cap.
        let direct = serde_json::json!({"remainingPercent": 42.0, "used": 999.0, "cap": 1000.0});
        let row = limit_window_row("window-weekly", "Weekly", &direct, now).expect("direct shape");
        assert_eq!(row.remaining_pct, Some(42.0));
        // Remaining + cap pair.
        let pair = serde_json::json!({"remaining": 7.5, "limit": 30.0});
        let row = limit_window_row("window-weekly", "Weekly", &pair, now).expect("pair shape");
        assert_eq!(row.remaining_pct, Some(25.0));
        // Nothing usable -> no row, never fabricated.
        assert!(limit_window_row("window-weekly", "Weekly", &serde_json::json!({"cap": 30.0}), now).is_none());
        assert!(limit_window_row("window-weekly", "Weekly", &serde_json::json!({"used": "lots", "cap": 30.0}), now).is_none());
        assert!(limit_window_row("window-weekly", "Weekly", &serde_json::json!({}), now).is_none());
    }

    #[test]
    fn weekly_spend_from_summary_shapes() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-27T12:00:00Z").unwrap().timestamp_millis();
        // Explicit weekly key.
        assert_eq!(weekly_spend(Some(&serde_json::json!({"weeklyCost": 12.5})), now), Some(12.5));
        // Daily breakdown: only entries dated within the last 7 days count.
        let summary = serde_json::json!({"daily": [
            {"date": "2026-09-26", "cost": 4.0},
            {"date": "2026-09-21", "cost": 6.0},
            {"date": "2026-09-01", "cost": 100.0},
            {"date": "not-a-date", "cost": 50.0},
        ]});
        assert_eq!(weekly_spend(Some(&summary), now), Some(10.0));
        // No weekly signal at all -> None (caller shows no row).
        assert_eq!(weekly_spend(Some(&serde_json::json!({"totalCost": 99.0})), now), None);
        assert_eq!(weekly_spend(Some(&serde_json::json!({})), now), None);
        assert_eq!(weekly_spend(None, now), None);
    }
}
