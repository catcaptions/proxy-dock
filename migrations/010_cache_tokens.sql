-- 010: cache-aware usage legs on usage rows.
--
-- Cost is gateway-observed input + output tokens, but some upstreams bill
-- cache reads/writes at their own rates (OpenAI `cached_tokens`, Codex
-- `input_tokens_details`, Anthropic cache legs, Gemini `cachedContentTokenCount`,
-- Command Code `inputTokenDetails`). `prompt_tokens` stays TOTAL input (OpenAI
-- convention); the cache legs are subsets of it, so uncached input derives as
-- prompt - cached - creation (clamped at zero, never negative).
-- `cache_savings` is computed alongside cost at record time (it needs the rate
-- table, like t3code's): cached*(input - read), never negative.
-- Old rows default to zero legs, so the cost formula reduces exactly to the
-- pre-010 two-rate math for them.
ALTER TABLE usage_events ADD COLUMN cached_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usage_events ADD COLUMN cache_creation_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE usage_events ADD COLUMN cache_savings REAL NOT NULL DEFAULT 0;
