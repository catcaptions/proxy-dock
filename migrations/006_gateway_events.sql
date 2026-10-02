-- 006: gateway request-event log (2F).
--
-- One row per gateway request (all wires): powers Home/provider recent-error
-- and routing-activity panels. error_snippet is truncated and never carries
-- secrets. Retention: background task keeps the newest 10k rows within 30d.
-- Timestamps are UTC 'YYYY-MM-DD HH:MM:SS' (same convention as usage_events);
-- RFC-3339 conversion happens on read.

CREATE TABLE IF NOT EXISTS gateway_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  provider_slug TEXT NOT NULL,
  account_id TEXT,
  model TEXT NOT NULL DEFAULT '',
  route TEXT NOT NULL DEFAULT '',
  outcome TEXT NOT NULL DEFAULT '',
  status INTEGER NOT NULL DEFAULT 0,
  latency_ms INTEGER NOT NULL DEFAULT 0,
  error_snippet TEXT
);

CREATE INDEX IF NOT EXISTS idx_events_q ON gateway_events(provider_slug, created_at);
