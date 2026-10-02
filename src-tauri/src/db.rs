use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;
use tauri::{AppHandle, Manager};

use crate::PROVIDER_SLUGS;

pub async fn pool(app: &AppHandle) -> Result<SqlitePool, String> {
    let dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    // One-time rename carry-over across app-data dirs: the Tauri identifier
    // changed `ai.proxyhub.app` -> `ai.proxydock.app`, so a fresh dir would
    // orphan the live database. Copy it forward once when present.
    if !dir.join("proxy-dock.db").exists() {
        if let Some(parent) = dir.parent() {
            let legacy_db = parent.join("ai.proxyhub.app").join("proxy-hub.db");
            if legacy_db.exists() {
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::copy(&legacy_db, dir.join("proxy-dock.db"));
            }
        }
    }
    pool_in(&dir).await
}

/// Standalone pool for the headless gateway binary (no Tauri `AppHandle`).
/// Honors `PROXYDOCK_DATA_DIR` (old `PROXYHUB_DATA_DIR` still works) so
/// preview runs can point at a scratch dir, defaulting to the desktop
/// app-data location beside the real app.
pub async fn pool_in(dir: &std::path::Path) -> Result<SqlitePool, String> {
    let dir = if let Ok(override_dir) = std::env::var("PROXYDOCK_DATA_DIR")
        .or_else(|_| std::env::var("PROXYHUB_DATA_DIR"))
    {
        std::path::PathBuf::from(override_dir)
    } else {
        dir.to_path_buf()
    };
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let db_path = dir.join("proxy-dock.db");
    // One-time rename carry-over: copy the Proxy Hub era database forward
    // so usage history and routing survive the rename.
    if !db_path.exists() {
        let legacy = dir.join("proxy-hub.db");
        if legacy.exists() {
            let _ = std::fs::copy(&legacy, &db_path);
        }
    }
    let options = SqliteConnectOptions::from_str(&format!("sqlite:{}?mode=rwc", db_path.display()))
        .map_err(|err| err.to_string())?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        // 5s busy timeout alongside WAL: 5 pooled conns + background prune +
        // pricing sync can briefly contend; fail loud only after waiting.
        .busy_timeout(std::time::Duration::from_secs(5));
    let pool = SqlitePoolOptions::new().max_connections(5).connect_with(options).await.map_err(|err| err.to_string())?;
    // sqlx migrations run in a transaction where `PRAGMA foreign_keys=OFF` is
    // a no-op (FK enforcement stays on) — see the 005-orphan guard recipe
    // below. Child tables (routing_policy, the only FK holder) must be
    // rebuilt/dropped before parents (accounts), never the reverse. Never
    // edit applied migrations (001-010+ checksums); all schema changes go in
    // a NEW file. Future rebuild migrations must delete orphan child rows
    // BEFORE the copy:
    //   DELETE FROM routing_policy WHERE NOT EXISTS
    //     (SELECT 1 FROM accounts_new
    //       WHERE accounts_new.provider_slug = routing_policy.provider_slug
    //         AND accounts_new.id = routing_policy.account_id);
    // or startup migration aborts on the first orphan (sqlite.org/foreignkeys,
    // launchbadge/sqlx#2085). Migration 005 itself is already applied and
    // stays untouched — this recipe is for FUTURE rebuilds only.
    sqlx::migrate!("../migrations").run(&pool).await.map_err(|err| err.to_string())?;
    seed_providers(&pool).await?;
    migrate_commandcode_slug(&pool).await;
    Ok(pool)
}

/// One-time rename `commandcode-go` -> `commandcode` (Go is a plan, not a
/// provider). Moves keyring secrets to the new slot first (old ids are known
/// only before the row rewrite), then rewrites SQLite rows. Best-effort:
/// never fails startup; leftover old slots simply require re-linking.
/// Covers all seven slug-carrying tables (accounts, routing_policy,
/// usage_events, quota_observations, model_cache, pricing_snapshots,
/// gateway_events) in one transaction; per-table counts log at info.
/// Idempotent: re-runs on every boot as the repair job for leftover
/// `commandcode-go` rows (old-slug catalog/prices/history would otherwise
/// stay invisible and lose exact-price matches).
async fn migrate_commandcode_slug(pool: &SqlitePool) {
    const OLD: &str = "commandcode-go";
    const NEW: &str = "commandcode";
    let ids: Vec<(String,)> = sqlx::query_as("SELECT id FROM accounts WHERE provider_slug = 'commandcode-go'")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    for (id,) in &ids {
        if let Ok(secret) = crate::secrets::read_raw_token(OLD, id) {
            if !secret.trim().is_empty() {
                let _ = crate::secrets::store_raw_token(NEW, id, &secret);
                let _ = crate::secrets::clear_local_token(OLD.to_string(), id.clone());
            }
        }
    }
    let Ok(mut tx) = pool.begin().await else {
        return;
    };
    let mut counts: Vec<(&str, u64)> = Vec::new();
    let tables: [(&str, &str); 8] = [
        ("providers", "slug"),
        ("accounts", "provider_slug"),
        ("routing_policy", "provider_slug"),
        ("usage_events", "provider_slug"),
        ("quota_observations", "provider_slug"),
        ("model_cache", "provider_slug"),
        ("pricing_snapshots", "provider_slug"),
        ("gateway_events", "provider_slug"),
    ];
    for (table, column) in tables {
        // Tables from newer migrations may not exist on very old DBs mid-
        // migrate; per-table errors stay best-effort inside the tx.
        let n = sqlx::query(&format!("UPDATE {table} SET {column} = '{NEW}' WHERE {column} = '{OLD}'"))
            .execute(&mut *tx)
            .await
            .map(|r| r.rows_affected())
            .unwrap_or(0);
        counts.push((table, n));
    }
    if tx.commit().await.is_ok() {
        let total: u64 = counts.iter().map(|(_, n)| n).sum();
        if total > 0 {
            tracing::info!(?counts, "commandcode slug repair moved rows");
        }
    }
}

