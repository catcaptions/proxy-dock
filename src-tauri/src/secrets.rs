#[cfg(not(windows))]
use keyring::Entry;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::adapters::{AntigravityAdapter, ChatGptAdapter, ClaudeAdapter, CommandCodeAdapter, OpenCodeAdapter, ProviderAdapter, TokenPresence};

/// OS vault service. Renamed `ai.proxyhub` -> `ai.proxydock` with the
/// Proxy Dock rename; reads fall back to the old service so existing
/// credentials survive, and migrate forward on success.
const SERVICE: &str = "ai.proxydock";
const LEGACY_SERVICE: &str = "ai.proxyhub";

/// Vault user slot for an account. Never logged; only presence/suffix leave.
fn slot(provider_slug: &str, account_id: &str) -> String {
    format!("local-token:{provider_slug}:{account_id}")
}

#[cfg(not(windows))]
fn entry(service: &str, provider_slug: &str, account_id: &str) -> Result<Entry, String> {
    Entry::new(service, &slot(provider_slug, account_id)).map_err(|err| err.to_string())
}

/// Service-scoped vault read (no fallback).
fn platform_get_from(service: &str, provider_slug: &str, account_id: &str) -> Result<String, String> {
    #[cfg(windows)]
    return crate::wincred::get(service, &slot(provider_slug, account_id));
    #[cfg(not(windows))]
    return entry(service, provider_slug, account_id).and_then(|e| e.get_password().map_err(|err| err.to_string()));
}

/// Raw vault read. Falls back to the legacy `ai.proxyhub` service so
/// credentials stored before the Proxy Dock rename keep working, and
/// migrates them forward on success. Never logs or exposes the value
/// beyond the local process.
fn platform_get(provider_slug: &str, account_id: &str) -> Result<String, String> {
    match platform_get_from(SERVICE, provider_slug, account_id) {
        ok @ Ok(_) => ok,
        Err(_) => {
            let legacy = platform_get_from(LEGACY_SERVICE, provider_slug, account_id)?;
            // Best-effort migrate forward; ignore failures.
            let _ = platform_set_to(SERVICE, provider_slug, account_id, &legacy);
            Ok(legacy)
        }
    }
}

/// Service-scoped vault write, verified by read-back so a broken store
/// fails loudly instead of pretending the credential was saved.
fn platform_set_to(service: &str, provider_slug: &str, account_id: &str, secret: &str) -> Result<(), String> {
    #[cfg(windows)]
    crate::wincred::set(service, &slot(provider_slug, account_id), secret)?;
    #[cfg(not(windows))]
    entry(service, provider_slug, account_id)?.set_password(secret).map_err(|err| err.to_string())?;
    match platform_get_from(service, provider_slug, account_id) {
        Ok(back) if back == secret => Ok(()),
        Ok(_) => Err("secret store read-back mismatch".to_string()),
        Err(err) => Err(format!("secret store unavailable after write: {err}")),
    }
}

/// Raw vault write. Targets the new service, then best-effort clears any
/// legacy duplicate so the two slots cannot diverge.
fn platform_set(provider_slug: &str, account_id: &str, secret: &str) -> Result<(), String> {
    platform_set_to(SERVICE, provider_slug, account_id, secret)?;
    platform_delete_from(LEGACY_SERVICE, provider_slug, account_id);
    Ok(())
}

fn platform_delete_from(service: &str, provider_slug: &str, account_id: &str) {
    #[cfg(windows)]
    let _ = crate::wincred::delete(service, &slot(provider_slug, account_id));
    #[cfg(not(windows))]
    let _ = entry(service, provider_slug, account_id).and_then(|e| {
        e.delete_credential().map_err(|err| err.to_string())
    });
}

fn platform_delete(provider_slug: &str, account_id: &str) {
    platform_delete_from(SERVICE, provider_slug, account_id);
    platform_delete_from(LEGACY_SERVICE, provider_slug, account_id);
}

