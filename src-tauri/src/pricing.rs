//! Background pricing sync from Models.dev (no UI, no key, no signup).
//!
//! Fills per-model USD/1k-token rates for cached catalog models that have no
//! price yet, so usage costs show up instead of "unknown". Rules:
//! - Only models present in our own `model_cache` are ever priced — nothing
//!   is invented for models we don't serve.
//! - Manual/official snapshots always win: sync inserts only when a pair has
//!   no snapshot at all, or when the latest snapshot itself came from
//!   models.dev (refresh third-party data, keep history via append).
//! - Candidate costs of 0/0 are treated as missing data, not free tiers.
//! - Base-tier `cost.input`/`cost.output` per 1M only (tiers and
//!   `context_over_200k` ignored); converted to per-1k for storage.
//! - Runs on gateway boot and after successful catalog refreshes, spawned off
//!   the request path; failures only log, never fail requests.

use sqlx::SqlitePool;

pub const MODELS_DEV_URL: &str = "https://models.dev/api.json";

/// One priced entry extracted from the Models.dev dataset.
#[derive(Debug, Clone)]
pub struct RawEntry {
    /// Normalized lookup key (lowercased dataset key, vendor prefix kept).
    pub key: String,
    /// Models.dev provider id (for canonical-vendor preference).
    pub vendor: String,
    /// Base-tier USD per 1M tokens.
    pub input_per_1m: f64,
    /// Base-tier USD per 1M tokens.
    pub output_per_1m: f64,
}

/// Flatten the Models.dev `{provider: {models: {id: {cost: {input, output}}}}}`
/// shape into priced entries. Skips malformed costs, negatives, absurd
/// values, and 0/0 placeholders (missing data, not free tiers).
pub fn parse_models_dev(doc: &serde_json::Value) -> Vec<RawEntry> {
    let mut out = Vec::new();
    let Some(providers) = doc.as_object() else {
        return out;
    };
    for (vendor, pvalue) in providers {
        let models = pvalue.get("models").and_then(|v| v.as_object());
        let Some(models) = models else { continue };
        for (id, m) in models {
            let (Some(input), Some(output)) = (
                m.get("cost").and_then(|c| c.get("input")).and_then(|v| v.as_f64()),
                m.get("cost").and_then(|c| c.get("output")).and_then(|v| v.as_f64()),
            ) else {
                continue;
            };
            if !input.is_finite()
                || !output.is_finite()
                || input < 0.0
                || output < 0.0
                || input >= 1e6
                || output >= 1e6
                || (input == 0.0 && output == 0.0)
            {
                continue;
            }
            out.push(RawEntry {
                key: id.to_lowercase(),
                vendor: vendor.clone(),
                input_per_1m: input,
                output_per_1m: output,
            });
        }
    }
    out
}

/// Canonical upstream vendor for a native model id (lowercased). Used to
/// prefer the authoritative rate when resellers disagree.
fn canonical_vendor(native_lower: &str) -> Option<&'static str> {
    let tail = native_lower.rsplit('/').next().unwrap_or(native_lower);
    for (prefix, vendor) in [
        ("claude-", "anthropic"),
        ("gemini-", "google"),
        ("gemma-", "google"),
        ("gpt-", "openai"),
        ("chatgpt-", "openai"),
        ("o1", "openai"),
        ("o3", "openai"),
        ("o4", "openai"),
        ("kimi-", "moonshotai"),
        ("moonshotai/", "moonshotai"),
        ("glm-", "zai"),
        ("zai-", "zai"),
        ("grok-", "xai"),
        ("xai/", "xai"),
        ("deepseek", "deepseek"),
        ("qwen", "qwen"),
        ("minimax", "minimax"),
        ("muse-spark", "meta"),
        ("meta/", "meta"),
        ("stepfun/", "stepfun"),
        ("tencent/", "tencent"),
        ("hy3", "tencent"),
        ("hy4", "tencent"),
        ("nemotron", "nvidia"),
        ("nvidia/", "nvidia"),
        ("laguna", "poolside"),
        ("poolside/", "poolside"),
        ("inkling", "thinkingmachines"),
        ("thinkingmachines/", "thinkingmachines"),
        ("mimo", "xiaomi"),
        ("xiaomi/", "xiaomi"),
        ("longcat", "meituan"),
        ("meituan/", "meituan"),
        ("fugu", "sakana"),
        ("sakana/", "sakana"),
        ("ling-", "inclusionai"),
    ] {
        if native_lower.starts_with(prefix) || tail.starts_with(prefix) {
            return Some(vendor);
        }
    }
    None
}

/// Match a native model id to a per-1M (input, output) rate.
///
/// Both sides are normalized: exact key first, then last-segment on either
/// side — so dataset key `anthropic/claude-opus-5-5` serves native
/// `claude-opus-5-5` and vice versa. Canonical vendor wins, else reseller
/// consensus (mode), else first seen. `None` when nothing matches.
/// Variant-suffixed ids (`-thinking`, `-agent`, …) do NOT match here — see
/// [`match_variant`].
pub fn match_price(entries: &[RawEntry], native_id: &str) -> Option<(f64, f64)> {
    let native = native_id.to_lowercase();
    pick_price(&candidates_for(entries, &native), &native)
}

