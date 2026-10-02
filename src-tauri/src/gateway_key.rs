//! Unified Proxy Dock key: the single secret that is both the app password
//! and the gateway API bearer.
//!
//! Threat model: loopback-only single-user desktop app. The key defends
//! against malicious browser pages (CSRF/DNS-rebinding hitting 127.0.0.1),
//! NOT against local malware — same-user processes can always read the vault
//! or the env override, so never claim otherwise. No multi-user, no remote
//! bind (that would be a re-scope).
//!
//! Storage: a NEW vault slot (`gateway-key`) under the same `ai.proxydock`
//! service as the provider credentials (no legacy slot — this key never
//! existed under the old name). Headless/preview runs may override via
//! `PROXYDOCK_GATEWAY_KEY` (legacy `PROXYHUB_GATEWAY_KEY` fallback), which
//! wins over the vault slot and disables vault writes. Never logged, never
//! SQLite, never in error bodies. The SPA reads it via Tauri IPC only
//! (`get_gateway_key`) — never over HTTP.

use axum::http::HeaderMap;
use base64::Engine as _;
use serde::Serialize;
use sha2::{Digest, Sha256};

/// OS vault service (same as provider credentials; this key has no legacy).
const SERVICE: &str = "ai.proxydock";
/// Vault user slot for the unified key. Never logged.
const SLOT: &str = "gateway-key";
/// Headless/preview env override (checked before the vault slot).
pub const ENV_OVERRIDE: &str = "PROXYDOCK_GATEWAY_KEY";
/// Pre-rename fallback for the env override (same pattern as PORT/DATA_DIR).
const LEGACY_ENV_OVERRIDE: &str = "PROXYHUB_GATEWAY_KEY";

/// Raw vault read for the gateway-key slot (no legacy fallback — new slot).
fn vault_get() -> Result<String, String> {
    #[cfg(windows)]
    return crate::wincred::get(SERVICE, SLOT);
    #[cfg(not(windows))]
    {
        keyring::Entry::new(SERVICE, SLOT)
            .map_err(|err| err.to_string())?
            .get_password()
            .map_err(|err| err.to_string())
    }
}

/// Vault write for the gateway-key slot, verified by read-back.
fn vault_set(secret: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        crate::wincred::set(SERVICE, SLOT, secret)?;
    }
    #[cfg(not(windows))]
    {
        keyring::Entry::new(SERVICE, SLOT)
            .map_err(|err| err.to_string())?
            .set_password(secret)
            .map_err(|err| err.to_string())?;
    }
    match vault_get() {
        Ok(back) if back == secret => Ok(()),
        Ok(_) => Err("secret store read-back mismatch".to_string()),
        Err(err) => Err(format!("secret store unavailable after write: {err}")),
    }
}

