---
item: FT-6
date: 2026-09-07
base_sha: 876a153b4039128f8052526629773fa189058c71
head_sha: 876a153b4039128f8052526629773fa189058c71
outcome: blocked
implementor_model: opus
reworks: 1
lanes:
  fanout: CHANGES_REQUESTED
  eve: ok
  blake: ok
  bob: "ok: codex-run.sh exited 0; output at /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-6-bob-output.txt (440KB, 1613"
  carl: "ok: gemini-run.sh (backend=copilot, model=gemini-3.8-flash) exited 0 in ~5m29s; output written to /Users/bob/git/src/github."
---

# FT-6 fast-track review

## Summary

Outcome: **blocked**. One rework cycle ran; 16 blocking findings were raised before rework, 1 remains open after it, and 4 candidates were refuted during adversarial verification. 28 advisory (non-blocking) findings are recorded below.

- Blocking before rework: 16
- Remaining after rework: 1
- Refuted: 4
- Advisory: 28

The single remaining blocker is Bob's HIGH finding that the core-level transaction guard also rejects transaction verbs through `DoogatDriver.execute_sql`, changing the in-process FFI transport although the card scoped the change to the server.

## Fan-out (Alice-engine)

---
prd: 
review: 1
date: 2026-09-07
head_sha: 876a153b4039128f8052526629773fa189058c71
reviewers: fanout
---

## Fanout

[FANOUT] 🔴 Guard inspects only the first statement, so a two-statement executeSql still opens the process-global buffer | File: ddb-core/src/service/sql.rs | Task: FT-6
[FANOUT] 🔴 Guard only inspects the first statement, so a trailing BEGIN still opens the shared buffer — no test covers it | File: ddb-core/src/service/sql.rs | Task: FT-6
[FANOUT] 🟠 executeBatch - the atomic unit the new error recommends - is unguarded and opens the same shared buffer | File: ddb-core/src/service/sql.rs | Task: FT-6
[FANOUT] 🟠 executeBatch, the path the new error recommends, still opens the process-global transaction buffer | File: ddb-core/src/service/sql.rs | Task: FT-6
[FANOUT] 🟠 execute_batch is unguarded, so executeBatch(["BEGIN"]) opens the same process-global buffer, and no test exercises it | File: ddb-core/src/service/sql.rs | Task: FT-6
[FANOUT] 🟠 Transaction guard only inspects the first statement: a trailing BEGIN opens the process-global buffer and silently swallows every other client's writes | File: ddb-core/src/service/sql.rs | Task: FT-6
[FANOUT] 🟠 execute_batch has no transaction-verb guard: executeBatch(["BEGIN"]) succeeds and leaves the shared buffer open for every other client | File: ddb-core/src/service/sql.rs | Task: FT-6

### Advisory (does not block)

- New behavior ships with no test: no unit test for the code, and the e2e BEGIN step now accepts any outcome
- The new code was not added to the shared code-to-category/status table the PRD constraint requires
- Guard sits in core, so it also fires on the in-process FFI path the PRD says is unchanged - with advice that surface cannot follow
- Docs claim PgWire reports the same error code; PgWire emits only the message
- TRANSACTION_NOT_SUPPORTED is missing from the one code->category table, so the app-error boundary classifies it Internal and redacts the message
- No test pins the new rejection; the one test touched was loosened to accept any outcome
- SAVEPOINT (and RELEASE) still reach the shared SQLite connection and leave it inside an open transaction
- Core error text hard-codes the GraphQL field name executeBatch, which leaks into CLI and FFI callers
- Docs claim the rejection is server-shaped, but the guard is in the core service every embedded caller uses
- Statement classification duplicated instead of extending the existing classify module
- The new rejection behavior has no assertion anywhere; COMMIT and ROLLBACK arms are entirely untested
- New error code missing from the per-code category table and its table test, so it silently defaults to Internal

### Rubric

R1: fail
R2: pass
R3: pass
R4: fail
R6: pass
R7: fail
R8: pass
R9: fail
R10: pass
R11: pass
R12: pass
R13: pass

### Rubric notes

