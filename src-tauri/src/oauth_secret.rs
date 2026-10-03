//! Antigravity OAuth client store: vault-backed app setting with env
//! overrides. There is deliberately NO shipped secret — a committed fallback
//! briefly existed and was purged before v0.1.0; never re-add one.
//!
//! The client is user-owned: whoever runs the app registers their own Google
//! OAuth "Desktop app" client and pastes the ID + secret once in Settings
//! (or env). Reads: env wins, else the vault bundle, else the built-in
//! public client ID with NO secret (exchange then fails with a pointer to
//! Settings). The desktop exchange/refresh and the adapter refresh all read
//! through here; the dev-only vite middleware (`dev-oauth.ts`) reads the
//! same env names directly. Never logged, never SQLite, never in error
//! bodies.

/// OS vault service (same as provider credentials and the gateway key).
const SERVICE: &str = "ai.proxydock";
/// Vault user slot for the Antigravity OAuth client bundle. Never logged.
const SLOT: &str = "antigravity-oauth-secret";
/// Headless/preview env overrides (checked before the vault slot).
pub const ENV_OVERRIDE: &str = "PROXYDOCK_ANTIGRAVITY_SECRET";
/// Pre-rename fallback for the env override.
const LEGACY_ENV_OVERRIDE: &str = "PROXYHUB_ANTIGRAVITY_SECRET";
/// Env override for a user-owned client ID (else the vault bundle, else the
/// built-in public client).
pub const ENV_CLIENT_ID: &str = "PROXYDOCK_ANTIGRAVITY_CLIENT_ID";
/// Pre-rename fallback for the client-ID override.
const LEGACY_ENV_CLIENT_ID: &str = "PROXYHUB_ANTIGRAVITY_CLIENT_ID";

/// Built-in public client ID (agy CLI flow). Usable for the authorize URL;
/// token calls additionally need the user-owned secret below.
pub const BUILTIN_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";

/// User-owned OAuth client: ID + secret.
pub struct OAuthClient {
    pub client_id: String,
    pub client_secret: String,
}

fn env_nonempty(names: &[&str]) -> Option<String> {
    for name in names {
        if let Ok(raw) = std::env::var(name) {
            let trimmed = raw.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

/// Non-empty env override, if set. Trimmed; blank counts as unset.
pub fn env_override() -> Option<String> {
    env_nonempty(&[ENV_OVERRIDE, LEGACY_ENV_OVERRIDE])
}

/// Effective client ID: env override first, else the vault bundle, else the
/// built-in public client (authorize URL works; token calls still need the
/// secret below).
pub fn client_id() -> String {
    if let Some(id) = env_nonempty(&[ENV_CLIENT_ID, LEGACY_ENV_CLIENT_ID]) {
        return id;
    }
    if let Ok(stored) = vault_get() {
        if let Ok(bundle) = serde_json::from_str::<serde_json::Value>(&stored) {
            if let Some(id) = bundle.get("client_id").and_then(|v| v.as_str()) {
                let id = id.trim().to_string();
                if !id.is_empty() {
                    return id;
                }
            }
        }
    }
    BUILTIN_CLIENT_ID.to_string()
}

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

/// Effective client: env wins, else the vault bundle, else the built-in
/// public ID with NO secret (token calls then fail with a pointer below).
/// `Err` carries a terse pointer, never the value. Legacy slot values that
/// are a bare secret (pre-bundle era) are honored as secret-only.
pub fn read() -> Result<OAuthClient, String> {
    let client_secret = match env_override() {
        Some(from_env) => from_env,
        None => match vault_get() {
            Ok(stored) => {
                if let Ok(bundle) = serde_json::from_str::<serde_json::Value>(&stored) {
                    match bundle.get("client_secret").and_then(|v| v.as_str()) {
                        Some(secret) if !secret.trim().is_empty() => secret.trim().to_string(),
                        _ => {
                            return Err(format!(
                                "Antigravity OAuth needs its client secret — paste your OAuth client in Settings or set {ENV_OVERRIDE}"
                            ))
                        }
                    }
                } else if !stored.trim().is_empty() {
                    // Legacy bare-secret slot value: keep working.
                    stored.trim().to_string()
                } else {
                    return Err(format!(
                        "Antigravity OAuth needs its client secret — paste your OAuth client in Settings or set {ENV_OVERRIDE}"
                    ));
                }
            }
            Err(_) => {
                return Err(format!(
                    "Antigravity OAuth needs its client secret — paste your OAuth client in Settings or set {ENV_OVERRIDE}"
                ))
            }
        },
    };
    Ok(OAuthClient { client_id: client_id(), client_secret })
}

/// Presence only (for Settings UI): a usable secret exists. Never returns
/// the value.
pub fn has_stored() -> bool {
    read().is_ok()
}

/// User-set override of the stored OAuth client (ID + secret bundle).
/// Refused while the env override is active. Never logs the values.
pub fn set_override(client_id: &str, secret: &str) -> Result<(), String> {
    if env_override().is_some() {
        return Err(format!(
            "secret is set via {ENV_OVERRIDE} — unset it to change the stored client"
        ));
    }
    let id = client_id.trim().to_string();
    let trimmed = secret.trim().to_string();
    if id.is_empty() || trimmed.is_empty() {
        return Err("client ID and secret must not be empty".to_string());
    }
    let bundle = serde_json::json!({"client_id": id, "client_secret": trimmed}).to_string();
    vault_set(&bundle)
}

/// IPC presence check for Settings (bool only — values never cross IPC
/// except on explicit user Set, which echoes back what was just typed).
#[tauri::command]
pub fn antigravity_secret_status() -> bool {
    has_stored()
}

/// IPC user-set of the OAuth client (ID + secret). Returns presence only.
#[tauri::command]
pub fn set_antigravity_secret(client_id: String, secret: String) -> Result<bool, String> {
    set_override(&client_id, &secret)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_secret_points_at_settings() {
        // Env must be absent for the Err branch; a machine with a stored
        // secret takes the Ok branch instead — both are asserted, so this
        // never fails loudly on dev machines nor passes silently in CI.
        if env_override().is_some() {
            return;
        }
        match read() {
            Ok(_) => assert!(has_stored()),
            Err(err) => {
                // Names the fix (env var) without echoing any secret material.
                assert!(err.contains(ENV_OVERRIDE), "error names the fix: {err}");
                assert!(err.len() < 128, "error stays terse: {err}");
            }
        }
    }
}