/// Non-empty env override, if set. Trimmed; blank counts as unset.
pub fn env_override() -> Option<String> {
    for name in [ENV_OVERRIDE, LEGACY_ENV_OVERRIDE] {
        if let Ok(raw) = std::env::var(name) {
            let trimmed = raw.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

/// Effective key: env override first, else the vault slot. `None` when
/// neither holds a key (fresh machine before first `ensure`).
pub fn read_effective() -> Option<String> {
    if let Some(from_env) = env_override() {
        return Some(from_env);
    }
    vault_get().ok().filter(|s| !s.trim().is_empty())
}

/// Install default key (user-directed): every fresh install starts with this
/// well-known value until the user sets or rotates it in Settings. This is
/// safe under the loopback-only threat model — the key stops malicious
/// browser pages, not local users — and it means external clients work out
/// of the box with `Authorization: Bearer proxy-dock`.
pub const DEFAULT_KEY: &str = "proxy-dock";

/// 32 random bytes as base64url (no pad) — same shape as the OAuth states.
/// Used by rotate (never for the install default).
fn generate() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Load the key, storing the install default on first run. Fails only when
/// the env override is absent AND the vault is unreadable/unwritable.
pub fn ensure() -> Result<String, String> {
    if let Some(key) = read_effective() {
        return Ok(key);
    }
    vault_set(DEFAULT_KEY)?;
    Ok(DEFAULT_KEY.to_string())
}

/// User-set override of the stored key. Refused while the env override is
/// active (the vault value would be dead config). Never logs the value.
pub fn set_override(secret: &str) -> Result<(), String> {
    if env_override().is_some() {
        return Err(format!("key is set via {ENV_OVERRIDE} — unset it to change the stored key"));
    }
    let trimmed = secret.trim().to_string();
    if trimmed.is_empty() {
        return Err("key must not be empty".to_string());
    }
    vault_set(&trimmed)
}

/// Rotate: fresh random key into the vault slot. Refused while the env
/// override is active. Returns the new key for the IPC caller to display.
/// Never logged.
pub fn rotate() -> Result<String, String> {
    if env_override().is_some() {
        return Err(format!("key is set via {ENV_OVERRIDE} — unset it to rotate the stored key"));
    }
    let fresh = generate();
    vault_set(&fresh)?;
    Ok(fresh)
}

/// Fixed-length, no-early-exit equality over the SHA-256 of both inputs.
/// `subtle` is not a dependency, so the hash-then-accumulate shape removes
/// the two oracles a naive compare leaks: length (digests are always 32
/// bytes) and prefix position (every byte pair always runs).
pub fn timing_safe_eq(a: &str, b: &str) -> bool {
    let ha = Sha256::digest(a.as_bytes());
    let hb = Sha256::digest(b.as_bytes());
    let mut diff = 0u8;
    for i in 0..ha.len() {
        diff |= ha[i] ^ hb[i];
    }
    diff == 0
}

/// `true` when `headers` carries `Authorization: Bearer <expected>`.
/// `None` (no key configured) never authorizes — fail closed.
pub fn bearer_authorized(headers: &HeaderMap, expected: Option<&str>) -> bool {
    let Some(want) = expected.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let Some(raw) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    // Scheme match is case-insensitive (RFC 9110); the token itself is exact.
    if raw.len() <= 7 || !raw[..7].eq_ignore_ascii_case("bearer ") {
        return false;
    }
    let presented = raw[7..].trim();
    if presented.is_empty() || presented.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control()) {
        return false;
    }
    timing_safe_eq(presented, want)
}

#[derive(Debug, Clone, Serialize)]
pub struct GatewayKeyInfo {
    /// The key itself. IPC-only surface — never served over HTTP, never logged.
    pub key: String,
    /// `true` when the value comes from the env override (vault writes off).
    pub from_env: bool,
}

/// IPC read for the SPA (memory-only on the frontend, never localStorage).
/// Stores the install default on first run so desktop always has a key.
#[tauri::command]
pub fn get_gateway_key() -> Result<GatewayKeyInfo, String> {
    let from_env = env_override().is_some();
    let key = ensure()?;
    Ok(GatewayKeyInfo { key, from_env })
}

/// IPC user-set override of the stored key.
#[tauri::command]
pub fn set_gateway_key(key: String) -> Result<GatewayKeyInfo, String> {
    set_override(&key)?;
    Ok(GatewayKeyInfo { key: key.trim().to_string(), from_env: false })
}

/// IPC rotate: fresh random key. Existing external client configs
/// (hermes/opencode `base_url` setups) break until they paste the new key.
#[tauri::command]
pub fn rotate_gateway_key() -> Result<GatewayKeyInfo, String> {
    let key = rotate()?;
    Ok(GatewayKeyInfo { key, from_env: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::AUTHORIZATION, value.parse().expect("header value"));
        headers
    }

    #[test]
    fn generated_key_is_32_bytes_base64url() {
        let key = generate();
        assert_eq!(key.len(), 43, "32 bytes encode to 43 base64url chars");
        assert!(key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_ne!(generate(), key);
    }

    #[test]
    fn install_default_is_documented() {
        assert_eq!(DEFAULT_KEY, "proxy-dock");
    }

    #[test]
    fn timing_compare_covers_lengths_and_values() {
        assert!(timing_safe_eq("abc", "abc"));
        assert!(!timing_safe_eq("abc", "abd"));
        assert!(!timing_safe_eq("abc", "abcd"));
        assert!(timing_safe_eq("", "")); // equal inputs agree; callers reject empty keys first
        assert!(!timing_safe_eq("a", ""));
    }

    #[test]
    fn bearer_check_fails_closed_without_key() {
        assert!(!bearer_authorized(&header("Bearer anything"), None));
        assert!(!bearer_authorized(&header("Bearer anything"), Some("")));
        assert!(!bearer_authorized(&header("Bearer anything"), Some("   ")));
    }

    #[test]
    fn bearer_check_accepts_exact_key() {
        assert!(bearer_authorized(&header("Bearer s3cret"), Some("s3cret")));
        // Scheme case is protocol syntax, not secret: accept it.
        assert!(bearer_authorized(&header("bearer s3cret"), Some("s3cret")));
        assert!(bearer_authorized(&header("BEARER s3cret"), Some("s3cret")));
    }

    #[test]
    fn bearer_check_rejects_wrong_or_malformed() {
        assert!(!bearer_authorized(&header("Bearer wrong"), Some("s3cret")));
        assert!(!bearer_authorized(&HeaderMap::new(), Some("s3cret")));
        assert!(!bearer_authorized(&header("Basic c2VjcmV0"), Some("s3cret")));
        assert!(!bearer_authorized(&header("Bearer "), Some("s3cret")));
        assert!(!bearer_authorized(&header("Bearer s3 cret"), Some("s3 cret")));
        assert!(!bearer_authorized(&header("Bearer s3cret "), Some("nope")));
    }
}