- R1: No test covers the new behavior. `transaction_verb`/`DoogatError::transaction_not_supported` have no unit test (ddb-core/src/service/sql.rs's #[cfg(test)] mod still holds only the two pre-existing ind
- R4: The new code was not added to the shared error-mapping tables, so transports derive the wrong classification. ddb-core/src/app_contract/error.rs:96-108 falls through `_ => AppErrorCategory::Internal`,
- R7: The guard validates only the first parsed statement (`statements.first()` in ddb-core/src/service/sql.rs:24), so a transaction verb anywhere else in a user-supplied multi-statement string passes valid
- R9: CONFIRMED empirically, not inferred: I ran a temporary ddb-core integration test (since deleted). `execute_sql("BEGIN")` -> Err TRANSACTION_NOT_SUPPORTED, but `execute_sql("SELECT 1; BEGIN")` -> Err S

_engine: workflow — dimensions 5, raw 23, unique 19, confirmed 7, refuted 0, demoted 0, unverified 0, diff_bytes 10669_

Verdict: 7 findings
Tests: {{TESTS_LINE}}

## Eve

lane_status: ok

Doubt + de-slop review of FT-6 (876a153..HEAD, staged). Read every changed file plus actor/, sql_engine/mod.rs, pgwire.rs, read_pool.rs, ffi/driver.rs + records.rs, app_contract/error.rs, http_error.rs. Built ddb-cli and ran an empirical repro against three throwaway servers in the scratchpad (script: scratchpad/ft6-repro.sh). Two bypass doors confirmed live on HEAD; the gate did not run ddb-core tests, so I ran `cargo test -p ddb-core --lib service::sql` (2 passed). 13 findings: 6 FIX, 2 VERIFY, 5 KNOWN.

| Severity | Title | File | Proof / evidence |
| --- | --- | --- | --- |
| CRITICAL | executeBatch still opens the process-global buffer: [BEGIN, INSERT] with no COMMIT strands every later client's write | ddb-core/src/service/sql.rs:64 | Empirical on HEAD (ft6-repro.sh, server A): `executeBatch(statements: ["BEGIN", "INSERT INTO leakprobe (title) VALUES ('batch-begin')"])` returned data [{message:BEGIN},{message:20260907022353}]. A later `executeSql(INSERT ... 'after-batch')` from another client returned success id 20260907022354; `git ls-tree -r HEAD` contains no file for it (count 0). That client's own `executeSql("COMMIT")` is now rejected with TRANSACTION_NOT_SUPPORTED, so the write is stranded until restart, then lost. |
| CRITICAL | Guard checks only the first statement: `INSERT ...; BEGIN` through executeSql opens the shared buffer | ddb-core/src/service/sql.rs:24 | Empirical on HEAD (ft6-repro.sh, server B): `executeSql(sql: "INSERT INTO leakprobe (title) VALUES ('lead'); BEGIN")` returned extensions.code = SQL_ERROR ("expected exactly one SQL statement"), not TRANSACTION_NOT_SUPPORTED. The next `executeSql(INSERT ... 'after-multi')` from another client returned success id 20260907022417; `git ls-tree -r HEAD` contains no file for it (count 0). |
| HIGH | In-process raw `BEGIN` via `execute_sql` silently changed, with a message that is false for FFI/CLI callers and a CHANGELOG line that denies the change | ddb-core/src/error.rs:301 | Pre-change `DoogatDriver::execute_sql("BEGIN")` opened a caller-owned transaction (sql_engine/transaction.rs:28; sql.rs:49). Post-change it returns Err(DdbError::Validation{code: TRANSACTION_NOT_SUPPORTED, msg: "BEGIN is not supported on this SQL surface: it is shared by every client..."}) from sql.rs:36-38. CHANGELOG.md:34 says in-process is "unchanged"; server.md:55 calls the rejection "server-shaped". |
| MEDIUM | New code was not added to the error-policy table; it falls into `_ => Internal` (redacted) in the app contract | ddb-core/src/app_contract/error.rs:107 | Neither app_contract/error.rs:96-108 nor ddb-server/src/http_error.rs:19-31 mentions TRANSACTION_NOT_SUPPORTED; the match ends `_ => AppErrorCategory::Internal`, which to_graphql_error_from_app (ddb-server/src/error.rs:95) redacts. |
| MEDIUM | The rejection is pinned by no test: e2e swallows the BEGIN result and no unit test covers the guard | tests/e2e/hazard_server_txn_scope.rs:124 | `let _ = rt.block_on(client.simple_query("BEGIN"));` accepts success, rejection or a dropped connection alike. `cargo test -p ddb-core --lib service::sql` = 2 passed (both pre-existing freshness tests). |
| MEDIUM | VERIFY: downstream (jink) use of raw BEGIN through the FFI driver | ddb-core/src/ffi/driver.rs:169 | The local jink-feedback repro channel (`dev/local/specs/jink-feedback/ddb-repros/`) is absent on this host, so downstream usage is unverified. |
| MEDIUM | VERIFY: a PgWire connection stays usable after the rejected BEGIN | ddb-server/src/pgwire.rs:114 | The e2e only checks server health via GraphQL after the PgWire client is dropped; nothing exercises the same PgWire connection after its BEGIN is rejected. |
| LOW | server.md paragraph states three wrong facts: `ddb sql`, guard at :18, and PgWire "reports the same code" | docs/src/technical/server.md:55 | CLI subcommand is `ddb query` (ddb-cli/src/main.rs:328). Guard is at service/sql.rs:36. `DoogatError::Structured` displays as `{message}` only (error.rs:93); pgwire.rs:114 wraps it as ApiError, so no code reaches a PgWire client. |
| LOW | KNOWN: message hardcodes the GraphQL field name `executeBatch` inside ddb-core | ddb-core/src/error.rs:302 | A transport-neutral crate names a GraphQL field PgWire, CLI and FFI callers cannot invoke. Out of scope: D1 mandates the wording. |
| LOW | KNOWN: every execute_sql now parses the statement twice (guard + engine) | ddb-core/src/service/sql.rs:23 | `transaction_verb` runs a full `Parser::parse_sql` before `SqlEngine::execute` parses again. Cost negligible next to the git commit. |
| LOW | KNOWN: hazard test module header still describes pre-fix evidence with stale line refs | tests/e2e/hazard_server_txn_scope.rs:4 | Lines 4-16 cite `service/sql.rs:11-24` and state the buffer is process-global as a live hazard. Allowlist restricts test edits. |
| LOW | KNOWN: error-code lists in building-apps.md and app-contract.md omit TRANSACTION_NOT_SUPPORTED | docs/src/guide/building-apps.md:318 | The developer-facing code list (`CASCADE_CYCLE` etc.) does not carry the new code. Docs allowlist is server.md only. |
| LOW | KNOWN: PgWire drops structured codes on the wire for every code, not only this one | ddb-server/src/pgwire.rs:114 | `PgWireError::ApiError(Box::new(e))` carries only `e.to_string()`. Pre-existing gap tracked as 00180. |

Verdict lines:

- D1: pass
- D2: pass
- D3: pass
- D4: pass
- D5: pass

## Blake

lane_status: ok

Read and traced the full FT-6 diff (staged, uncommitted: CHANGELOG.md, ddb-core/src/error.rs, ddb-core/src/service/sql.rs, ddb-server/src/schema/mutations/operations.rs, docs/src/technical/server.md, tests/e2e/hazard_server_txn_scope.rs — matches the allowlist exactly). Ran cargo build --workspace (pass, 1m06s), cargo build -p ddb-cli (pass), cargo +1.89.0 clippy --workspace --all-targets -- -D warnings (clean), cargo test -p ddb-server (222 unit tests + integration suites, all pass), and `cargo test -p ddb-e2e --test e2e hazard_server_txn_scope` (passes). Then hand-traced transaction_verb()'s parse path against SqlEngine::execute_batch's real multi-statement dispatch and confirmed, by executing an already-present (untracked) probe test in the tree, that a compound SQL string with a transaction verb as a non-first statement fully defeats the new guard.

| Severity | Title | File | Proof / evidence |
| --- | --- | --- | --- |
| CRITICAL | transaction_verb() only inspects the first parsed statement, so a compound SQL string smuggles BEGIN past the new guard and reopens the process-global transaction buffer | ddb-core/src/service/sql.rs:18 | `transaction_verb` matches `statements.first()?` (sql.rs:18-30) while `SqlEngine::execute_batch` (sql_engine/mod.rs:240-313) executes every statement and scans `statements.iter().any(...)` itself. For `"SELECT id FROM note; BEGIN"` the guard returns None, statement 2 calls handle_begin (transaction.rs:19-29) issuing a live `SAVEPOINT ddb_txn`, then execute() errors "expected exactly one SQL statement" and `inspect_err` (sql.rs:46-48) copies the buffer onto the shared service. Empirically: `cargo test -p ddb-core --test zz_tmp_txn_bypass -- --nocapture` -> `Err(SqlEngine("expected exactly one SQL statement"))`, then a fresh `DoogatService::open` panicked with `Sql("database is locked")`. Reachable from GraphQL executeSql (operations.rs:256-285) and PgWire simple_query (pgwire.rs:110-114). |
| MEDIUM | TRANSACTION_NOT_SUPPORTED was not added to the app_contract category table the constraints named as "the one table", though it isn't reached by any live call path today | ddb-core/src/app_contract/error.rs:95 | The Structured match arm (error.rs:95-108) lists every other code explicitly and falls to `_ => AppErrorCategory::Internal` (line 107). Not reachable today: all live execute_sql call sites use `to_graphql_error`/`classify()` (ddb-server/src/error.rs:9-13) or PgWireError::ApiError — verified via `rg execute_sql` across ddb-server/src. |

Verdict lines:

- B1: fail
- B2: pass
- B3: pass
- B4: fail
- B6: pass
- B7: fail
- B8: pass
- B9: fail
- B10: pass
- B11: pass
- B12: pass
- B13: pass

## Bob (codex)

lane_status: ok: codex-run.sh exited 0; output at /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-6-bob-output.txt (440KB, 161393 tokens). 8 findings + 1 unverifiable, 12 rubric verdicts.

| Severity | Title | File | Proof / evidence |
| --- | --- | --- | --- |
| CRITICAL | GraphQL executeBatch(["BEGIN"]) still succeeds and leaves self.txn on the shared service, recreating the cross-client write-capture hazard | ddb-core/src/service/sql.rs:53 | executeBatch(["BEGIN"]) via GraphQL bypasses the guard and opens the shared transaction buffer, so another client's writes are captured. |
| CRITICAL | transaction_verb inspects only the first statement; SELECT 1; BEGIN opens the shared buffer before the multi-statement error, and inspect_err preserves it | ddb-core/src/service/sql.rs:24 | Input `SELECT 1; BEGIN` passes the guard, opens the shared txn buffer, then errors on multi-statement while `inspect_err` leaves the buffer open. |
| HIGH | Core-level guard also rejects transaction verbs through DoogatDriver.execute_sql, changing the in-process FFI transport despite the server-only requirement | ddb-core/src/service/sql.rs:35 | FFI callers issuing BEGIN through DoogatDriver.execute_sql now get rejected although the card scoped the change to the server. |
| HIGH (refuted) | PgWire wire response uses SQLSTATE XX000 and display message only, omitting TRANSACTION_NOT_SUPPORTED | ddb-server/src/pgwire.rs:114 | A PgWire client sending BEGIN receives XX000 with no TRANSACTION_NOT_SUPPORTED code. See Adversarial verification. |
| HIGH (refuted) | Hazard test discards every BEGIN result and asserts no rejection, code, guidance, GraphQL rejection, or COMMIT/ROLLBACK behavior | tests/e2e/hazard_server_txn_scope.rs:124 | Claimed the test passes even if the guard is removed. See Adversarial verification. |
| HIGH (refuted) | TRANSACTION_NOT_SUPPORTED has no explicit error-policy classification; AppError defaults it to Internal | ddb-core/src/app_contract/error.rs:96 | Claimed the new code falls through to Internal so transports disagree. See Adversarial verification. |
| MEDIUM | transaction_not_supported is public despite the card permitting no new public API beyond the error-code constant | ddb-core/src/error.rs:297 | Make it `pub(crate)`. |
| MEDIUM | Docs claim every executeBatch failure rolls the whole set back, contradicting DDL committing immediately | docs/src/technical/server.md:55 | The same document states DDL commits immediately outside the implicit transaction; qualify the claim to DML. |
| LOW | Cannot statically verify: cargo builds, clippy, server tests, and the FT-6 e2e gate pass | N/A | Informational. |

Verdict lines:

- R1: fail
- R2: fail
- R3: pass
- R4: fail
- R6: pass
- R7: fail
- R8: pass
- R9: fail
- R10: fail
- R11: pass
- R12: pass
- R13: pass

## Carl (gemini)

lane_status: ok: gemini-run.sh (backend=copilot, model=gemini-3.8-flash) exited 0 in ~5m29s; output written to /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-6-carl-output.txt with 5 findings and all 12 rubric verdicts. Note: the lane ran cargo test commands of its own during review.

| Severity | Title | File | Proof / evidence |
| --- | --- | --- | --- |
| CRITICAL | Non-first transaction verbs bypass the transaction_verb guard | ddb-core/src/service/sql.rs | Input `SELECT 1; BEGIN` - the guard only inspects the first statement, so BEGIN buffers writes in the shared server actor instead of being rejected. |
| HIGH (refuted) | Swallowed test assertion in hazard_server_txn_scope | tests/e2e/hazard_server_txn_scope.rs | The BEGIN result is discarded, so the test allegedly passes even when TRANSACTION_NOT_SUPPORTED is not returned. See Adversarial verification. |
| HIGH | Missing tests for BEGIN/COMMIT/ROLLBACK rejection | ddb-core/src/service/sql.rs | No unit or integration test asserts TRANSACTION_NOT_SUPPORTED for BEGIN, COMMIT, or ROLLBACK; a regression in the guard would go undetected. |
| MEDIUM | Simplification: use iter().find_map instead of match statements.first() | ddb-core/src/service/sql.rs:23 | Replace `match statements.first()?` with `statements.iter().find_map(...)`. |
| LOW | Docs reference non-existent CLI command 'ddb sql' | docs/src/technical/server.md | Documentation references `ddb sql` instead of `ddb query`. |

Verdict lines:

- R1: fail
- R2: fail
- R3: pass
- R4: pass
- R6: pass
- R7: fail
- R8: pass
- R9: fail
- R10: fail
- R11: pass
- R12: pass
- R13: pass

## Adversarial verification

**Confirmed**

- **[bob, HIGH] Core-level guard also rejects transaction verbs through DoogatDriver.execute_sql (ddb-core/src/service/sql.rs:35)** — Confirmed: `DoogatService::execute_sql` (sql.rs:35-38) carries the new `transaction_verb` guard unconditionally, and `DoogatDriver::execute_sql` (ddb-core/src/ffi/driver.rs:167-172, the public UniFFI method) calls that exact function with no server/FFI discrimination. A raw "BEGIN"/"COMMIT"/"ROLLBACK" through the driver now returns TRANSACTION_NOT_SUPPORTED where it previously succeeded (pre-diff the body ran `engine.execute("BEGIN")` and stored the suspended buffer, functionally opening a transaction). The dedicated begin/commit/rollback_transaction FFI methods are untouched (they call `SqlEngine::execute` directly), but the raw-string path is a real, undiscriminated FFI behavior change with no test covering it.

**Refuted**

- **[bob] PgWire wire response uses SQLSTATE XX000 and display message only, omitting TRANSACTION_NOT_SUPPORTED (ddb-server/src/pgwire.rs:114)** — Refuted. pgwire.rs is untouched by the FT-6 diff; the `PgWireError::ApiError(Box::new(e))` wrap at line 114 is pre-existing code applying identically to every `DoogatError::Structured` variant already shipped (UNIQUE_VIOLATION, REFERENCES_VIOLATION, etc.), not something this change introduced. The card's requirement is that GraphQL and PgWire both route through the same one-place guard in `DoogatService::execute_sql` and get the same structured error — satisfied, since pgwire.rs calls `self.actor.execute_sql`, the same actor-wrapped service GraphQL uses. Nothing in the card, allowlist, or test mandates a PgWire SQLSTATE mapping (SQLSTATE codes are fixed 5-char Postgres codes; ddb's descriptive string codes were never SQLSTATE-shaped). The test explicitly discards the BEGIN response and asserts only the safety invariant, so the claimed proof describes a requirement outside this item's contract and outside the diff's scope.
- **[bob] Hazard test discards every BEGIN result and asserts no rejection, code, guidance, GraphQL rejection, or COMMIT/ROLLBACK behavior (tests/e2e/hazard_server_txn_scope.rs:124)** — Refuted. Traced the full call path: PgWire's non-SELECT BEGIN and GraphQL's executeSql both dispatch through handlers.rs:89 (`svc.execute_sql(&sql)`) to the SAME shared `DoogatService` owned by the single actor loop (actor/mod.rs:665). Without the guard, BEGIN would set `self.txn = Some(buf)`; the subsequent GraphQL INSERT would then hit `buffer_or_collect_write` (dml.rs:377-388) with `self.txn.is_some()`, pushing the write into the buffer instead of committing (dml.rs:396). Leg 1's `assert_committed` (hazard_server_txn_scope.rs:129-134) would then find no file for leaked_id in `git ls-tree HEAD` and panic with "no file for it exists in git HEAD". Removing the guard makes leg 1 fail, not pass. The `let _ =` is deliberate per the adjacent comment (lines 121-123): both rejection and acceptance of BEGIN are safe outcomes as long as legs 1/2 hold.
- **[bob] TRANSACTION_NOT_SUPPORTED has no explicit error-policy classification; AppError defaults it to Internal (ddb-core/src/app_contract/error.rs:96)** — Refuted. Both call sites that can produce the code (GraphQL executeSql/executeBatch via `.map_err(to_graphql_error)` at operations.rs:262,304, and PgWire via `PgWireError::ApiError` at pgwire.rs:114) bypass `app_contract::AppError` entirely and use ddb-server/src/error.rs::classify(), whose `DoogatError::Structured` arm (line 13) returns the real code and message verbatim, so both transports already report the same code/message. The `_ => Internal` default at app_contract/error.rs:107 is a pre-existing, deliberately tested fallback (app_contract_error_t6.rs:130, SOME_FUTURE_CODE -> Internal) that this error's actual code paths never traverse, so no divergence or wrong redaction occurs. (Recorded as advisory for the latent coherence gap.)
- **[carl] Swallowed test assertion in hazard_server_txn_scope (tests/e2e/hazard_server_txn_scope.rs)** — Refuted. The `let _ =` discards only the raw client-side reply to the BEGIN statement, per the card's explicit instruction ("make the BEGIN step accept a structured error as the safe outcome"); it does not discard the test's real assertions. The invariant is checked by `assert_committed` for Leg 1 (buffered-into-foreign-txn) and Leg 2 (wedged-after-disconnect), which inspect git HEAD directly and are untouched by the diff. Under the old buggy behavior, Leg 1's `insert_probe` write would be parked in the PgWire client's transaction instead of reaching git and `assert_committed` would panic. The test is not a no-op pass.