/// Candidate dataset entries for a normalized id: full-key hits plus
/// last-segment hits on either side (vendor prefixes live on both sides).
fn candidates_for<'a>(entries: &'a [RawEntry], native_lower: &str) -> Vec<&'a RawEntry> {
    let tail = native_lower.rsplit('/').next().unwrap_or(native_lower).to_string();
    let mut seen = std::collections::HashSet::new();
    let mut candidates: Vec<&RawEntry> = Vec::new();
    for e in entries {
        let entry_tail = e.key.rsplit('/').next().unwrap_or(&e.key);
        if e.key == *native_lower || e.key == tail || entry_tail == tail {
            if seen.insert((e.vendor.clone(), e.key.clone())) {
                candidates.push(e);
            }
        }
    }
    candidates
}

/// Canonical-vendor preference, else reseller-consensus mode, else first.
fn pick_price(candidates: &[&RawEntry], native_lower: &str) -> Option<(f64, f64)> {
    if candidates.is_empty() {
        return None;
    }
    if let Some(vendor) = canonical_vendor(native_lower) {
        if let Some(hit) = candidates.iter().find(|e| e.vendor == vendor) {
            return Some((hit.input_per_1m, hit.output_per_1m));
        }
    }
    // Reseller consensus: most common exact pair wins ties deterministically.
    let mut counts: std::collections::HashMap<(u64, u64), usize> = std::collections::HashMap::new();
    let mut order: Vec<(u64, u64)> = Vec::new();
    for e in candidates.iter() {
        let key = (e.input_per_1m.to_bits(), e.output_per_1m.to_bits());
        if counts.insert(key, counts.get(&key).copied().unwrap_or(0) + 1).is_none() {
            order.push(key);
        }
    }
    let mut best: Option<(u64, u64)> = None;
    let mut best_count = 0;
    for key in order {
        let count = counts[&key];
        if count > best_count {
            best_count = count;
            best = Some(key);
        }
    }
    best.map(|(i, o)| (f64::from_bits(i), f64::from_bits(o)))
}

/// Same-model variant suffixes: packaging/mode/effort markers that share the
/// base model's rate (thinking is billed as output at standard rates;
/// -agent/-high/-low/-medium/-tiered/-extra-low/-paid are serving options,
/// not different SKUs). Longest first so `-extra-low` beats `-low`.
/// Suffixes that DO change price (`-fast`, `-pro`, `-lite`, `-mini`,
/// `-nano`, `-max`, `-ultra`, `-preview`, `-exp`, `-turbo`, `-vision`,
/// `-spark`) are deliberately absent and never stripped.
const VARIANT_SUFFIXES: &[&str] = &[
    "-extra-low",
    "-thinking",
    "-agent",
    "-tiered",
    "-medium",
    "-high",
    "-low",
    "-paid",
];

/// Strip one known same-model variant suffix. Returns the base id.
pub fn strip_variant(native_lower: &str) -> Option<String> {
    for suffix in VARIANT_SUFFIXES {
        if native_lower.len() > suffix.len() && native_lower.ends_with(suffix) {
            return Some(native_lower[..native_lower.len() - suffix.len()].to_string());
        }
    }
    None
}

/// Free-tier naming convention (`-free` / `:free` suffix): billed nothing.
/// Only used when nothing matches at all.
pub fn is_free_tier_id(native_lower: &str) -> bool {
    native_lower.ends_with("-free") || native_lower.ends_with(":free")
}

/// Second-chance match: strip a variant suffix, then match the base id with
/// the same preference rules. Returns the base id used plus its rates, so
/// the snapshot source can record the mapping.
pub fn match_variant(entries: &[RawEntry], native_id: &str) -> Option<(String, f64, f64)> {
    let native = native_id.to_lowercase();
    let base = strip_variant(&native)?;
    pick_price(&candidates_for(entries, &base), &base).map(|(i, o)| (base, i, o))
}

/// Source rank for bare-slug price lookups: official/manual snapshots always
/// beat third-party ones, regardless of recency. `1` = authoritative
/// (official page, manual PUT), `0` = everything else (models.dev, unknown).
/// The bare lookup orders by rank DESC, id DESC so an older official row
/// shadows a newer scraped row; the backfill below only fills rows with no
/// exact price of their own.
pub fn source_rank(source: &str) -> i64 {
    let lower = source.trim().to_lowercase();
    if lower.starts_with("official") || lower.starts_with("manual") {
        1
    } else {
        0
    }
}

/// ORDER BY fragment for ranked bare lookups (official/manual first, then
/// newest). Shared with the gateway's `priced_cost` bare branch.
pub const BARE_RANK_ORDER_BY: &str =
    "CASE WHEN lower(source) LIKE 'official%' OR lower(source) LIKE 'manual%' THEN 1 ELSE 0 END DESC, id DESC";

