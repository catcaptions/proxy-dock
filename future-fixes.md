# Future fixes — Proxy Dock

Two sources, one list. §1 is the live 2026-10-02 incident (diagnosed, unfixed).
§2–§5 are the 4-agent audit backlog, deduplicated against §1 (items marked
**[incident]** overlap the live failure and fix together). §6 lists intentional
workarounds — do not "fix" those. Line refs are from audit time and may have
shifted a few lines after the Proxy Dock rename; file names are unchanged.

Status marks: `[ ]` open, `[/]` in progress, `[x]` done.

## 1. Active incident (2026-10-02): CommandCode Go subagent empty streams

Symptom: subagents calling `http://127.0.0.1:11434/commandcode/v1` (Go plan,
`gpt-6-luna`) fail with caller-side `EmptyStreamError`; the main agent mostly
succeeds. Parallel fan-out happens by harness design (`max_concurrent_children:
10`) onto a single Go account with no roll candidate.

Mechanism (proven from caller logs + code):
1. The gateway sends the SSE `role` chunk immediately
   (`src-tauri/src/gateway/router.rs:1653`), before reading any upstream byte.
2. When the first upstream event is an error (`success:false` / `error`), two
   branches do a bare `return` (`router.rs:1687-1698`): no finish chunk, no
   `[DONE]`, no usage row.
3. The caller gets HTTP 200 + ~40 bytes + 1 chunk, no `finish_reason` → raises
   `EmptyStreamError` (`hermes-agent/agent/chat_completion_helpers.py:3327`),
   retries 3× stream attempts × 3 empty-retries per child, then stamps `(empty)`.
4. `serve_with_event_log` commits the 200 at headers time, so the DB shows
   `ok` — 553×200, 0×429/5xx on `commandcode|chat/completions`. Failures are
   invisible server-side.
5. Caller evidence (`%LOCALAPPDATA%\hermes\logs\agent.log`): 9 subagent ids,
   `depth=1: 121` vs `depth=0: 9`; main vs subagent requests identical
   (endpoint, model, stream, max_tokens, tools). Serial subagents fail too
   (user-observed): parallelism is a multiplier, not the cause. The defect
   fires on a single request alone — fix the gateway first; the caller
   parallelism knob only lowers the count.

Ruled out for this incident: thought-only 200-empty replies (all 418
commandcode usage rows have `completion_tokens > 0`).

## 1b. Follow-up (2026-10-03): `input.arguments` rejection + dead turns

New symptom, same turn shape: `[upstream error] Missing required parameter:
'input[24].arguments'` (DB `gateway_events` 633/636, `truncated|200` with
snippet — the §1 surfacing working as intended). Failing shape: history
`tool-call` parts carried flat `input:{}` with no `arguments` key (empty,
omitted, or unparsable model arguments all collapsed to `{}`), and zero-arg
tools declared `input_schema:{}`. First turn (tools only) succeeded; every
replay turn with history calls failed validation. The user call: the model's
bad call is the model's fault — but the turn must not die there.
- [x] Translation fix: every `tool-call` part emits `arguments` (JSON string,
  `"{}"` minimum) alongside `input`; missing/non-object/empty `parameters`
  becomes `{"type":"object"}`. Regression test mirrors the gmail shape.
- [x] Peek-first streams: `bridge_stream_response` reads the first upstream
  event before committing to 200 SSE (180s cap, 1MB preamble cap). Pre-content
  inline/translate errors return rollable `Next`/terminal `Stop` (HTTP 502
  envelope) so the caller retries and the user still gets a response; the
  validated first event is translated up front and replayed after the role
  chunk. Post-content errors keep the emit-as-content path.

Fixes (all done 2026-10-03 — wave 1 + auth slice, 129 cargo tests green):
- [x] Pre-content upstream errors no longer bare-return. In both
  `if !content_started { return; }` branches, emit the error as a content
  delta then call `finish_and_record(&acc)` (same as the post-content path).
- [x] Upstream snippet flows into `log_event`/`error_snippet` (redacted); streams
  log `ok` vs `truncated` at completion
- [x] Per-account in-flight cap with honest `429 + Retry-After` (exact upstream
  value forwarded where captured)
- [ ] Caller (user action): `delegation.max_concurrent_children: 10 → 1`; optionally pin
  child provider/model to a non-Go route.