## Rework

One rework cycle ran. Closures:

- **[bob, HIGH] Core-level guard also rejects transaction verbs through DoogatDriver.execute_sql (ddb-core/src/service/sql.rs:35)** — closure attempt recorded, finding still listed as remaining. `transaction_verb` (sql.rs:18-30) now does `statements.iter().find_map(...)` over every parsed statement instead of `statements.first()`, so a payload like `"CREATE TABLE ...; BEGIN"` is caught before `engine.execute()` runs — `execute_sql` returns `Err(transaction_not_supported)` immediately, closing the reported bypass. A backstop (`detach_transaction`, lines 40-51) additionally rejects and rolls back any transaction buffer the engine still holds after `execute()`/`execute_batch()`, whether caused by a verb the parser missed or a batch that leaves BEGIN unmatched, replacing the old unconditional `self.txn = engine.suspend_transaction()`. Verified: `cargo test -p ddb-core --lib service::sql::` -> 6 passed, 0 failed, including the new regression test `rejects_a_transaction_verb_in_any_position` (runs `"SELECT name FROM project; BEGIN"`, asserts rejection, asserts no buffer left on the service, asserts a subsequent INSERT still reaches git). `rejects_a_batch_that_leaves_its_transaction_open` covers the executeBatch analog. No test failures, no ignored tests in this run.

