//! Antigravity OAuth client store: vault-backed app setting with env
//! overrides plus a compiled-in default.
//!
//! Priority: env override wins, else the vault bundle/bare value, else the
//! compiled default below. Reads always yield a usable secret (one-click
//! sign-in); `has_stored` reports user-configured only (env or vault) for
//! Settings display. The desktop exchange/refresh and the adapter refresh
//! all read through here; the dev-only vite middleware (`dev-oauth.ts`)
//! reads the same env names directly. Never logged, never SQLite, never in
//! error bodies.

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
/// token calls additionally need the secret below (compiled default at
/// minimum, user override when configured).
pub const BUILTIN_CLIENT_ID: &str =
    "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";

/// Compiled-in default client secret for the built-in client above.
/// Installed-application client (cf. gemini-cli: "It's ok to save this in
/// git because this is an installed application ... the client secret is
/// obviously not treated as a secret"). Public in CLIProxyAPI
/// `internal/auth/antigravity` listings (pkg.go.dev, all versions). It only
/// identifies the app; tokens are per-user and it cannot spend anyone's
/// credits or quota. Env/vault overrides win when set; never log the value.
pub const COMPILED_DEFAULT_SECRET: &str = "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";

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

/// Parse a vault slot value: JSON bundle (`{"client_id","client_secret"}`)
/// wins; a non-JSON value is a legacy bare secret. Trims; blank counts as
/// unset. A JSON value without a usable secret is unset (never the raw JSON).
fn vault_secret_from_stored(stored: &str) -> Option<String> {
    let trimmed = stored.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(bundle) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return bundle
            .get("client_secret")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    Some(trimmed.to_string())
}

/// Resolve the effective secret from already-read inputs: env wins, else
/// vault bundle/bare value, else the compiled default. Pure (tested).
fn effective_secret(env: Option<String>, vault_stored: Option<String>) -> String {
    if let Some(from_env) = env {
        let trimmed = from_env.trim().to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    if let Some(stored) = vault_stored.as_deref() {
        if let Some(secret) = vault_secret_from_stored(stored) {
            return secret;
        }
    }
    COMPILED_DEFAULT_SECRET.to_string()
}

/// User-configured secret only (env or vault). None when the caller would
/// fall back to the compiled default. Never logs the value.
fn user_secret() -> Option<String> {
    if let Some(from_env) = env_override() {
        return Some(from_env);
    }
    vault_get().ok().and_then(|stored| vault_secret_from_stored(&stored))
}

/// Effective client: ID falls back through env → vault bundle → built-in;
/// secret is always `Some` (env → vault bundle/bare → compiled default).
/// Callers send `code_verifier` (PKCE) and `client_secret` together.
pub fn read_optional() -> (String, Option<String>) {
    let id = client_id();
    let secret = effective_secret(env_override(), vault_get().ok());
    (id, Some(secret))
}

/// Effective client with a secret. Never `Err` for a missing secret (the
/// compiled default applies); `Err` carries a terse pointer, never the
/// value. Legacy slot values that are a bare secret (pre-bundle era) are
/// honored as secret-only.
pub fn read() -> Result<OAuthClient, String> {
    let (client_id, secret) = read_optional();
    match secret {
        Some(client_secret) => Ok(OAuthClient { client_id, client_secret }),
        None => Err(format!(
            "Antigravity sign-in unavailable — set {ENV_OVERRIDE} or paste a client in Settings"
        )),
    }
}

/// Presence only (for Settings UI): a user-configured secret exists (env or
/// vault). The compiled default does NOT count. Never returns the value.
pub fn has_stored() -> bool {
    user_secret().is_some()
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
    fn default_fallback_when_unconfigured() {
        // Env wins in production; skip the live read when it is set so this
        // stays deterministic on dev machines and CI alike.
        if env_override().is_some() {
            return;
        }
        // Pure precedence: env > vault bundle/bare > compiled default.
        assert_eq!(effective_secret(Some(" env-s ".to_string()), Some("vault-s".to_string())), "env-s");
        assert_eq!(effective_secret(Some("  ".to_string()), Some("vault-s".to_string())), "vault-s");
        assert_eq!(effective_secret(None, None), COMPILED_DEFAULT_SECRET);
        // Live read never Errs for a missing secret; unconfigured machines
        // get exactly the compiled default.
        let client = read().expect("read falls back to default");
        assert!(!client.client_secret.trim().is_empty());
        let (_, opt) = read_optional();
        assert!(opt.as_deref().is_some_and(|s| !s.trim().is_empty()));
        if !has_stored() {
            assert_eq!(client.client_secret, COMPILED_DEFAULT_SECRET);
            assert_eq!(opt.as_deref(), Some(COMPILED_DEFAULT_SECRET));
        }
    }

    #[test]
    fn bundle_and_bare_values_parse() {
        let bundle = serde_json::json!({"client_id": "cid", "client_secret": "  s3cr3t  "}).to_string();
        assert_eq!(vault_secret_from_stored(&bundle).as_deref(), Some("s3cr3t"));
        assert_eq!(vault_secret_from_stored("  raw-bare-secret  ").as_deref(), Some("raw-bare-secret"));
        assert_eq!(effective_secret(None, Some(bundle.clone())), "s3cr3t");
        assert_eq!(effective_secret(None, Some("raw-bare-secret".to_string())), "raw-bare-secret");
        // Unusable slot values fall through to the compiled default.
        assert!(vault_secret_from_stored("").is_none());
        assert!(vault_secret_from_stored("   ").is_none());
        assert!(vault_secret_from_stored(&serde_json::json!({"client_id": "cid"}).to_string()).is_none());
        assert!(vault_secret_from_stored(&serde_json::json!({"client_secret": "  "}).to_string()).is_none());
        assert_eq!(effective_secret(None, Some("   ".to_string())), COMPILED_DEFAULT_SECRET);
    }

    #[test]
    fn errors_never_echo_secrets() {
        // Validation errors (no vault write) must not echo what was typed.
        let probe = "probe-secret-xyz-123";
        let err = set_override("", probe).unwrap_err();
        assert!(!err.contains(probe), "error echoes secret");
        assert!(!err.contains(COMPILED_DEFAULT_SECRET), "error echoes default");
        assert!(err.len() < 128, "error stays terse: {err}");
        // The unreachable missing-secret branch stays terse and secret-free.
        if let Err(err) = read() {
            assert!(!err.contains(COMPILED_DEFAULT_SECRET));
            assert!(err.len() < 128, "error stays terse: {err}");
        }
    }
}