Verification: 3 parallel streaming subagent-shaped requests against
`/commandcode/v1` on one Go account — every stream must end with finish +
`[DONE]` or a JSON error, never role-only close. Add a regression test for
pre-content `success:false`; `cargo test`; `tsc --noEmit`.

## 2. P0 — will lose requests / money

### 2.1 Plan-mismatch 403 stops rolling instead of skipping
`adapters/mod.rs:68-70`, `adapters/commandcode.rs`, `adapters/opencode.rs`,
`gateway/router.rs` (route_chat). `is_auth = 401|403` → terminal `Stop`, so a
legacy `plan=None` Go/Free key ahead of a valid account aborts the whole chain
instead of rolling. Repro: `None`-plan key priority 0 + valid key → upstream
403 ends the chain, valid account never tried.
- [x] Done 2026-10-03: plan-shaped 403 bodies roll (`Next`), credential
  401/403 stays terminal — shared `is_plan_403` + per-adapter wiring + tests.

### 2.2 Unified gateway key — DONE 2026-10-03 (replaces bearer proposal)
One secret ("Proxy Dock key" = app password = API bearer). Install default
is the well-known value `proxy-dock` (stored on first run; user-set/rotate in
Settings replaces it); rotate issues a fresh 32-byte base64url key. `gate_middleware` requires
`Authorization: Bearer` (constant-time compare, fail-closed) on all
`/v1/*`, `/:slug/v1/*`, `/api/*` except `/health` (+ preflight); wrong/
missing → 401 `proxy_dock_unauthorized`. Origin/Referer mutating gate kept
behind it. `PROXYDOCK_GATEWAY_KEY` env override for headless/preview.
Frontend attaches the bearer everywhere (desktop: IPC, memory-only; preview:
sessionStorage); Settings has masked Copy + confirm-gated Rotate + Set.
- [x] External clients must now send the key (hermes/opencode configs,
  scripts) — only `/health` stays open. Rotate breaks old configs by design.

### 2.3 State-changing GETs — COVERED by the §2.2 bearer gate
Live upstream fetches on model/catalog GETs now require the key, which
browsers cannot send cross-origin — the CSRF/rebind class is closed.
- [x] Done via bearer enforcement (no separate POST-for-refresh needed).

## 3. P1 — likely buggy / fragile

### 3.1 SSE truncation — DONE via §1 fixes (`send().await` backpressure,
`KeepAlive`, disconnect abort, terminal `[DONE]` + synthesized usage).
`gateway/router.rs` all five stream fns (`mpsc::channel(32)` +
`tx.try_send().is_ok()`, no `keep_alive`, no disconnect select). Slow client
→ `false` → `return` with no `[DONE]`; `try_send` also fires on a merely full
buffer, not just disconnects. `generate_once` buffers the full non-stream
body with no client-cancel; upstream keeps burning paid tokens after
disconnect. Per Axum docs: `tx.send().await` + disconnect select +
`KeepAlive::default()`.
- [ ] Bounded `send().await` + `is_closed()` disconnect abort in all stream
  fns; `KeepAlive`; abort the upstream poll on disconnect.

### 3.2 Event retention `AND` should be `OR` — unbounded growth
`gateway/router.rs` prune query: rows must be both beyond newest-10k AND older
than 30d to delete. Busy months never prune; quiet DBs keep ancient rows.
`usage_events` / `quota_observations` / `pricing_snapshots` have no cap at all.
- [x] Done 2026-10-03: `OR` + per-table caps (events 10k, usage 50k, quota
  10k, pricing 5k); account delete purges history rows.

### 3.3 Slug migration misses 3 tables
`src-tauri/src/db.rs` `migrate_commandcode_slug` rewrites 4 tables, misses
`model_cache` / `pricing_snapshots` / `gateway_events` → old-slug
catalog/prices/history invisible, old usage loses exact-price match. (Same
class of bug as the Proxy Dock rename — that one ships copy-forward shims;
this one still needs them.)
- [x] Done 2026-10-03: all 8 slug tables covered in a tx with count logging
  (idempotent repair); one-time leftover repair included.

### 3.4 Migration 005 fails on orphan policy rows
`migrations/005_accounts_pk.sql:37-38` copies `routing_policy` verbatim, but
`migrate!` runs in a tx where `PRAGMA foreign_keys=OFF` is a no-op — any orphan
`(provider,account)` aborts startup migration. Refs: sqlite.org/foreignkeys,
`launchbadge/sqlx#2085`.
- [x] Done 2026-10-03: guard recipe recorded as a comment at the migrator
  (migration already applied — no rebuild needed).