/// Stable account slot for a credential. Email (lowercased) when the provider
/// verified one, else `key-<12 hex>` of the secret's sha256. Same identity
/// always maps to the same slot, so re-signing updates in place and can never
/// duplicate or clobber a different account. The frontend mirrors this rule
/// in `deriveAccountId` — keep the two in sync.
/// Stable slot rule shared with the HTTP credential endpoint (identical to
/// the frontend `deriveAccountId`): verified email (lowercased) or
/// `key-<12 hex of sha256(secret)>`.
pub(crate) fn account_id_for(email: &str, secret: &str) -> String {
    let email = email.trim().to_lowercase();
    if !email.is_empty() {
        return email;
    }
    let mut hasher = Sha256::new();
    hasher.update(secret.trim().as_bytes());
    let digest = hasher.finalize();
    format!("key-{}", digest[..6].iter().map(|b| format!("{b:02x}")).collect::<String>())
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenStatus {
    NoToken,
    TokenPresent,
}

impl From<TokenPresence> for TokenStatus {
    fn from(presence: TokenPresence) -> Self {
        match presence {
            TokenPresence::NoToken => TokenStatus::NoToken,
            TokenPresence::TokenPresent => TokenStatus::TokenPresent,
        }
    }
}

fn adapter_for(slug: &str) -> Option<Box<dyn ProviderAdapter>> {
    match slug {
        "commandcode" => Some(Box::new(CommandCodeAdapter)),
        "opencode" => Some(Box::new(OpenCodeAdapter)),
        "chatgpt" => Some(Box::new(ChatGptAdapter)),
        "antigravity" => Some(Box::new(AntigravityAdapter)),
        "claude" => Some(Box::new(ClaudeAdapter)),
        _ => None,
    }
}

pub fn stored_presence(provider_slug: &str, account_id: &str) -> TokenStatus {
    match platform_get(provider_slug, account_id) {
        Ok(secret) if !secret.trim().is_empty() => TokenStatus::TokenPresent,
        _ => adapter_for(provider_slug)
            .map(|a| TokenStatus::from(a.local_token_presence()))
            .unwrap_or(TokenStatus::NoToken),
    }
}

/// Async vault presence check off the hot path: the blocking keyring read
/// runs in `spawn_blocking` so chat streams never stall the runtime.
/// Never logs or exposes the secret itself.
pub async fn has_local_token_async(provider_slug: String, account_id: String) -> bool {
    tokio::task::spawn_blocking(move || has_local_token(&provider_slug, &account_id))
        .await
        .unwrap_or(false)
}

/// Async raw vault read off the hot path (same `spawn_blocking` rule).
/// Callers hold the returned secret only for the upstream send.
pub async fn read_raw_token_async(provider_slug: String, account_id: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || read_raw_token(&provider_slug, &account_id))
        .await
        .unwrap_or(Err("secret store task failed".to_string()))
}

pub fn has_local_token(provider_slug: &str, account_id: &str) -> bool {
    matches!(stored_presence(provider_slug, account_id), TokenStatus::TokenPresent)
}

/// Raw vault read, for modules (quota) that need the secret itself.
/// Never logs or exposes the value beyond the local process.
pub fn read_raw_token(provider_slug: &str, account_id: &str) -> Result<String, String> {
    platform_get(provider_slug, account_id)
}

/// Raw vault write, for modules (quota) persisting refreshed credentials.
pub fn store_raw_token(provider_slug: &str, account_id: &str, secret: &str) -> Result<(), String> {
    platform_set(provider_slug, account_id, secret)
}

#[tauri::command]
pub fn local_token_status(provider_slug: String, account_id: String) -> TokenStatus {
    stored_presence(&provider_slug, &account_id)
}

#[tauri::command]
pub fn set_local_token(provider_slug: String, account_id: String, token: String) -> Result<TokenStatus, String> {
    let secret = token.trim().to_string();
    if secret.is_empty() {
        return Err("token must not be empty".to_string());
    }
    platform_set(&provider_slug, &account_id, &secret)?;
    Ok(TokenStatus::TokenPresent)
}