/// Ranked bare-slug price: the latest snapshot across providers for the same
/// bare model slug (after the last `/`, case-insensitive), preferring
/// official/manual sources over recency. Returns the price JSON plus its
/// source. `None` when nothing matches.
pub async fn lookup_bare_price(pool: &SqlitePool, model: &str) -> Option<(String, String)> {
    sqlx::query_as::<_, (String, String)>(&format!(
        "SELECT price_json, source FROM pricing_snapshots WHERE lower(substr(model, instr(model, '/') + 1)) = lower(substr(?1, instr(?1, '/') + 1)) ORDER BY {BARE_RANK_ORDER_BY} LIMIT 1",
    ))
    .bind(model)
    .fetch_optional(pool)
    .await
    .unwrap_or(None)
}

/// Honesty counts for usage summaries: priced vs unpriced requests.
/// `estimated_cost IS NOT NULL` = priced (including explicit 0.0 free-tier or
/// manual-zero rows — a known price, honestly zero); NULL = unknown, never
/// presented as $0.00. Optional provider/account filter; unfiltered when both
/// are empty.
pub async fn priced_counts(pool: &SqlitePool, provider_slug: &str, account_id: &str) -> (i64, i64) {
    let (priced_sql, total_sql) = if provider_slug.is_empty() && account_id.is_empty() {
        (
            "SELECT COUNT(*) FROM usage_events WHERE estimated_cost IS NOT NULL".to_string(),
            "SELECT COUNT(*) FROM usage_events".to_string(),
        )
    } else if account_id.is_empty() {
        (
            "SELECT COUNT(*) FROM usage_events WHERE provider_slug = ? AND estimated_cost IS NOT NULL".to_string(),
            "SELECT COUNT(*) FROM usage_events WHERE provider_slug = ?".to_string(),
        )
    } else {
        (
            "SELECT COUNT(*) FROM usage_events WHERE provider_slug = ? AND account_id = ? AND estimated_cost IS NOT NULL".to_string(),
            "SELECT COUNT(*) FROM usage_events WHERE provider_slug = ? AND account_id = ?".to_string(),
        )
    };
    let mut priced_q = sqlx::query_as::<_, (i64,)>(&priced_sql);
    let mut total_q = sqlx::query_as::<_, (i64,)>(&total_sql);
    if !provider_slug.is_empty() {
        priced_q = priced_q.bind(provider_slug);
        total_q = total_q.bind(provider_slug);
    }
    if !account_id.is_empty() {
        priced_q = priced_q.bind(account_id);
        total_q = total_q.bind(account_id);
    }
    let priced: i64 = priced_q.fetch_optional(pool).await.unwrap_or(None).map(|(n,)| n).unwrap_or(0);
    let total: i64 = total_q.fetch_optional(pool).await.unwrap_or(None).map(|(n,)| n).unwrap_or(0);
    (priced, total.saturating_sub(priced).max(0))
}

/// Latest snapshot source for a pair, if any.
async fn latest_source(pool: &SqlitePool, provider_slug: &str, model: &str) -> Option<String> {
    sqlx::query_as::<_, (String,)>(
        "SELECT source FROM pricing_snapshots WHERE provider_slug = ? AND model = ? ORDER BY id DESC LIMIT 1",
    )
    .bind(provider_slug)
    .bind(model)
    .fetch_optional(pool)
    .await
    .unwrap_or(None)
    .map(|(s,)| s)
}

/// Fill past usage rows with the given per-1k rates. Exact (provider, model)
/// rows take NULLs plus rows previously filled from another provider's price
/// (`bare`); then the bare model slug (after the last `/`, case-insensitive)
/// shares the price with the same model served via other providers — unless
/// their own provider priced them exactly. Best-effort, never fails callers.
///
/// The four-leg formula prices uncached input at the input rate and each
/// cache leg at its own rate (`None` falls back to the input rate, so rows
/// without cache-rate snapshots keep the old two-rate math exactly).
/// `cache_savings` records what the cached legs would have cost at full input
/// rates minus what they cost (never negative).
pub async fn backfill_costs(
    pool: &SqlitePool,
    provider_slug: &str,
    model: &str,
    prompt_per_1k: f64,
    completion_per_1k: f64,
    cache_read_per_1k: Option<f64>,
    cache_write_per_1k: Option<f64>,
) {
    let valid = |r: f64| r.is_finite() && r >= 0.0;
    let read_rate = cache_read_per_1k.filter(|r| valid(*r)).unwrap_or(prompt_per_1k);
    let write_rate = cache_write_per_1k.filter(|r| valid(*r)).unwrap_or(prompt_per_1k);
    let cost_sql = "UPDATE usage_events SET estimated_cost = MAX(prompt_tokens - cached_tokens - cache_creation_tokens, 0) / 1000.0 * ?1 + cached_tokens / 1000.0 * ?2 + cache_creation_tokens / 1000.0 * ?3 + completion_tokens / 1000.0 * ?4, cache_savings = MAX(cached_tokens / 1000.0 * (?1 - ?2), 0)";
    let _ = sqlx::query(&format!("{cost_sql}, cost_source = 'exact' WHERE provider_slug = ?5 AND model = ?6 AND (estimated_cost IS NULL OR cost_source = 'bare')"))
    .bind(prompt_per_1k)
    .bind(read_rate)
    .bind(write_rate)
    .bind(completion_per_1k)
    .bind(provider_slug)
    .bind(model)
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!("{cost_sql}, cost_source = 'bare' WHERE estimated_cost IS NULL AND lower(substr(model, instr(model, '/') + 1)) = lower(substr(?6, instr(?6, '/') + 1)) AND NOT (provider_slug = ?5 AND model = ?6) AND NOT EXISTS (SELECT 1 FROM pricing_snapshots p WHERE p.provider_slug = usage_events.provider_slug AND p.model = usage_events.model)"))
    .bind(prompt_per_1k)
    .bind(read_rate)
    .bind(write_rate)
    .bind(completion_per_1k)
    .bind(provider_slug)
    .bind(model)
    .execute(pool)
    .await;
}