async fn seed_providers(pool: &SqlitePool) -> Result<(), String> {
    let labels = [
        ("commandcode", "Command Code"),
        ("opencode", "OpenCode"),
        ("chatgpt", "ChatGPT Codex"),
        ("antigravity", "Antigravity"),
        ("claude", "Claude"),
    ];
    for slug in PROVIDER_SLUGS {
        let label = labels.iter().find(|(s, _)| *s == slug).map(|(_, l)| *l).unwrap_or(slug);
        sqlx::query("INSERT OR IGNORE INTO providers (slug, label) VALUES (?, ?)")
            .bind(slug)
            .bind(label)
            .execute(pool)
            .await
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

/// Register a linked account for gateway rolling. Called from the sign-in /
/// verify commands after the credential lands in the OS keyring. Secrets stay
/// in the keyring — this row holds routing metadata only, never the secret.
///
/// No-clobber contract (pinned): `INSERT OR IGNORE` on the composite
/// `(provider_slug, id)` PK means re-verify updates `plan` but NEVER
/// overwrites a user-edited `label`. See `same_identity_*` router tests.
pub async fn register_account(
    pool: &SqlitePool,
    provider_slug: &str,
    account_id: &str,
    label: &str,
    plan: Option<&str>,
) -> Result<(), String> {
    sqlx::query("INSERT OR IGNORE INTO accounts (id, provider_slug, label, priority) VALUES (?, ?, ?, 1)")
        .bind(account_id)
        .bind(provider_slug)
        .bind(label)
        .execute(pool)
        .await
        .map_err(|err| err.to_string())?;
    if let Some(plan) = plan {
        sqlx::query("UPDATE accounts SET plan = ? WHERE id = ? AND provider_slug = ?")
            .bind(plan)
            .bind(account_id)
            .bind(provider_slug)
            .execute(pool)
            .await
            .map_err(|err| err.to_string())?;
    }
    sqlx::query("INSERT OR IGNORE INTO routing_policy (provider_slug, account_id, priority, enabled) VALUES (?, ?, 1, 1)")
        .bind(provider_slug)
        .bind(account_id)
        .execute(pool)
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// Verify-before-save routing gate (migration 011 `accounts.verified`):
/// pasted ChatGPT/Antigravity OAuth stores with `verified = 0` and stays
/// ineligible for routing until a quota refresh validates it (which calls
/// [`mark_account_verified`]). Browser OAuth and key verifies store
/// `verified = 1` directly. Reads tolerate pre-011 DBs (missing column =
/// treated as verified, preserving old behavior).
/// Mark an account verified after a successful quota refresh. Best-effort:
/// never fails callers (missing column on pre-011 DBs is fine).
pub async fn mark_account_verified(pool: &SqlitePool, provider_slug: &str, account_id: &str) -> Result<(), String> {
    sqlx::query("UPDATE accounts SET verified = 1 WHERE id = ? AND provider_slug = ?")
        .bind(account_id)
        .bind(provider_slug)
        .execute(pool)
        .await
        .map(|_| ())
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// Mark an account unverified at paste-import time (sibling calls this from
/// the credential-import path before any quota check). Best-effort.
pub async fn mark_account_unverified(pool: &SqlitePool, provider_slug: &str, account_id: &str) -> Result<(), String> {
    sqlx::query("UPDATE accounts SET verified = 0 WHERE id = ? AND provider_slug = ?")
        .bind(account_id)
        .bind(provider_slug)
        .execute(pool)
        .await
        .map(|_| ())
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// True when the account may serve traffic: no row, or `verified != 0`.
/// Missing column (pre-011 DBs, in-memory test schemas) reads as eligible.
pub async fn is_account_routable(pool: &SqlitePool, provider_slug: &str, account_id: &str) -> bool {
    sqlx::query_as::<_, (Option<i64>,)>("SELECT verified FROM accounts WHERE id = ? AND provider_slug = ?")
        .bind(account_id)
        .bind(provider_slug)
        .fetch_optional(pool)
        .await
        .unwrap_or(None)
        .map(|(v,)| v.unwrap_or(1) != 0)
        .unwrap_or(true)
}

/// Persist a freshly observed plan (e.g. from quota refresh) for transport
/// selection. `None` never clobbers a known plan.
pub async fn update_account_plan(
    pool: &SqlitePool,
    provider_slug: &str,
    account_id: &str,
    plan: Option<&str>,
) -> Result<(), String> {
    let Some(plan) = plan else { return Ok(()) };
    sqlx::query("UPDATE accounts SET plan = ? WHERE id = ? AND provider_slug = ?")
        .bind(plan)
        .bind(account_id)
        .bind(provider_slug)
        .execute(pool)
        .await
        .map_err(|err| err.to_string())?;
    Ok(())
}
