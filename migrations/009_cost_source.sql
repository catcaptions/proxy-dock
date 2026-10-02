-- 009: cost provenance on usage rows.
--
-- Pricing is shared by bare model slug across providers (the same model
-- served via antigravity or commandcode is the same pricing), with an exact
-- (provider, model) row winning over a bare-slug fallback. When a provider
-- records its own price later, its PUT backfill must overwrite rows that
-- were previously filled from another provider's price — but never rows
-- priced exactly. cost_source tracks that: 'exact' | 'bare' | NULL unknown.
ALTER TABLE usage_events ADD COLUMN cost_source TEXT;
