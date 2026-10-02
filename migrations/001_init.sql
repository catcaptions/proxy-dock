CREATE TABLE IF NOT EXISTS providers (
  slug TEXT PRIMARY KEY,
  label TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS accounts (
  id TEXT PRIMARY KEY,
  provider_slug TEXT NOT NULL REFERENCES providers(slug),
  label TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS model_cache (
  provider_slug TEXT NOT NULL,
  native_id TEXT NOT NULL,
  context_length INTEGER,
  PRIMARY KEY (provider_slug, native_id)
);
CREATE TABLE IF NOT EXISTS usage_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  provider_slug TEXT NOT NULL,
  account_id TEXT,
  model TEXT NOT NULL,
  prompt_tokens INTEGER NOT NULL DEFAULT 0,
  completion_tokens INTEGER NOT NULL DEFAULT 0,
  estimated_cost REAL,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS pricing_snapshots (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  provider_slug TEXT NOT NULL,
  model TEXT NOT NULL,
  source TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS quota_observations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  provider_slug TEXT NOT NULL,
  account_id TEXT,
  remaining TEXT,
  reset_at TEXT,
  observed_at TEXT NOT NULL DEFAULT (datetime('now'))
);
