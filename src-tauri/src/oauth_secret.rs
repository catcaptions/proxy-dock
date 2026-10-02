//! Antigravity OAuth client secret store: vault-backed app setting with an
//! env override. There is deliberately NO shipped default — a committed
//! fallback briefly existed and was purged before v0.1.0; never re-add one.
//!
//! Reads: env `PROXYDOCK_ANTIGRAVITY_SECRET` (legacy
//! `PROXYHUB_ANTIGRAVITY_SECRET` fallback) wins, else the vault slot.
//! The desktop exchange/refresh and the adapter refresh all read through
//! here; the dev-only vite middleware (`dev-oauth.ts`) reads the same env
//! names directly. Never logged, never SQLite, never in error bodies.

/// OS vault service (same as provider credentials and the gateway key).
const SERVICE: &str = "ai.proxydock";
/// Vault user slot for the Antigravity OAuth client secret. Never logged.
const SLOT: &str = "antigravity-oauth-secret";
/// Headless/preview env override (checked before the vault slot).
pub const ENV_OVERRIDE: &str = "PROXYDOCK_ANTIGRAVITY_SECRET";
/// Pre-rename fallback for the env override.
const LEGACY_ENV_OVERRIDE: &str = "PROXYHUB_ANTIGRAVITY_SECRET";

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

/// Effective secret: env override first, else the vault slot. `Err` carries a
/// terse pointer, never the value.
pub fn read() -> Result<String, String> {
    if let Some(from_env) = env_override() {
        return Ok(from_env);
    }
    match vault_get() {
        Ok(secret) if !secret.trim().is_empty() => Ok(secret),
        _ => Err(format!(
            "Antigravity OAuth needs a client secret — paste it in Settings or set {ENV_OVERRIDE}"
        )),
    }
}

/// Presence only (for Settings UI). Never returns the value.
pub fn has_stored() -> bool {
    env_override().is_some() || vault_get().is_ok_and(|s| !s.trim().is_empty())
}

/// User-set override of the stored secret. Refused while the env override is
/// active. Never logs the value.
pub fn set_override(secret: &str) -> Result<(), String> {
    if env_override().is_some() {
        return Err(format!(
            "secret is set via {ENV_OVERRIDE} — unset it to change the stored secret"
        ));
    }
    let trimmed = secret.trim().to_string();
    if trimmed.is_empty() {
        return Err("secret must not be empty".to_string());
    }
    vault_set(&trimmed)
}

/// IPC presence check for Settings (bool only — the value never crosses IPC
/// except on explicit user Set, which echoes back what was just typed).
#[tauri::command]
pub fn antigravity_secret_status() -> bool {
    has_stored()
}

/// IPC user-set of the secret. Returns presence only.
#[tauri::command]
pub fn set_antigravity_secret(secret: String) -> Result<bool, String> {
    set_override(&secret)?;
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