### 3.5 Frontend correctness batch — DONE 2026-10-03 (tsc clean, 10 specs green)
- [x] Testids namespaced (`account-{provider}-{id}` etc.; `qp-*` for the dense list).
- [x] Single storage helper (`getToken/setToken/clearToken`, guarded).
- [x] `AbortController` + in-flight dedup on usage fetches.
- [x] One shared `OfflineBanner` on Home and provider pages.
- [x] CommandCode empty-port URL fixed; `forgetAccount` null preserved;
  flow state synced; `linkingRef` finally-reset; `preciseBackIn` year guard;
  actions visible without hover; `progressbar` values; table `scope`;
  file-input reset; import applies routing + pricing with one terse line.
- [x] Routing editor UI: drag-to-reorder (grip, drop indicator, keyboard
  arrows) with enable toggle beside refresh/remove; no pN numbering. Label
  editing removed with the old cluster.

## 4. P2 — medium / correctness gaps

- [x] Antigravity project cache (per-account, TTL ~1h); serve path fails
  fast to the next account (full host loop kept for quota/catalog).
- [x] 401-refresh race: per-account singleflight locks in serving + catalog
  + quota; `refreshed_payload` merge drops `id_token`, keeps refresh.
- [ ] Attribution uses the default account, not the serving one
  (`router.rs` event log): pass the serving id back from `route_*`; add a
  `serving_account_id` column.
- [x] Native-wire `model` rewrite gated to OpenAI-shaped (`choices`) bodies.
- [x] In-stream errors surfaced via `*_checked` translators (no fabricated
  clean stops).
- [x] ChatGPT vision honest-400 gate; CommandCode image parts honest-400
  (mapping unverified).
- [x] Vault de-blocked: read-once per candidate via `spawn_blocking`.
- [x] Bare-slug price ranking (official/manual first).
- [x] Time bounds as bound params; doc fixed.
- [x] Skipped/failed/busy counters; first error wins.
- [x] Terminal `[DONE]` + synthesized usage always (incl. passthrough).
- [ ] Security hygiene (remainder): committed Antigravity secret → env-only
  + rotate (needs provider-side rotation); `CSP:null` + `withGlobalTauri`
  lock-down; `Host` allowlist. Done: wincred orphan sweep + target bound,
  `id_token` dropped on persist/merge, verify-before-save gating via
  migration 011 + candidate filter, `provider` on all-failed errors.

## 5. Missing — things to add

- [x] Routing editor UI: drag-to-reorder with enable toggle (backend already
  supported it).
- [ ] Export/import bundle (no backend route; frontend import now applies
  routing + pricing via existing endpoints and reports skips). Secrets stay
  vault-only; document machine scope.
- [ ] Request cancellation (done for streams: disconnect abort) + body
  limits (2MB default 413s tool-heavy agents) + per-route timeouts.
  KeepAlive + in-flight caps done.
- [x] `quota_observations` retention (newest 10k); SQLite `busy_timeout`
  (5s) with WAL.
- [x] `usage_summary` honesty counts (`priced/unpriced_requests` per total
  and group).
- [ ] Keyring health probe (`NoEntry` vs read-back mismatch); split
  `vault` vs `adapter_env` in `hasToken`.
- [x] Pricing sync singleflight + 1h freshness skip (still off request path;
  no ETag/disk cache yet).
- [ ] API contracts: explicit `POST /v1/embeddings|images|files` honest
  400s; `?beta=true` Claude OAuth test pin; per-provider `stale` in unified
  catalog. Done: per-provider image/file 400s.
- [ ] Tray persist + minimize-to-tray + start-minimized; packaging/signing/
  updater/SBOM; Studio cost/quota reconciliation (`last reconciled` + drift
  alert; `reset_at` currently NULL).
- [/] Tests: singleflight locks + bare-rank + `is_plan_403` + empty-bridge +
  inflight + checked-translator tests added. Still open: `[DONE]`-count
  byte tests, wincred roundtrip in CI (gated test exists), `normalize_bound`
  fuzz.

## 6. Quirks — intentional, keep (do not "fix")

Verified against docs and other repos (EasyCLIProxyAPI, commandcode-go-proxy,
antigravity-usage, codex-backend-sdk). Follow-ups noted where they exist.