fn today_label() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

#[derive(Debug, Default)]
pub struct SyncReport {
    pub inserted: usize,
    pub refreshed: usize,
    pub skipped_have_price: usize,
    pub unmatched: Vec<String>,
}

/// Fill missing prices for every cached catalog model from pre-fetched
/// Models.dev entries. Manual/official snapshots always win; rows whose
/// latest snapshot came from models.dev are refreshed (history appends).
/// Fallback chain per model: exact match, same-model variant base (source
/// records the `~base` mapping), free-tier suffix (`-free`/`:free` → zero),
/// else honestly unmatched.
pub async fn fill_missing(pool: &SqlitePool, entries: &[RawEntry]) -> SyncReport {
    let mut report = SyncReport::default();
    let cached: Vec<(String, String)> =
        sqlx::query_as("SELECT DISTINCT provider_slug, native_id FROM model_cache")
            .fetch_all(pool)
            .await
            .unwrap_or_default();
    let today = today_label();
    for (slug, native_id) in cached {
        let priced: Option<(f64, f64, String)> = match match_price(entries, &native_id) {
            Some((input_1m, output_1m)) => {
                Some((input_1m / 1000.0, output_1m / 1000.0, format!("models.dev {today}")))
            }
            None => match match_variant(entries, &native_id) {
                Some((base, input_1m, output_1m)) => Some((
                    input_1m / 1000.0,
                    output_1m / 1000.0,
                    format!("models.dev {today} (~{base})"),
                )),
                None if is_free_tier_id(&native_id.to_lowercase()) => {
                    Some((0.0, 0.0, format!("models.dev {today} (free suffix)")))
                }
                None => None,
            },
        };
        let Some((prompt_per_1k, completion_per_1k, source)) = priced else {
            report.unmatched.push(format!("{slug}/{native_id}"));
            continue;
        };
        match latest_source(pool, &slug, &native_id).await {
            None => {
                let price_json =
                    serde_json::json!({"prompt_per_1k": prompt_per_1k, "completion_per_1k": completion_per_1k})
                        .to_string();
                if sqlx::query(
                    "INSERT INTO pricing_snapshots (provider_slug, model, source, price_json) VALUES (?, ?, ?, ?)",
                )
                .bind(&slug)
                .bind(&native_id)
                .bind(&source)
                .bind(&price_json)
                .execute(pool)
                .await
                .is_err()
                {
                    continue;
                }
                backfill_costs(pool, &slug, &native_id, prompt_per_1k, completion_per_1k, None, None).await;
                report.inserted += 1;
            }
            Some(prev) if prev.starts_with("models.dev") => {
                let price_json =
                    serde_json::json!({"prompt_per_1k": prompt_per_1k, "completion_per_1k": completion_per_1k})
                        .to_string();
                if sqlx::query(
                    "INSERT INTO pricing_snapshots (provider_slug, model, source, price_json) VALUES (?, ?, ?, ?)",
                )
                .bind(&slug)
                .bind(&native_id)
                .bind(&source)
                .bind(&price_json)
                .execute(pool)
                .await
                .is_err()
                {
                    continue;
                }
                backfill_costs(pool, &slug, &native_id, prompt_per_1k, completion_per_1k, None, None).await;
                report.refreshed += 1;
            }
            Some(_) => {
                report.skipped_have_price += 1;
            }
        }
    }
    report.unmatched.sort();
    report.unmatched.dedup();
    report
}

/// Fetch the Models.dev dataset. Heavy (~5 MB); callers fetch once per sync.
pub async fn fetch_dataset() -> Result<Vec<RawEntry>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|err| format!("pricing sync client failed: {err}"))?;
    let resp = client
        .get(MODELS_DEV_URL)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|err| format!("models.dev fetch failed: {err}"))?;
    if !resp.status().is_success() {
        return Err(format!("models.dev fetch failed ({})", resp.status()));
    }
    let doc: serde_json::Value = resp
        .json()
        .await
        .map_err(|err| format!("models.dev parse failed: {err}"))?;
    Ok(parse_models_dev(&doc))
}

