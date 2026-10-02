//! Headless Proxy Dock gateway (no desktop window).
//!
//! Runs the exact same Axum router + SQLite stack as the Tauri app so the
//! browser preview (`npm run dev`, vite `/api` proxy) has a backend to talk
//! to. Usage:
//!
//! ```text
//! cargo run --manifest-path src-tauri/Cargo.toml --bin gateway
//! PROXYDOCK_PORT=11434 PROXYDOCK_DATA_DIR=C:\temp\pd-data cargo run ... --bin gateway
//! ```
//!
//! - `PROXYDOCK_PORT` (default 11434, the shared candidate): ephemeral
//!   fallback when busy — the printed base URL is authoritative, and the vite
//!   `/api` proxy targets the candidate, so prefer a free candidate.
//!   (Old `PROXYHUB_*` names still work as fallbacks.)
//! - `PROXYDOCK_DATA_DIR` (default: the desktop app-data dir): point at a
//!   scratch dir to keep preview experiments away from real config.
//! - Secrets still live in the OS keyring (same `ai.proxydock` service, with
//!   fallback to the legacy `ai.proxyhub` service); SQLite never holds secrets.
//! - `PROXYDOCK_GATEWAY_KEY` (old `PROXYHUB_GATEWAY_KEY` fallback): headless
//!   parity for the unified Proxy Dock key. When set, it wins over the vault
//!   `gateway-key` slot (vault writes refuse while it is set). When unset,
//!   the gateway generates a key into the vault on first run — read it back
//!   via the desktop Settings, or set this env var so external clients and
//!   the browser preview share one known value. Every `/v1/*`, `/:slug/v1/*`,
//!   and `/api/*` call (except `/health` and OPTIONS) needs
//!   `Authorization: Bearer <key>`; external `base_url` clients (hermes,
//!   opencode) break with 401 `proxy_dock_unauthorized` until the header is
//!   added.

use proxy_dock::{db, gateway::router, SHARED_PORT_CANDIDATE};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "proxy_dock=info".into()),
        )
        .init();

    // Data dir: explicit override, else the desktop app-data location.
    // (Mirrored here because db::pool needs a Tauri AppHandle.)
    let dir = match std::env::var("PROXYDOCK_DATA_DIR")
        .or_else(|_| std::env::var("PROXYHUB_DATA_DIR"))
    {
        Ok(custom) if !custom.trim().is_empty() => std::path::PathBuf::from(custom),
        _ => {
            #[cfg(target_os = "windows")]
            let base = std::env::var("APPDATA").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from("."));
            #[cfg(target_os = "macos")]
            let base = std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join("Library/Application Support"))
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            let base = std::env::var("HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from("."));
            // New location (db::pool_in carries the legacy DB file forward).
            base.join("ai.proxydock.app")
        }
    };
    let pool = db::pool_in(&dir).await.map_err(|e| format!("database: {e}"))?;

    let candidate: u16 = std::env::var("PROXYDOCK_PORT")
        .or_else(|_| std::env::var("PROXYHUB_PORT"))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(SHARED_PORT_CANDIDATE);
    let listener = router::bind_with_fallback(candidate).await?;
    let port = listener.local_addr()?.port();
    println!("proxy-dock gateway on http://127.0.0.1:{port} (data: {})", dir.display());

    // Dev `dist/` next to the workspace root when present (same-origin UI).
    let dist_dir = std::env::current_dir()
        .ok()
        .map(|c| c.join("dist"))
        .filter(|d| d.join("index.html").is_file());
    router::serve_on_listener(pool, listener, port, dist_dir).await?;
    Ok(())
}