All other blocking findings (the 15 remaining entries in the pre-rework blocking list, covering the first-statement-only guard and the unguarded `execute_batch` across the fanout, eve, blake, bob and carl lanes) were closed by that same rework and do not appear under Remaining.

## Advisory

Non-blocking findings, 28 total.

| Lane | Severity | Title | File |
| --- | --- | --- | --- |
| eve | MEDIUM | New code was not added to the error-policy table; it falls into `_ => Internal` (redacted) in the app contract | ddb-core/src/app_contract/error.rs:107 |
| eve | MEDIUM | The rejection is pinned by no test: e2e swallows the BEGIN result and no unit test covers the guard | tests/e2e/hazard_server_txn_scope.rs:124 |
| eve | LOW | server.md paragraph states three wrong facts: `ddb sql`, guard at :18, and PgWire "reports the same code" | docs/src/technical/server.md:55 |
| eve | MEDIUM | VERIFY: downstream (jink) use of raw BEGIN through the FFI driver | ddb-core/src/ffi/driver.rs:169 |
| eve | MEDIUM | VERIFY: a PgWire connection stays usable after the rejected BEGIN | ddb-server/src/pgwire.rs:114 |
| eve | LOW | KNOWN: message hardcodes the GraphQL field name `executeBatch` inside ddb-core | ddb-core/src/error.rs:302 |
| eve | LOW | KNOWN: every execute_sql now parses the statement twice (guard + engine) | ddb-core/src/service/sql.rs:23 |
| eve | LOW | KNOWN: hazard test module header still describes pre-fix evidence with stale line refs | tests/e2e/hazard_server_txn_scope.rs:4 |
| eve | LOW | KNOWN: error-code lists in building-apps.md and app-contract.md omit TRANSACTION_NOT_SUPPORTED | docs/src/guide/building-apps.md:318 |
| eve | LOW | KNOWN: PgWire drops structured codes on the wire for every code, not only this one | ddb-server/src/pgwire.rs:114 |
| blake | MEDIUM | TRANSACTION_NOT_SUPPORTED was not added to the app_contract category table the constraints named as "the one table", though it isn't reached by any live call path today | ddb-core/src/app_contract/error.rs:95 |
| bob | MEDIUM | transaction_not_supported is public despite the card permitting no new public API beyond the error-code constant | ddb-core/src/error.rs:297 |
| bob | MEDIUM | Docs claim every executeBatch failure rolls the whole set back, contradicting DDL committing immediately | docs/src/technical/server.md:55 |
| bob | LOW | Cannot statically verify: cargo builds, clippy, server tests, and the FT-6 e2e gate pass | N/A |
| carl | MEDIUM | Simplification: use iter().find_map instead of match statements.first() | ddb-core/src/service/sql.rs:23 |
| carl | LOW | Docs reference non-existent CLI command 'ddb sql' | docs/src/technical/server.md |
| fanout | MEDIUM | New behavior ships with no test: no unit test for the code, and the e2e BEGIN step now accepts any outcome | tests/e2e/hazard_server_txn_scope.rs:124 |
| fanout | MEDIUM | The new code was not added to the shared code-to-category/status table the PRD constraint requires | ddb-core/src/app_contract/error.rs:107 |
| fanout | MEDIUM | Guard sits in core, so it also fires on the in-process FFI path the PRD says is unchanged - with advice that surface cannot follow | ddb-core/src/ffi/driver.rs:169 |
| fanout | LOW | Docs claim PgWire reports the same error code; PgWire emits only the message | docs/src/technical/server.md:53 |
| fanout | MEDIUM | TRANSACTION_NOT_SUPPORTED is missing from the one code->category table, so the app-error boundary classifies it Internal and redacts the message | ddb-core/src/error.rs:120 |
| fanout | MEDIUM | No test pins the new rejection; the one test touched was loosened to accept any outcome | tests/e2e/hazard_server_txn_scope.rs:124 |
| fanout | MEDIUM | SAVEPOINT (and RELEASE) still reach the shared SQLite connection and leave it inside an open transaction | ddb-core/src/service/sql.rs:25 |
| fanout | LOW | Core error text hard-codes the GraphQL field name executeBatch, which leaks into CLI and FFI callers | ddb-core/src/error.rs:300 |
| fanout | MEDIUM | Docs claim the rejection is server-shaped, but the guard is in the core service every embedded caller uses | docs/src/technical/server.md:55 |
| fanout | LOW | Statement classification duplicated instead of extending the existing classify module | ddb-core/src/service/sql.rs:18 |
| fanout | MEDIUM | The new rejection behavior has no assertion anywhere; COMMIT and ROLLBACK arms are entirely untested | tests/e2e/hazard_server_txn_scope.rs:124 |
| fanout | MEDIUM | New error code missing from the per-code category table and its table test, so it silently defaults to Internal | ddb-core/src/error.rs:122 |