/// Pricing-sync singleflight + freshness: at most one sync in flight, and a
/// successful dataset fetch counts as fresh for 1h (skipped while fresh).
/// Syncs stay off the request path (`spawn_sync`); the manual endpoint shares
/// this gate so double-clicks don't double-fetch the ~5MB dataset.
pub const PRICING_FRESH_SECS: u64 = 3600;

struct SyncState {
    last_ok: Option<std::time::Instant>,
    running: bool,
}

static SYNC_STATE: std::sync::OnceLock<std::sync::Mutex<SyncState>> = std::sync::OnceLock::new();

fn sync_state() -> &'static std::sync::Mutex<SyncState> {
    SYNC_STATE.get_or_init(|| {
        std::sync::Mutex::new(SyncState { last_ok: None, running: false })
    })
}

/// Try to claim the sync slot: `false` when another sync runs or the dataset
/// is still fresh (caller skips). The claim releases via `finish_sync`.
fn claim_sync() -> bool {
    let Ok(mut state) = sync_state().lock() else {
        return false;
    };
    if state.running {
        return false;
    }
    if let Some(at) = state.last_ok {
        if at.elapsed().as_secs() < PRICING_FRESH_SECS {
            return false;
        }
    }
    state.running = true;
    true
}

fn finish_sync(ok: bool) {
    if let Ok(mut state) = sync_state().lock() {
        state.running = false;
        if ok {
            state.last_ok = Some(std::time::Instant::now());
        }
    }
}

/// Full sync: fetch once, fill every cached provider. Best-effort by design.
/// Skipped while another sync runs or the dataset is fresh (<1h old).
pub async fn sync_pricing(pool: &SqlitePool) -> SyncReport {
    if !claim_sync() {
        return SyncReport::default();
    }
    let ok = match fetch_dataset().await {
        Ok(entries) => {
            let report = fill_missing(pool, &entries).await;
            finish_sync(true);
            return report;
        }
        Err(err) => {
            tracing::warn!(error = %err, "models.dev pricing sync skipped");
            false
        }
    };
    finish_sync(ok);
    SyncReport::default()
}