#[tauri::command]
pub fn clear_local_token(provider_slug: String, account_id: String) -> Result<TokenStatus, String> {
    platform_delete(&provider_slug, &account_id);
    Ok(TokenStatus::NoToken)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalAccountInfo {
    pub provider: String,
    pub account: String,
    /// Account email when the stored credential carries one (OAuth flows).
    /// OpenCode Go keys do not expose an email via the provider API, so this
    /// is None for raw API keys.
    pub email: Option<String>,
    /// Last 3 chars of the stored secret for display (e.g. `•••xyz`).
    /// The full secret is never returned.
    pub key_suffix: String,
    pub status: TokenStatus,
}

fn suffix_of(secret: &str) -> String {
    let trimmed = secret.trim();
    if trimmed.len() <= 3 {
        return "•••".to_string();
    }
    trimmed[trimmed.len() - 3..].to_string()
}

#[tauri::command]
pub fn local_account_info(provider_slug: String, account_id: String) -> LocalAccountInfo {
    let (email, key_suffix, status) = match platform_get(&provider_slug, &account_id)
    {
        Ok(secret) if !secret.trim().is_empty() => {
            // OAuth flows store JSON StoredCredential; raw keys store the key itself.
            let email = serde_json::from_str::<crate::oauth::StoredCredential>(&secret)
                .ok()
                .map(|cred| cred.email)
                .filter(|email| !email.trim().is_empty());
            let raw_key = serde_json::from_str::<crate::oauth::StoredCredential>(&secret)
                .ok()
                .map(|cred| cred.access_token)
                .unwrap_or(secret);
            (email, suffix_of(&raw_key), TokenStatus::TokenPresent)
        }
        _ => (None, String::new(), TokenStatus::NoToken),
    };
    LocalAccountInfo { provider: provider_slug, account: account_id, email, key_suffix: key_suffix, status }
}

#[tauri::command]
pub async fn start_codex_sign_in(
    app: tauri::AppHandle<tauri::Wry>,
    pool: tauri::State<'_, SqlitePool>,
) -> Result<SignInStarted, String> {
    let pkce = crate::oauth::pkce();
    let state = crate::oauth::random_state();
    let (server, receiver) = crate::oauth::CallbackServer::new(
        crate::oauth::CODEX_CALLBACK_PORT,
        crate::oauth::CODEX_CALLBACK_PATH,
        state.clone(),
    );
    let port = server.run().await?;
    open_browser(&app, &crate::oauth::codex_auth_url(&state, &pkce, port))?;
    let outcome = wait_for_callback(receiver, "chatgpt", "default", &state).await;
    let result = outcome?;
    let credential = crate::oauth::exchange_codex(&result.code, &pkce.verifier, port).await?;
    let account = account_id_for(&credential.email, &credential.access_token);
    store_credential("chatgpt", &account, &credential)?;
    // Best-effort routing registration (never fails the sign-in itself).
    let _ = crate::db::register_account(&pool, "chatgpt", &account, &credential.email, None).await;
    Ok(SignInStarted { url: codex_callback_hint(port), provider: "chatgpt".to_string(), account, email: credential.email.clone() })
}

#[tauri::command]
pub async fn start_antigravity_sign_in(
    app: tauri::AppHandle<tauri::Wry>,
    pool: tauri::State<'_, SqlitePool>,
) -> Result<SignInStarted, String> {
    let state = crate::oauth::random_state();
    let (server, receiver) = crate::oauth::CallbackServer::new(
        crate::oauth::ANTIGRAVITY_CALLBACK_PORT,
        crate::oauth::ANTIGRAVITY_CALLBACK_PATH,
        state.clone(),
    );
    let port = server.run().await?;
    open_browser(&app, &crate::oauth::antigravity_auth_url(&state, port))?;
    let outcome = wait_for_callback(receiver, "antigravity", "default", &state).await;
    let result = outcome?;
    let credential = crate::oauth::exchange_antigravity(&result.code, port).await?;
    let account = account_id_for(&credential.email, &credential.access_token);
    store_credential("antigravity", &account, &credential)?;
    let _ = crate::db::register_account(&pool, "antigravity", &account, &credential.email, None).await;
    Ok(SignInStarted { url: antigravity_callback_hint(port), provider: "antigravity".to_string(), account, email: credential.email.clone() })
}

#[tauri::command]
pub async fn start_claude_sign_in(
    app: tauri::AppHandle<tauri::Wry>,
    pool: tauri::State<'_, SqlitePool>,
) -> Result<SignInStarted, String> {
    // Claude subscription OAuth via claude.ai (PKCE, localhost :54545), the
    // same flow as `cli-proxy-api --claude-login` / `claude setup-token`.
    let pkce = crate::oauth::pkce();
    let state = crate::oauth::random_state();
    let (server, receiver) = crate::oauth::CallbackServer::new(
        crate::oauth::CLAUDE_CALLBACK_PORT,
        crate::oauth::CLAUDE_CALLBACK_PATH,
        state.clone(),
    );
    let port = server.run().await?;
    open_browser(&app, &crate::oauth::claude_auth_url(&state, &pkce, port))?;
    let outcome = wait_for_callback(receiver, "claude", "default", &state).await;
    let result = outcome?;
    let credential = crate::oauth::exchange_claude(&result.code, &pkce.verifier, &state, port).await?;
    let account = account_id_for(&credential.email, &credential.access_token);
    store_credential("claude", &account, &credential)?;
    // Best-effort routing registration (never fails the sign-in itself).
    let _ = crate::db::register_account(&pool, "claude", &account, &credential.email, None).await;
    Ok(SignInStarted { url: claude_callback_hint(port), provider: "claude".to_string(), account, email: credential.email.clone() })
}

#[tauri::command]
pub async fn start_commandcode_sign_in(
    app: tauri::AppHandle<tauri::Wry>,
    pool: tauri::State<'_, SqlitePool>,
) -> Result<SignInStarted, String> {
    // Web CLI-login via the Studio auth page: Studio POSTs the issued API key
    // to our localhost callback after the user signs in (or shows a "Copy
    // your API key" fallback the user can paste instead).
    let state = crate::oauth::commandcode_state();
    let (callback_url, receiver) = crate::oauth::run_commandcode_callback(state.clone()).await?;
    open_browser(&app, &crate::oauth::commandcode_auth_url(&callback_url, &state))?;
    let payload = match tokio::time::timeout(std::time::Duration::from_secs(300), receiver).await {
        Ok(Ok(Ok(payload))) => payload,
        Ok(Ok(Err(err))) => return Err(format!("sign-in failed: {err}")),
        Ok(Err(_)) => return Err("sign-in was cancelled".to_string()),
        Err(_) => {
            return Err(
                "no callback from Studio after 5 minutes — use the \"Copy your API key\" fallback on the Studio page and paste the key below".to_string(),
            )
        }
    };
    if payload.state.as_deref().unwrap_or_default() != state {
        return Err("state mismatch — possible CSRF; aborting sign-in".to_string());
    }
    let api_key = payload.api_key.unwrap_or_default().trim().to_string();
    if api_key.is_empty() {
        return Err("Studio callback did not include an API key".to_string());
    }
    let identity = crate::oauth::verify_commandcode_key(&api_key).await?;
    let account = account_id_for(&identity.email, &api_key);
    platform_set("commandcode", &account, &api_key)?;
    let email = if identity.email.is_empty() { identity.user_name.clone() } else { identity.email.clone() };
    let _ = crate::db::register_account(&pool, "commandcode", &account, &email, None).await;
    Ok(SignInStarted { url: callback_url, provider: "commandcode".to_string(), account, email })
}

#[tauri::command]
pub async fn verify_commandcode_key(pool: tauri::State<'_, SqlitePool>, api_key: String) -> Result<CommandCodeVerified, String> {
    let secret = api_key.trim().to_string();
    if secret.len() < 8 {
        return Err("that key looks too short — paste the full API key".to_string());
    }
    let identity = crate::oauth::verify_commandcode_key(&secret).await?;
    let account = account_id_for(&identity.email, &secret);
    platform_set("commandcode", &account, &secret)?;
    let _ = crate::db::register_account(&pool, "commandcode", &account, &identity.email, None).await;
    Ok(CommandCodeVerified { provider: "commandcode".to_string(), account, email: identity.email, user_name: identity.user_name })
}

#[tauri::command]
pub async fn verify_opencode_key(pool: tauri::State<'_, SqlitePool>, api_key: String) -> Result<OpencodeVerified, String> {
    let secret = api_key.trim().to_string();
    if secret.len() < 8 {
        return Err("that key looks too short — paste the full Go API key".to_string());
    }
    let status = crate::oauth::verify_opencode_key(&secret).await?;
    let account = account_id_for("", &secret);
    platform_set("opencode", &account, &secret)?;
    // The verified plan ("Go" or "Free") is registered for gateway display.
    let _ = crate::db::register_account(&pool, "opencode", &account, &account, Some(&status.plan)).await;
    Ok(OpencodeVerified { models: status.models, provider: "opencode".to_string(), account, plan: status.plan })
}

#[tauri::command]
pub async fn verify_claude_key(pool: tauri::State<'_, SqlitePool>, api_key: String) -> Result<ClaudeVerified, String> {
    let secret = api_key.trim().to_string();
    if secret.len() < 8 {
        return Err("that key looks too short — paste the full API key".to_string());
    }
    let models = crate::oauth::verify_claude_key(&secret).await?;
    let account = account_id_for("", &secret);
    platform_set("claude", &account, &secret)?;
    // Anthropic exposes no plan name over the API — the account lists with
    // its key slot and an honest empty plan (unknown means unknown).
    let _ = crate::db::register_account(&pool, "claude", &account, &account, None).await;
    Ok(ClaudeVerified { models, provider: "claude".to_string(), account })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SignInStarted {
    pub url: String,
    pub provider: String,
    /// Stable account slot id (email or key-hash) — never display as identity.
    pub account: String,
    /// Display identity (email, or username where the provider gives no email).
    pub email: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OpencodeVerified {
    pub provider: String,
    pub account: String,
    pub models: Vec<String>,
    /// Provider-reported subscription plan ("Go" or "Free").
    pub plan: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CommandCodeVerified {
    pub provider: String,
    pub account: String,
    pub email: String,
    pub user_name: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ClaudeVerified {
    pub provider: String,
    pub account: String,
    pub models: Vec<String>,
}

fn codex_callback_hint(port: u16) -> String {
    format!("http://localhost:{port}{}", crate::oauth::CODEX_CALLBACK_PATH)
}

fn antigravity_callback_hint(port: u16) -> String {
    format!("http://localhost:{port}{}", crate::oauth::ANTIGRAVITY_CALLBACK_PATH)
}

fn claude_callback_hint(port: u16) -> String {
    format!("http://localhost:{port}{}", crate::oauth::CLAUDE_CALLBACK_PATH)
}

/// Opens the authorize URL in the system browser via the opener plugin,
/// which hands the full URL to the OS (ShellExecute on Windows) with no
/// cmd shell in between. The previous `cmd /C start` spawn re-parsed `&`
/// as a command separator even when quoted, truncating every OAuth
/// authorize URL at its first query parameter — rejected by all providers
/// (Google: missing response_type; OpenAI: invalid authorize request;
/// Windows "cannot find" dialog showing the URL as a program name).
/// Errors carry the URL so the sign-in panel can offer a manual open.
fn open_browser(app: &tauri::AppHandle<tauri::Wry>, url: &str) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_url(url, None::<&str>).map_err(|err| {
        format!("could not open the system browser ({err}) — open this URL manually:\n{url}")
    })
}

async fn wait_for_callback(
    receiver: tokio::sync::oneshot::Receiver<Result<crate::oauth::CallbackResult, String>>,
    _provider: &str,
    _account: &str,
    _state: &str,
) -> Result<crate::oauth::CallbackResult, String> {
    match tokio::time::timeout(std::time::Duration::from_secs(300), receiver).await {
        Ok(Ok(Ok(result))) => Ok(result),
        Ok(Ok(Err(err))) => Err(format!("sign-in failed: {err}")),
        Ok(Err(_)) => Err("sign-in was cancelled".to_string()),
        Err(_) => Err("sign-in timed out after 5 minutes — try again".to_string()),
    }
}

fn store_credential(provider_slug: &str, account_id: &str, credential: &crate::oauth::StoredCredential) -> Result<(), String> {
    let payload = serde_json::to_string(credential).map_err(|err| err.to_string())?;
    platform_set(provider_slug, account_id, &payload)?;
    Ok(())
}

pub fn not_configured_response(provider_slug: &str) -> serde_json::Value {
    let message = adapter_for(provider_slug)
        .map(|a| a.not_configured_message())
        .unwrap_or_else(|| format!("unknown provider '{provider_slug}' is not configured"));
    serde_json::json!({
        "error": { "message": message, "type": "proxy_dock_not_configured", "code": 501 }
    })
}

#[cfg(test)]
mod tests {
    /// The opener-plugin path hands the whole URL to the OS with no shell,
    /// so `&` query separators survive (the old `cmd /C start` spawn
    /// truncated every authorize URL at the first `&`). This pins the
    /// contract: the URL handed to the opener must be the complete,
    /// untruncated authorize URL. (Shell behavior itself is covered by the
    /// live sign-in path, not unit-testable headlessly.)
    #[test]
    fn authorize_urls_keep_all_query_params() {
        for url in [
            crate::oauth::codex_auth_url("s", &crate::oauth::pkce(), 1455),
            crate::oauth::antigravity_auth_url("s", 51121),
            crate::oauth::claude_auth_url("s", &crate::oauth::pkce(), 54545),
        ] {
            assert!(url.contains("response_type=code"), "truncated URL: {url}");
            assert!(url.contains("client_id="), "truncated URL: {url}");
            assert!(url.contains("state=s"), "truncated URL: {url}");
        }
    }
}