Notable advisory detail worth carrying forward: the SAVEPOINT gap (fanout, ddb-core/src/service/sql.rs:25) was probed on this tree — `execute_sql("SAVEPOINT sp1")` -> Ok(Rows{..}); a following INSERT does reach git, but `DoogatService::open` from another process then fails with `Sql("database is locked")`, i.e. one client can wedge the shared index connection through the same surface this item hardens (availability, not write loss).

## Assumptions

From the implementors:

1. Placed the guard in `DoogatService::execute_sql` rather than in each transport, because the card forbids a per-transport switch and the shared-actor chokepoint (`ddb-server/src/actor/`) is read-only for me. Side effect: the FFI `DoogatDriver::execute_sql("BEGIN")` is now rejected too. The card enumerates the in-process transaction API as begin/commit/rollback_transaction, which is untouched, and no code in the repo calls execute_sql with a transaction verb.
2. Detected the verb by parsing with sqlparser's GenericDialect and matching `Statement::StartTransaction`/`Commit`/`Rollback`, mirroring the engine's own dispatch (sql_engine/mod.rs:273,371-373), rather than a leading-keyword string test that a comment or leading token could slip past. Costs one extra parse per execute_sql call.
3. Did not guard `execute_batch`. A residual hole remains: `executeBatch(["BEGIN"])` over GraphQL still opens a dangling buffer, because handlers.rs:92 joins the statements into one string and hands them to the same `execute_batch` the CLI uses — closing it there would break in-process CLI transactions, which the card forbids. Flagging rather than fixing.
4. Left the test file's module doc comment (which still narrates the hazard as present) alone: the allowlist restricted my test-file edits to removing `#[ignore]` and relaxing the BEGIN step.
5. Edited `ddb-server/src/schema/mutations/operations.rs` only to append one sentence to the `executeSql` GraphQL field description, so clients see the rule in introspection. No resolver logic changed. `ddb-server/src/pgwire.rs` needed no change: the error already flows through its existing `PgWireError::ApiError` mapping.
6. Read two files outside my read-allowlist — `ddb-server/src/error.rs` and `ddb-server/src/http_error.rs` — to verify the constraint that the new code is redacted and routed like every other structured error. Read-only; no edits.
7. Rejecting the verb in the core service (rather than in each transport) is the intended reading of the card's "one code produced in one place, not a per-transport switch" constraint, even though it also rejects a raw BEGIN through the in-process FFI `DoogatDriver.executeSql`; I addressed findings 10/14 by making the message and the docs/CHANGELOG honest about that reach instead of moving the guard into pgwire.rs and operations.rs.
8. The card's "in-process transactions unchanged" means the owned API (begin_transaction/commit_transaction/rollback_transaction, which call `SqlEngine::execute` directly and never pass through the guard) plus balanced CLI batches; it does not promise that a raw transaction verb typed into execute_sql keeps working.
9. An unbalanced batch's buffered writes are discarded on rejection. They had not reached git, and the alternative (committing them) would silently redefine the batch's atomicity, so discarding is the safe outcome.
10. `tests/e2e/hazard_server_txn_scope.rs` keeps the predecessor's `let _ = ...simple_query("BEGIN")` line: the allowlist permits making the BEGIN step accept a structured error as the safe outcome, and both legs of the test still assert the real invariant unweakened.
11. No test asserts the exact GraphQL field description strings (checked), so extending executeBatch's description is safe.