/// Spawn the sync off the request path (boot + post-refresh hooks).
pub fn spawn_sync(pool: &SqlitePool) {
    let pool = pool.clone();
    tokio::spawn(async move {
        let report = sync_pricing(&pool).await;
        tracing::info!(
            inserted = report.inserted,
            refreshed = report.refreshed,
            skipped = report.skipped_have_price,
            unmatched = report.unmatched.len(),
            "models.dev pricing sync done"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_doc() -> serde_json::Value {
        serde_json::json!({
            "openai": {"models": {
                "gpt-6-luna": {"cost": {"input": 0.1, "output": 0.5}},
                "gpt-5.4": {"cost": {"input": 2.5, "output": 15}},
            }},
            "nano-gpt": {"models": {
                "openai/gpt-6-luna": {"cost": {"input": 0.1, "output": 0.5}},
                "glm-5.3-flash": {"cost": {"input": 0.15, "output": 0.5}},
                "odd-free": {"cost": {"input": 0, "output": 0}},
                "odd-negative": {"cost": {"input": -1, "output": 2}},
                "odd-novals": {"cost": {}},
            }},
            "evil-reseller": {"models": {
                "glm-5.3-flash": {"cost": {"input": 99, "output": 99}},
            }},
            "mystery-vendor": {"models": {
                "anthropic/claude-opus-5-5": {"cost": {"input": 5, "output": 25}},
            }},
            "second-reseller": {"models": {
                "glm-5.3-flash": {"cost": {"input": 0.15, "output": 0.5}},
            }},
            "anthropic": {"models": {
                "claude-sonnet-4-6": {"cost": {"input": 3, "output": 15}},
            }},
            "venice": {"models": {
                "claude-sonnet-4-6": {"cost": {"input": 3.6, "output": 18}},
            }},
        })
    }

    #[test]
    fn models_dev_matching_prefers_canonical_then_consensus() {
        let entries = parse_models_dev(&sample_doc());
        // 0/0, negative, and valueless costs never parse.
        assert!(entries.iter().all(|e| e.key != "odd-free"));
        assert!(entries.iter().all(|e| e.key != "odd-negative"));
        assert!(entries.iter().all(|e| e.key != "odd-novals"));
        // Exact + vendor-prefixed keys resolve; canonical vendor wins ties.
        assert_eq!(match_price(&entries, "gpt-6-luna"), Some((0.1, 0.5)));
        assert_eq!(match_price(&entries, "openai/gpt-6-luna"), Some((0.1, 0.5)));
        assert_eq!(match_price(&entries, "claude-sonnet-4-6"), Some((3.0, 15.0)));
        // Consensus beats a lone outlier reseller (2 vs 1).
        assert_eq!(match_price(&entries, "glm-5.3-flash"), Some((0.15, 0.5)));
        // Case-insensitive; opaque ids miss.
        assert_eq!(match_price(&entries, "GPT-5.4"), Some((2.5, 15.0)));
        assert_eq!(match_price(&entries, "chat_20706"), None);
        // Vendor-prefixed dataset keys serve bare native ids (canon wins).
        assert_eq!(match_price(&entries, "claude-opus-5-5"), Some((5.0, 25.0)));
    }

    #[tokio::test]
    async fn fill_missing_respects_manual_prices() {
        let pool = SqlitePool::connect("sqlite::memory:").await.expect("memory pool");
        sqlx::query("CREATE TABLE model_cache (provider_slug TEXT NOT NULL, native_id TEXT NOT NULL, context_length INTEGER, endpoints TEXT NOT NULL DEFAULT '[\"chat\"]', updated_at TEXT, source TEXT NOT NULL DEFAULT 'live', PRIMARY KEY (provider_slug, native_id))")
            .execute(&pool).await.expect("cache table");
        sqlx::query("CREATE TABLE pricing_snapshots (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, model TEXT NOT NULL, source TEXT NOT NULL, price_json TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(&pool).await.expect("pricing table");
        sqlx::query("CREATE TABLE usage_events (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, account_id TEXT, model TEXT NOT NULL, prompt_tokens INTEGER NOT NULL DEFAULT 0, completion_tokens INTEGER NOT NULL DEFAULT 0, cached_tokens INTEGER NOT NULL DEFAULT 0, cache_creation_tokens INTEGER NOT NULL DEFAULT 0, estimated_cost REAL, cache_savings REAL NOT NULL DEFAULT 0, cost_source TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(&pool).await.expect("usage table");
        sqlx::query("INSERT INTO model_cache (provider_slug, native_id) VALUES ('commandcode', 'gpt-6-luna'), ('commandcode', 'gpt-5.4'), ('commandcode', 'ghost-m')")
            .execute(&pool).await.expect("cache seed");
        sqlx::query("INSERT INTO pricing_snapshots (provider_slug, model, source, price_json) VALUES ('commandcode', 'gpt-5.4', 'official page', '{\"prompt_per_1k\": 9, \"completion_per_1k\": 9}')")
            .execute(&pool).await.expect("manual seed");
        sqlx::query("INSERT INTO usage_events (provider_slug, account_id, model, prompt_tokens, completion_tokens, estimated_cost) VALUES ('commandcode', 'a', 'gpt-6-luna', 1000, 0, NULL)")
            .execute(&pool).await.expect("usage seed");
        let entries = parse_models_dev(&sample_doc());
        let report = fill_missing(&pool, &entries).await;
        assert_eq!(report.inserted, 1);
        assert_eq!(report.skipped_have_price, 1);
        assert_eq!(report.unmatched, vec!["commandcode/ghost-m".to_string()]);
        // Manual row untouched; synced row priced + backfilled.
        let manual: (String,) = sqlx::query_as("SELECT price_json FROM pricing_snapshots WHERE provider_slug = 'commandcode' AND model = 'gpt-5.4' ORDER BY id DESC LIMIT 1")
            .fetch_one(&pool).await.expect("manual row");
        assert!(manual.0.contains('9'));
        let cost: (Option<f64>,) = sqlx::query_as("SELECT estimated_cost FROM usage_events WHERE model = 'gpt-6-luna'")
            .fetch_one(&pool).await.expect("usage row");
        assert_eq!(cost.0, Some(0.0001));
        // Second run refreshes the models.dev row (history appends) instead of duplicating blindly.
        let report2 = fill_missing(&pool, &entries).await;
        assert_eq!(report2.inserted, 0);
        assert_eq!(report2.refreshed, 1);
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pricing_snapshots WHERE provider_slug = 'commandcode' AND model = 'gpt-6-luna'")
            .fetch_one(&pool).await.expect("count");
        assert_eq!(count.0, 2);
    }

    #[tokio::test]
    async fn bare_lookup_prefers_official_over_newer_scraped() {
        let pool = SqlitePool::connect("sqlite::memory:").await.expect("memory pool");
        sqlx::query("CREATE TABLE pricing_snapshots (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, model TEXT NOT NULL, source TEXT NOT NULL, price_json TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(&pool).await.expect("pricing table");
        // Newer scraped row first, older official row second (higher id = newer).
        sqlx::query("INSERT INTO pricing_snapshots (provider_slug, model, source, price_json) VALUES ('opencode', 'opencode/shared-m', 'models.dev 2026-10-02', '{\"prompt_per_1k\": 99, \"completion_per_1k\": 99}')")
            .execute(&pool).await.expect("scraped seed");
        sqlx::query("INSERT INTO pricing_snapshots (provider_slug, model, source, price_json) VALUES ('commandcode', 'commandcode/shared-m', 'official page', '{\"prompt_per_1k\": 5, \"completion_per_1k\": 25}')")
            .execute(&pool).await.expect("official seed");
        // Source ranks: official/manual win over recency.
        assert_eq!(source_rank("official page"), 1);
        assert_eq!(source_rank("manual"), 1);
        assert_eq!(source_rank("Manual PUT"), 1);
        assert_eq!(source_rank("models.dev 2026-10-02"), 0);
        assert_eq!(source_rank(""), 0);
        // Bare lookup from a third provider resolves to the official row.
        let hit = lookup_bare_price(&pool, "antigravity/shared-m").await.expect("bare hit");
        assert_eq!(hit.1, "official page");
        assert!(hit.0.contains('5'));
        // Unknown slugs stay unmatched.
        assert!(lookup_bare_price(&pool, "antigravity/ghost-m").await.is_none());
    }

    #[tokio::test]
    async fn honesty_counts_split_priced_and_unpriced() {
        let pool = SqlitePool::connect("sqlite::memory:").await.expect("memory pool");
        sqlx::query("CREATE TABLE usage_events (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, account_id TEXT, model TEXT NOT NULL, prompt_tokens INTEGER NOT NULL DEFAULT 0, completion_tokens INTEGER NOT NULL DEFAULT 0, cached_tokens INTEGER NOT NULL DEFAULT 0, cache_creation_tokens INTEGER NOT NULL DEFAULT 0, estimated_cost REAL, cache_savings REAL NOT NULL DEFAULT 0, cost_source TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(&pool).await.expect("usage table");
        // Priced (incl. explicit 0.0 free-tier/manual-zero = known price),
        // unpriced (NULL = unknown, never $0.00), across providers.
        sqlx::query("INSERT INTO usage_events (provider_slug, account_id, model, prompt_tokens, completion_tokens, estimated_cost, cost_source) VALUES ('opencode', 'a', 'm', 10, 5, 1.0, 'exact'), ('opencode', 'a', 'm', 10, 5, 0.0, 'exact'), ('opencode', 'a', 'm', 10, 5, NULL, NULL), ('claude', 'b', 'm', 10, 5, NULL, NULL)")
            .execute(&pool).await.expect("usage seed");
        assert_eq!(priced_counts(&pool, "", "").await, (2, 2));
        assert_eq!(priced_counts(&pool, "opencode", "").await, (2, 1));
        assert_eq!(priced_counts(&pool, "opencode", "a").await, (2, 1));
        assert_eq!(priced_counts(&pool, "claude", "").await, (0, 1));
    }

    #[test]
    fn pricing_sync_singleflights_and_skips_while_fresh() {
        // First claim wins; concurrent claim loses; finish releases.
        assert!(claim_sync());
        assert!(!claim_sync());
        finish_sync(true);
        // Fresh dataset (<1h) skips without network.
        assert!(!claim_sync());
        // A failed sync releases the slot without marking fresh.
        // (Simulate expiry by resetting the timestamp back.)
        if let Ok(mut state) = sync_state().lock() {
            state.last_ok = Some(std::time::Instant::now() - std::time::Duration::from_secs(PRICING_FRESH_SECS + 1));
        }
        assert!(claim_sync());
        finish_sync(false);
        assert!(claim_sync());
        finish_sync(false);
    }

    #[test]
    fn variant_suffixes_map_to_base_while_price_changing_ones_do_not() {
        // Suffixes that denote the same model share its rate …
        assert_eq!(strip_variant("gemini-2.5-flash-thinking"), Some("gemini-2.5-flash".to_string()));
        assert_eq!(strip_variant("gemini-3-flash-agent"), Some("gemini-3-flash".to_string()));
        assert_eq!(strip_variant("gemini-3.6-flash-tiered"), Some("gemini-3.6-flash".to_string()));
        assert_eq!(strip_variant("gemini-3.1-pro-high"), Some("gemini-3.1-pro".to_string()));
        assert_eq!(strip_variant("gemini-3.5-flash-extra-low"), Some("gemini-3.5-flash".to_string()));
        assert_eq!(strip_variant("tencent/hy3-paid"), Some("tencent/hy3".to_string()));
        // … while suffixes that denote different SKUs never strip …
        assert_eq!(strip_variant("deepseek-v4-flash-fast"), None);
        assert_eq!(strip_variant("gpt-5.4-pro"), None);
        assert_eq!(strip_variant("gemini-2.5-flash-lite"), None);
        assert_eq!(strip_variant("gpt-5.4-mini"), None);
        assert_eq!(strip_variant("gemini-3-flash"), None);
        assert_eq!(strip_variant("low"), None);
        // … and free-tier naming is detected separately.
        assert!(is_free_tier_id("poolside/laguna-s-2.1-free"));
        assert!(is_free_tier_id("x:free"));
        assert!(!is_free_tier_id("gemini-3-flash"));
        assert!(!is_free_tier_id("freezer-m"));
    }

    #[tokio::test]
    async fn fill_missing_maps_variants_and_free_suffixes() {
        let pool = SqlitePool::connect("sqlite::memory:").await.expect("memory pool");
        sqlx::query("CREATE TABLE model_cache (provider_slug TEXT NOT NULL, native_id TEXT NOT NULL, context_length INTEGER, endpoints TEXT NOT NULL DEFAULT '[\"chat\"]', updated_at TEXT, source TEXT NOT NULL DEFAULT 'live', PRIMARY KEY (provider_slug, native_id))")
            .execute(&pool).await.expect("cache table");
        sqlx::query("CREATE TABLE pricing_snapshots (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, model TEXT NOT NULL, source TEXT NOT NULL, price_json TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(&pool).await.expect("pricing table");
        sqlx::query("CREATE TABLE usage_events (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, account_id TEXT, model TEXT NOT NULL, prompt_tokens INTEGER NOT NULL DEFAULT 0, completion_tokens INTEGER NOT NULL DEFAULT 0, cached_tokens INTEGER NOT NULL DEFAULT 0, cache_creation_tokens INTEGER NOT NULL DEFAULT 0, estimated_cost REAL, cache_savings REAL NOT NULL DEFAULT 0, cost_source TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(&pool).await.expect("usage table");
        sqlx::query("INSERT INTO model_cache (provider_slug, native_id) VALUES ('antigravity', 'gemini-2.5-flash-thinking'), ('antigravity', 'some-free-thing:free'), ('antigravity', 'chat_20706'), ('antigravity', 'deepseek-v4-flash-fast')")
            .execute(&pool).await.expect("cache seed");
        // Dataset knows only the base model.
        let doc = serde_json::json!({
            "google": {"models": {"gemini-2.5-flash": {"cost": {"input": 0.3, "output": 2.5}}}},
        });
        let report = fill_missing(&pool, &parse_models_dev(&doc)).await;
        assert_eq!(report.inserted, 2);
        assert_eq!(report.unmatched.len(), 2);
        // Variant row carries the base mapping in its source …
        let variant: (String, String) = sqlx::query_as("SELECT price_json, source FROM pricing_snapshots WHERE model = 'gemini-2.5-flash-thinking' ORDER BY id DESC LIMIT 1")
            .fetch_one(&pool).await.expect("variant row");
        assert!(variant.0.contains("0.0003"), "unexpected price {}", variant.0);
        assert!(variant.1.contains("~gemini-2.5-flash"), "unexpected source {}", variant.1);
        // … free-suffix row is an explicit zero, and opaque/fast ids stay out.
        let free: (String, String) = sqlx::query_as("SELECT price_json, source FROM pricing_snapshots WHERE model = 'some-free-thing:free' ORDER BY id DESC LIMIT 1")
            .fetch_one(&pool).await.expect("free row");
        assert!(free.0.contains('0'));
        assert!(free.1.contains("free suffix"));
    }

    #[tokio::test]
    async fn backfill_prices_cache_legs_and_records_savings() {
        let pool = SqlitePool::connect("sqlite::memory:").await.expect("memory pool");
        sqlx::query("CREATE TABLE pricing_snapshots (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, model TEXT NOT NULL, source TEXT NOT NULL, price_json TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(&pool).await.expect("pricing table");
        sqlx::query("CREATE TABLE usage_events (id INTEGER PRIMARY KEY AUTOINCREMENT, provider_slug TEXT NOT NULL, account_id TEXT, model TEXT NOT NULL, prompt_tokens INTEGER NOT NULL DEFAULT 0, completion_tokens INTEGER NOT NULL DEFAULT 0, cached_tokens INTEGER NOT NULL DEFAULT 0, cache_creation_tokens INTEGER NOT NULL DEFAULT 0, estimated_cost REAL, cache_savings REAL NOT NULL DEFAULT 0, cost_source TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now')))")
            .execute(&pool).await.expect("usage table");
        // 1000 prompt (700 cached read, 100 creation) + 100 completion.
        sqlx::query("INSERT INTO usage_events (provider_slug, account_id, model, prompt_tokens, completion_tokens, cached_tokens, cache_creation_tokens, estimated_cost) VALUES ('opencode', 'a', 'm', 1000, 100, 700, 100, NULL)")
            .execute(&pool).await.expect("usage seed");
        // No cache rates: identical to the old two-rate math, zero savings.
        backfill_costs(&pool, "opencode", "m", 1.0, 4.0, None, None).await;
        let row: (Option<f64>, f64) =
            sqlx::query_as("SELECT estimated_cost, cache_savings FROM usage_events WHERE model = 'm'")
                .fetch_one(&pool).await.expect("row");
        assert_eq!(row.0, Some(1.4));
        assert_eq!(row.1, 0.0);
        // Discounted read leg: cost drops, savings record the difference.
        sqlx::query("UPDATE usage_events SET estimated_cost = NULL, cache_savings = 0, cost_source = NULL")
            .execute(&pool).await.expect("reset");
        backfill_costs(&pool, "opencode", "m", 1.0, 4.0, Some(0.1), Some(1.25)).await;
        let row: (Option<f64>, f64) =
            sqlx::query_as("SELECT estimated_cost, cache_savings FROM usage_events WHERE model = 'm'")
                .fetch_one(&pool).await.expect("row again");
        // 200*1.0 + 700*0.1 + 100*1.25 + 100*4.0 = 0.795; savings 700*(1.0-0.1) = 0.63.
        assert!((row.0.unwrap() - 0.795).abs() < 1e-9, "cost {:?}", row.0);
        assert!((row.1 - 0.63).abs() < 1e-9, "savings {}", row.1);
    }
}
