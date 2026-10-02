# PROXY DOCK - CODE CHECK RESULTS

This text uses ASD-STE100 Simplified Technical English.
Each sentence has a maximum of 20 words.

## 1. PURPOSE

1. This document gives the results of the code check.
2. Four agents examined the backend, frontend, data store, and security.
3. They also searched the internet for correct practice.
4. They compared your code with other public projects.

## 2. GENERAL RESULT

1. Your code is clear about its sources and reasons.
2. Most problems are small and you can repair them.
3. Do not change the design.
4. Keep all workarounds in chapter 6.
5. They are correct and necessary.

## 3. DEFECTS - STOP LEVEL

### 3.1 Defect 1 - Wrong plan stops all accounts

1. Location: adapters/mod.rs, gateway/router.rs.
2. The code stops on all error 403.
3. A bad key stops the search for a good key.
4. This causes a valid request to fail.
5. Change the code to continue on plan error 403.
6. Stop only on password error 401.

NOTE: Plan error means the key is valid but the plan is wrong.

### 3.2 Defect 2 - No password on the local gateway

1. Location: gateway/router.rs, lib.rs.
2. Any local program can send requests without a password.
3. Any local program can read, add, and delete accounts.
4. The Origin check does not stop local programs.
5. Add a local password for all /api/* requests.
6. Store the password in the OS vault.
7. Give the password to the UI through Tauri IPC only.

WARNING: Do not open the gateway to the network before you add this password.

### 3.3 Defect 3 - GET request changes data

1. Location: gateway/router.rs, api/models.
2. GET with refresh=1 starts a live fetch upstream.
3. It uses the stored key and burns quota.
4. An attacker page can trigger it with an image tag.
5. Change refresh to POST only.
6. Or apply the Origin check to these GET requests.

## 4. DEFECTS - HIGH LEVEL

### 4.1 Defect 4 - Stream breaks on slow clients

1. Location: gateway/router.rs stream tasks.
2. The code uses try_send with a small queue.
3. A slow client fills the queue.
4. Then the code ends the stream without DONE.
5. The upstream server continues and burns tokens.
6. Use send with wait instead of try_send.
7. Stop the upstream task when the client leaves.
8. Add SSE keep-alive.

### 4.2 Defect 5 - Old events never delete

1. Location: gateway/router.rs serve function.
2. The code uses AND instead of OR.
3. A row must be old AND beyond 10000 rows to delete.
4. This causes the table to grow without limit.
5. Change AND to OR.
6. Add the same limit to usage, quota, and price tables.

### 4.3 Defect 6 - Slug move misses three tables

1. Location: db.rs migrate function.
2. The code moves four tables to the new slug.
3. It does not move model_cache, pricing, and events.
4. Old catalog and price data become invisible.
5. Move all seven tables in one transaction.
6. Write a repair job for old databases.

### 4.4 Defect 7 - Migration 005 fails on orphan rows

1. Location: migrations/005_accounts_pk.sql.
2. The code copies routing_policy as is.
3. An orphan row has no parent account.
4. SQLite stops the full migration on such a row.
5. Delete orphan rows before the copy.
6. Test with a database that has orphan rows.

### 4.5 Defect 8 - Frontend test and store problems

1. Location: AccountBlock.tsx, Home.tsx, auth.ts, hooks.ts.
2. Test IDs use account ID only, without provider.
3. The same email on two providers gives the same ID.
4. This makes tests unstable. Add provider to the ID.
5. The code reads session first in one place and local first in another place.
6. This can restore an old token. Use one read order.
7. Home sends nine requests for each range change.
8. There is no abort for old requests. Add AbortController.
9. Only Home shows the offline banner. Add it to provider pages.
10. Command Code callback fails when window port is empty.
11. Repair the URL for port 80 and 443.

## 5. DEFECTS - MEDIUM LEVEL

1. Antigravity finds the project on each chat and tries three hosts. Store the project for one hour. Fail fast to the next account. Location: antigravity.rs, router.rs.
2. Two chats with an old token refresh at the same time. The second refresh fails. Use one lock per account. Location: router.rs, catalog.rs.
3. The log shows the default account, not the account that served. Send the serving ID back to the log. Location: router.rs.
4. The code adds model to native Claude and Codex messages. These formats have no model field. Add model only to OpenAI format. Location: router.rs.
5. The code hides stream errors and sends stop. The client cannot retry. Send an error chunk or abort. Location: claude.rs, chatgpt.rs.
6. ChatGPT drops image parts and sends text only. Stop and send honest error 400 like Claude. Location: chatgpt.rs.
7. Each request reads the vault many times. Store presence for 30 seconds. Location: secrets.rs, router.rs.
8. Shared price uses the newest row from any provider. This can pick the wrong source. Prefer official and manual sources first. Location: router.rs, pricing.rs.
9. Time filters go into SQL as text. This is safe today but fragile. Use bound parameters. Location: router.rs.
10. The Antigravity secret is in the code. Move it to environment only and rotate it. Location: oauth.rs, quota.rs, dev-oauth.ts.
11. CSP is null and Tauri API is global. Any script error gives full IPC access. Enable CSP and limit IPC rights. Location: tauri.conf.json.
12. Wincred delete leaves data chunks when the manifest is gone. Add a full sweep for orphan chunks. Location: wincred.rs.
13. Pasted ChatGPT and Antigravity logins store without check. Mark them unverified and block them from serve until quota check passes. Location: router.rs.
14. Errors miss the provider field on hot paths. Add provider to all serve errors. Location: router.rs.
15. No Host check exists. Add a Host allow list for localhost and 127.0.0.1 on all requests.

## 6. MISSING FUNCTIONS

1. Add a route editor in the UI. The backend already supports it.
2. Add export and import for data. Do not include secrets.
3. Add request cancel, body size limit, and timeout per route.
4. Add delete rules for quota history.
5. Add busy timeout to SQLite with WAL mode.
6. Show priced and unpriced request counts. Do not show $0.00 for unknown cost.
7. Add a vault health check. Show NoEntry separate from read error.
8. Use one lock for price sync. Add ETag cache for api.json.
9. Send honest error 400 for embeddings, images, and files.
10. Save tray settings to disk. Add start minimized and stop from tray.
11. Add packaging, signing, updater, and SBOM.
12. Show last reconciled time for cost against Studio truth.

## 7. WORKAROUNDS - DO NOT REMOVE

1. Vault split into 512 byte chunks is correct. The OS limit is near 2.5 kB. OAuth JSON is larger. Keep the manifest last design.
2. Flat /:slug/v1/* route table is correct. Axum nest() removes the prefix and loses the slug. Keep the current table and its test.
3. Npm version lookup for Codex is correct. The catalog needs an official CLI version. You do not spoof it. Keep strict parse and cache.
4. User-Agent antigravity and prod first order is correct. The internal endpoints reject generic agents. Keep it.
5. Exact Command Code callback copy is correct. Studio expects that exact shape and headers. Keep it byte for byte.
6. Date text YYYY-MM-DD HH:MM:SS with normalize is correct. SQLite compares text correctly only in this shape. Keep it, but use bound parameters.
7. Child before parent in migration 005 is correct. SQLite has no ALTER PRIMARY KEY. Keep the order.
8. Price history append with latest wins is correct. Keep it.
9. Shared price by bare slug is correct. The same model has the same price. Keep exact wins, add source rank.
10. Suffix strip for -thinking and -agent is correct. Do not strip -pro, -lite, -max, and -fast. They change the price. Keep the deny list.
11. Observed catalog is correct. Codex has no official list. Keep live rows separate from observed rows.
12. ThoughtSignature in call ID is correct. Gemini needs it back. The ~ sign is safe. Keep it.
13. Vite host 127.0.0.1 is correct. Studio calls back on 127.0.0.1, not ::1. Keep it.
14. Auto capture with paste fallback is correct. Ports can be busy. Keep both paths.
15. No reuse of keys across providers is correct. It respects provider terms. Keep it.

## 8. REFERENCES

1. Axum nest and Path: docs.rs/axum, github.com/tokio-rs/axum.
2. Axum SSE KeepAlive: docs.rs/axum/.../sse/struct.KeepAlive.html.
3. OpenAI chat stream and Responses stream: platform.openai.com/docs, developers.openai.com/api.
4. Anthropic stream and auth: platform.claude.com/docs.
5. Gemini streamGenerateContent: ai.google.dev/api.
6. OAuth native apps RFC 8252: rfc-editor.org/rfc/rfc8252.html.
7. Tauri security and opener: tauri.app/security, v2.tauri.app/plugin/opener.
8. DNS rebind and Host check: modelcontextprotocol hostHeaderValidation, CVE-2026-11624.
9. Windows CredWriteW limit: learn.microsoft.com/.../wincred.
10. SQLite dates and keys: sqlite.org/lang_datefunc.html, sqlite.org/foreignkeys.html.
11. sqlx migrate: docs.rs/sqlx, github.com/launchbadge/sqlx/issues/2085.
12. models.dev schema: models.dev, github.com/vercel/models.dev.
13. EasyCLIProxyAPI and CLIProxyAPI auth: github.com/router-for-me/EasyCLIProxyAPI.

## 9. COMMANDCODE GO SUBAGENT EMPTY STREAMS (2026-10-02)

1. Subagents received empty streams on the CommandCode Go route.
2. The main agent mostly worked, subagents mostly failed.
3. The caller raises EmptyStreamError after one chunk and no finish.
4. Live logs show status 200, 40 bytes, and one chunk.
5. The gateway sends the role chunk before it reads upstream.
6. On early upstream error it returns with no finish and no DONE.
7. The database records these as 200 ok, so they stay invisible.
8. Parallel and sequential subagents both receive empty streams.
9. Parallelism raises the count, but it is not the cause.
10. The defect fires on one single request alone.
11. One account cannot roll to a different account.
12. The harness still fans out children unless limited to one.
13. Set caller parallelism to one child at a time.
14. This lowers the count but does not remove the defect.
15. Fix the gateway to send the error as content, then finish.
16. Log the upstream error text with each failure.
17. Limit parallel requests per account with honest 429.
18. Use send with wait instead of try_send on full queues.
19. Keep the first error, do not overwrite it with empty.
20. Add the provider name to all failed-request errors.
21. Move vault reads off the hot path or read once.
22. Never return 200 with empty text and empty tools.
23. Full detail lives in future-fixes.md in this folder.

## 10. FIX STATUS (2026-10-03)

1. Chapter 9 incident fixes are done and tested.
2. The gateway no longer returns bare role-only streams.
3. Upstream errors surface as content plus finish.
4. Streams log ok or truncated at completion.
5. Per-account limits return honest 429 with Retry-After.
6. Slow clients apply backpressure instead of truncation.
7. Thought-only replies return honest 502, never 200-empty.
8. Plan-shaped 403 errors roll, credential errors stop.
9. Refresh races use one lock per account.
10. Vault reads leave the hot path.
11. Vision parts return honest 400 everywhere.
12. Prices prefer official and manual sources.
13. Usage reports priced and unpriced request counts.
14. New pastes stay ineligible until quota validates them.
15. The gateway requires one unified key on all routes.
16. The key lives in the vault, never in logs.
17. External clients must send the key now.
18. Health probes stay open without a key.
19. The suite shows 129 passed and zero warnings.
20. Open items remain in future-fixes.md with empty boxes.
