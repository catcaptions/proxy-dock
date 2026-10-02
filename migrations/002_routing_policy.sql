ALTER TABLE accounts ADD COLUMN credential TEXT;
ALTER TABLE accounts ADD COLUMN plan TEXT;
CREATE TABLE IF NOT EXISTS routing_policy (
  provider_slug TEXT NOT NULL,
  account_id TEXT NOT NULL REFERENCES accounts(id),
  priority INTEGER NOT NULL DEFAULT 1,
  enabled INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (provider_slug, account_id)
);
CREATE INDEX IF NOT EXISTS idx_usage_events_provider_created ON usage_events(provider_slug, created_at);
CREATE INDEX IF NOT EXISTS idx_quota_observations_account ON quota_observations(account_id, observed_at);