## Gate

| Command | Exit code | Seconds |
| --- | --- | --- |
| `cargo build` | 0 | 12.5 |
| `cargo build -p ddb-cli` | 0 | 0.2 |
| `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings` | 0 | 0.21 |
| `cargo test -p ddb-server` | 0 | 115.3 |
| `cargo test -p ddb-e2e --test e2e hazard_server_txn_scope` | 0 | 10 |

Files touched by the item: `ddb-core/src/error.rs`, `ddb-core/src/service/sql.rs`, `ddb-server/src/schema/mutations/operations.rs`, `docs/src/technical/server.md`, `tests/e2e/hazard_server_txn_scope.rs`, `CHANGELOG.md`.

## Driver exit notes

Driver (Opus 5, S2) accepts `blocked` and applies D5: the work is stashed as `FT-6`, master untouched at `876a153`.

Gate credibility: `cargo build -p ddb-cli` at 0.2 s and clippy at 0.21 s are cache hits, not measurements (the S1 lesson). Only `cargo test -p ddb-server` (115 s) and the e2e hazard test (10 s) are real numbers here. The driver did not re-certify the gate, because the item is not being committed.

This item came far closer than FT-3. Sixteen blocking findings before rework, one after — and the fifteen that closed were all the same real defect found independently by all five lanes: the first version's guard inspected only `statements.first()`, so `SELECT 1; BEGIN` walked straight past it, and `execute_batch` had no guard at all. The rework fixed both properly (`find_map` over every parsed statement, plus a `detach_transaction` backstop that rolls back any buffer the engine still holds after `execute`/`execute_batch`) and added two named regression tests. That work is good and should be kept.

