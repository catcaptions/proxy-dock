-- 011: verify-before-save routing gate.
--
-- Pasted ChatGPT/Antigravity OAuth credentials store without live
-- verification; such accounts must stay ineligible for routing until a quota
-- refresh validates them (see db::mark_account_verified/unverified and
-- is_account_routable). Browser OAuth and key verifies store verified = 1.
-- Pre-011 rows predate the gate (all came through verified flows), so the
-- default keeps them eligible. Never edit applied migrations (001-010);
-- this file is the only schema change in this batch.
ALTER TABLE accounts ADD COLUMN verified INTEGER NOT NULL DEFAULT 1;