| Quirk | Why it exists |
|---|---|
| wincred 512B chunking + manifest + read-back verify | Generic-credential blobs top ~2.5kB; OAuth JSON is 2–3kB; keyring 3.6 silently dropped writes. Follow-up: orphan-chunk GC only. |
| Flat `/:slug/v1/*` route table, no `nest()` | `nest()` strips the prefix without capturing; `Path(slug)` 500'd. Pinned by regression test. |
| npm-latest Codex `client_version` discovery | `/models` requires a CLI-release version; strict semver parse, override env, once-per-process, honest fallback. Never spoof. |
| `User-Agent: antigravity` + prod→daily→sandbox | Internal endpoints reject generic UAs; regional 400s served elsewhere. Same as the proven tool. |
| CommandCode CLI byte-for-byte callback mirror | Studio expects the exact shape/headers. Exactness is correctness. |
| `datetime('now')` TEXT + `normalize_bound` | Lexicographic compare needs one shape; normalize inward at the edge. Just bind params. |
| Migration child-before-parent rebuild | SQLite has no `ALTER PK`; sqlx runs migrations in a tx where `PRAGMA foreign_keys=OFF` is a no-op. |
| Append-history pricing + latest-wins read | Audit trail + trivial reads + NULL backfill. The old UNIQUE index was the bug (fixed). |
| Bare-slug shared pricing + `cost_source` | Same model via two providers is the same price absent provider data. Add source ranking, don't remove sharing. |
| `-thinking`/`-agent` suffix strip + denylist | Thinking bills at base rates; `-pro/-lite/-max/-fast` genuinely reprice. Extend only with rate-card evidence. |
| Observed catalog for catalog-less providers | Codex has no official list path; serve-proven IDs recorded as `observed`, `live` rows untouchable. |
| `thoughtSignature` smuggling in call ids | Gemini requires it back multi-turn; `~` is unambiguous. Watch id-length limits. |
| Go bridge behind `PROXYDOCK_GO_BRIDGE` flag | Private subscription route; instant kill-switch until proven. Single retry layer + gateway roll only. |
| Preview credential mirror (best-effort) | Gateway reads the keyring; preview lives in browser storage. `verified` flag + quota re-validation required. |
| No credential reuse / no dotfile hunting | Respects provider ToS; avoids file-theft patterns. Keep explicit. |

## 7. Verification log

- 2026-10-02: rename `proxy-hub` → `proxy-dock` (crate `proxy_dock`,
  `ai.proxydock.app`, `proxy-dock.db`, `PROXYDOCK_*`) with copy-forward
  shims for vault/DB/env/browser-storage. `tsc --noEmit` clean,
  `cargo test --lib` 99 passed after full `cargo clean` rebuild.
- 2026-10-03: wave 1 (incident framing, plan-403 roll, backpressure,
  in-flight caps, prune, singleflight, vision gates, bare-rank, honesty
  counts, verified-gating via migration 011, frontend batch incl. routing
  editor + sparklines) + leftover router wiring + unified gateway key
  (vault `gateway-key`, bearer gate, Settings Copy/Rotate/Set). `tsc`
  clean, `cargo test --lib` 129 passed, zero warnings. Playwright
  control-api specs 10 passed; screenshot baselines untouched.
- Visual snapshot baselines predate the rename and the quota UI — update
  only via explicit `test:visual:update`, never bundled with features.

## 8. References

Axum nesting/SSE docs; OpenAI chat + Responses streaming; Anthropic streaming
+ auth (`anthropic-version`, `oauth-2025-04-20`); Gemini
`streamGenerateContent`; RFC 8252 loopback/PKCE/state; Tauri security /
opener / GHSA-7gmj-67g7-phm9; tower-http CORS; MCP host validation /
CVE-2026-11624; WinCred/MS Learn + keyring crate; sqlx migrate + SQLite
rebuild recipe; models.dev schema; EasyCLIProxyAPI/CLIProxyAPI auth+storage;
`commandcode-go-proxy/commandcode_proxy.py`; antigravity-usage;
codex-backend-sdk; `hsjlyj/codex-proxy`; Claude OAuth proxies; hermes-agent
`agent/chat_completion_helpers.py`, `agent/turn_empty_response.py`,
`tools/delegate_tool_dispatch.py`.
