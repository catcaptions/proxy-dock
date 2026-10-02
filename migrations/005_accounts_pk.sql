-- 005: composite account identity + credential purge + pricing/model-cache extension.
--
-- Rationale: accounts(id PK) collides when one identity (email) links two
-- providers (INSERT OR IGNORE in register_account silently drops the second).
-- Identity is the (provider_slug, id) pair from here on.
--
-- Recipe notes (load-bearing, do not reorder):
-- - SQLite has no ALTER TABLE .. PRIMARY KEY: rebuild via *_new tables.
-- - sqlx migrations run in a transaction where PRAGMA foreign_keys=OFF is a
--   no-op and FK enforcement is on, so drop the CHILD (routing_policy, the
--   only FK holder) before the parent (accounts).
-- - 001-004 are untouched (sqlx checksums).

CREATE TABLE accounts_new (
  id TEXT NOT NULL,
  provider_slug TEXT NOT NULL REFERENCES providers(slug),
  label TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 1,
  credential TEXT,
  plan TEXT,
  PRIMARY KEY (provider_slug, id)
);

-- Legacy `credential` column is purged: secrets live in the OS keyring only.
INSERT INTO accounts_new (id, provider_slug, label, priority, credential, plan)
  SELECT id, provider_slug, label, priority, NULL, plan FROM accounts;

CREATE TABLE routing_policy_new (
  provider_slug TEXT NOT NULL,
  account_id TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 1,
  enabled INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (provider_slug, account_id),
  FOREIGN KEY (provider_slug, account_id) REFERENCES accounts_new (provider_slug, id)
);

INSERT INTO routing_policy_new (provider_slug, account_id, priority, enabled)
  SELECT provider_slug, account_id, priority, enabled FROM routing_policy;

DROP TABLE routing_policy;
DROP TABLE accounts;
ALTER TABLE accounts_new RENAME TO accounts;
ALTER TABLE routing_policy_new RENAME TO routing_policy;

-- Pricing: manual snapshots only (no scraped prices). PUT appends a row;
-- reads take latest-wins per (provider_slug, model).
ALTER TABLE pricing_snapshots ADD COLUMN price_json TEXT NOT NULL DEFAULT '{}';
CREATE UNIQUE INDEX IF NOT EXISTS idx_pricing_lookup ON pricing_snapshots(provider_slug, model);

-- Model cache: live catalog storage (1A/2E). endpoints is a JSON array of
-- "chat" | "responses" | "messages"; updated_at is UTC 'YYYY-MM-DD HH:MM:SS'.
ALTER TABLE model_cache ADD COLUMN endpoints TEXT NOT NULL DEFAULT '["chat"]';
ALTER TABLE model_cache ADD COLUMN updated_at TEXT;
CREATE INDEX IF NOT EXISTS idx_cache_slug ON model_cache(provider_slug);

-- Usage query support (2A): pair-identity filters + time ranges.
CREATE INDEX IF NOT EXISTS idx_usage_q ON usage_events(provider_slug, account_id, model, created_at, id);
