-- 007: pricing history that 005 promised but its UNIQUE index blocked.
--
-- 005's comment says PUT appends a row and reads take latest-wins per
-- (provider_slug, model) — but idx_pricing_lookup is UNIQUE, so every price
-- update for an existing pair fails with 502 "database unavailable".
-- Replace it with a plain lookup index; reads already do
-- ORDER BY id DESC LIMIT 1, and PUT backfills NULL-cost usage rows.
DROP INDEX IF EXISTS idx_pricing_lookup;
CREATE INDEX IF NOT EXISTS idx_pricing_lookup ON pricing_snapshots(provider_slug, model);
