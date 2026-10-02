-- 008: catalog provenance for usage-observed models.
--
-- Providers without a documented catalog endpoint (ChatGPT/Codex) can never
-- fill model_cache via live refresh. Successful serves now record the model
-- as `observed` (see catalog::note_observed); provider-listed rows stay
-- `live`. Live rows are never touched by the observed path, so refresh
-- staleness semantics are unchanged. All pre-existing rows came from live
-- refreshes, hence the default.
ALTER TABLE model_cache ADD COLUMN source TEXT NOT NULL DEFAULT 'live';