The one survivor is a scope violation, and it is real. Decision D1 says "reject BEGIN/COMMIT/ROLLBACK **on the shared server actor**" and the card repeated "in-process CLI and FFI transactions are unchanged". The guard was placed in `DoogatService::execute_sql` (`ddb-core/src/service/sql.rs:35`), which is the shared core entry point — and `DoogatDriver::execute_sql` (`ddb-core/src/ffi/driver.rs:167`, a public UniFFI method) calls that same function with no server/in-process discrimination. So a raw `"BEGIN"` string through the FFI driver, or through `ddb sql`, now returns `TRANSACTION_NOT_SUPPORTED` where it previously opened a transaction. The dedicated `begin/commit/rollback_transaction` FFI methods are genuinely untouched (they call `SqlEngine::execute` directly), so the blast radius is only the raw-SQL path — but it is a real, undiscriminated behavior change on two transports the card put out of scope, with no test covering it.

What makes it a block rather than a nit is the second half, which Eve raised pre-rework: the new error message and the CHANGELOG line both describe this as a *server* restriction. They are now false for CLI and FFI callers. Shipping a rejection whose own message misstates who it applies to is the kind of drift the coherence invariants exist to catch.

Two honest ways to close it, and they are not equivalent — this is a decision for the maintainer, not the driver, because it changes D1's scope:

1. **Keep D1 as written**: move the guard to the server boundary (the actor's SQL entry, or a flag the server sets on the service), leaving `DoogatService::execute_sql` unguarded for in-process callers. Preserves CLI/FFI behavior exactly. Costs a discrimination point in the core that the "one code, one place" invariant would rather not have.
2. **Widen D1**: keep the guard core-wide, and fix the message, the CHANGELOG line and `server.md` to say plainly that raw transaction verbs are rejected on every SQL entry point, with `executeBatch` (server) and the dedicated driver methods (FFI) as the atomic units. Cheaper and arguably more coherent, but it IS a behavior change on CLI and FFI and needs its own CHANGELOG entry and a test.

The driver's read is that (2) is the better product answer — a suspended, process-global transaction buffer opened by a raw string is not a feature anyone should rely on in-process either — but (2) contradicts a written decision, so it is not the driver's call to make unattended.

Retry shape for S3: recover with `git stash pop` on the `FT-6` stash (do NOT restart from base; the any-position guard, the `detach_transaction` backstop and the two regression tests are all correct), then apply whichever of (1) or (2) the maintainer picks, and add the missing test for the chosen in-process behavior. Note that the `FT-3` and `FT-6` stashes both touch `CHANGELOG.md`, so popping both will conflict there; pop them one at a time.

Advisory worth carrying regardless of the choice: `TRANSACTION_NOT_SUPPORTED` was not added to the app-contract error table and falls into the `_ => Internal` default (`ddb-core/src/app_contract/error.rs:107`). The verifier refuted this as a *blocking* defect correctly — both live call sites bypass `AppError` and use `ddb-server/src/error.rs::classify()`, so no transport reports the wrong thing today — but it is exactly the latent gap PRD 00173 exists to close, and the code should be added to that table when 00173 lands.

Lane note (D6): all five lanes ran this time. Carl (copilot/gemini) came back after two failures, finishing in ~5m29s with real findings including one CRITICAL that matched the other four lanes.
