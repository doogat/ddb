---
prd: docs/dev/project-management/prds/wip/00174-decouple-maintenance-from-write-actor-v1.md
review: 1
date: 2026-10-17
head_sha: 0163cdc7366a5d6202f7fea611ffe486c3da8247
reviewers: alice,blake,bob,carl
agents:
  alice: available
  blake: available
  bob: available
  carl: available
dispatch_rows:
  bob: 1d74cb8f
  carl: 6272ec80
---

# Review: 00174-decouple-maintenance-from-write-actor-v1

Diff range: `c8279063360c2fc090604d1150617019f5edf6ba..0163cdc7366a5d6202f7fea611ffe486c3da8247`

codex_rung_guard: not fired

pack: ok

## Review Summary

Reviewed: 4 completed tasks
PRDs checked: 00174-decouple-maintenance-from-write-actor-v1.md

### Agent Status

- Alice: ✅ Available (consensus lens, Claude subagent)
- Blake: ✅ Available (blind lens, PRD-only)
- Bob: ✅ Available (doubt lens, codex) — after one retry (inlined). The first dispatch returned a lack-of-input refusal (`sandbox_apply` blocked both readers); the inlined retry prompt (context + full diff inlined) produced the review consolidated here.
- Carl: ✅ Available (gemini via copilot backend)

Mechanical facts block: computed, but `compute_mech_facts.py` is Python-only, so every changed Rust file is listed as `skipped (non-python)`. No countable claim in this cycle is covered by the block, so none was discarded under the computed-facts rule. Tautological-test check: 0 test functions in 0 files (Python-only too). Fail-first replay: skipped (no `--replay-cmd`; this is a Rust project).

Note on consolidation: Bob's issue lines were emitted as markdown bullets (`- [BOB] …`), which `consolidate_findings.py`'s issue regex (anchored at `^[`) does not match. The leading `- ` was stripped from his output file (text otherwise verbatim) and consolidation was re-run, so his 7 findings are in the table below. Without that step they would have been silently dropped.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [3/4] | 🟠 | Configured `[remote] timeout_secs` never reaches fetch/push. `SyncManager` hard-codes `DEFAULT_REMOTE_TIMEOUT` at ddb-core/src/sync_manager/mod.rs:254 and ddb-core/src/sync_manager/mod.rs:430. `ServerConfig.remote_timeout_secs` feeds only the process-global libgit2 opts (ddb-server/src/lib.rs:46). With timeout_secs=5, a stalled remote fails at about 5s through libgit2's idle timeout. `map_remote_error` (ddb-core/src/git_ops/remote.rs:213) then compares elapsed against 30s, so the caller gets a raw git error instead of REMOTE_TIMEOUT/504. With timeout_secs=60, the fetch progress-callback deadline still aborts a healthy slow fetch at 30s. The task asked for the timeout to be threaded through sync_manager callers. docs/src/technical/server.md:40 says the "configured timeout" bounds sync, which is false for the error mapping and the callback layer. Fix: thread a `Duration` into `SyncManager`/`DoogatService::sync` from the server (CLI keeps the default). | ddb-core/src/sync_manager/mod.rs:254 | 1 | ALICE, BLAKE, BOB |
| R2 | [2/4] | 🟠 | `integration_nosql_get_stays_bounded_while_compact_runs` does not prove overlap, so it would pass against pre-change code. It seeds about 20 tiny doogats, so compact finishes in milliseconds. The test never asserts that compact was still running when the GET returned, for example by checking `compact_thread.is_finished()` is false. If the compact finishes first, the GET is trivially fast with or without the maintenance worker. Force overlap or assert it; the sync-decoupling test already does this with its stalled remote. | tests/e2e/integration_nosql_during_compact.rs:2600 | 3 | ALICE, BLAKE |
| R3 | [2/4] | ⚪ | rustfmt drift: `let write_svc =DoogatService::open_shared_with_mirror(...)` is missing a space. | ddb-server/src/actor/mod.rs:94 | 2 | ALICE, BLAKE |
| R4 | [1/4] | 🟠 | Missing tests for new behavior. The task required a fetch-only slow-but-progressing fixture proving the `transfer_progress`/`sideband_progress` deadline aborts a trickling fetch. ddb-core/src/git_ops/remote_timeout_tests.rs has only silent-stall fixtures, which exercise libgit2's idle timeout, so `remote_callbacks` (ddb-core/src/git_ops/remote.rs:204) has no test. The diff also adds no test for `[remote]` parsing and the `.max(1)` floor (ddb-server/src/config.rs:89), and none for the `install_remote_timeouts` startup-failure path (ddb-server/src/lib.rs:95). | ddb-core/src/git_ops/remote_timeout_tests.rs | 1 | ALICE |
| R5 | [1/4] | 🟠 | Push has no deadline, `PushOptions` or callbacks. `remote.push` passes `None` (`ddb-core/src/git_ops/remote.rs:70-77`), and the push path in `remote.rs` has no wall-clock deadline. Push relies only on the process-global libgit2 idle timeout. A remote that trickles bytes, or any transport the idle timeout does not cover, never aborts a push. Fetch has the same gap before any progress callback fires, for example a slowly dribbled ref advertisement. The spec requires an abort-capable mechanism, `FetchOptions`/`PushOptions`, and one options/credentials construction point for 00191. Fetch installs callbacks and push installs none. | ddb-core/src/git_ops/remote.rs:27 | 1 | BLAKE |
| R6 | [1/4] | 🟠 | NoSQL reads bypass the mirror's `open_lock`. `nosql_get`, `nosql_scan_type`, `nosql_scan_tag` and `nosql_backlinks` call `open_nosql_fresh()` -> `RedbIndex::open` directly (`ddb-core/src/service/utility.rs:104-121`). Only mirror writes go through `RedbMirror::with_index` and its `open_lock` (`ddb-core/src/nosql.rs:254-258`). Because the maintenance worker now shares the mirror and runs concurrently, a mirror write from the worker can overlap a NoSQL read on the write thread. The overlap would fail on redb's exclusive file open (`DatabaseAlreadyOpen`). It is a latent race. `docs/src/technical/server.md:36` claims a single redb handle with no competing opens, which is inaccurate for the read path. | ddb-core/src/service/utility.rs | 3 | BLAKE |
| R7 | [1/4] | 🟠 | Silent-stall protection is installed only by server startup. CLI/FFI callers use the changed core operations without installing these options; push's timeout argument only classifies errors after return and cannot terminate a stall. Initialize default native timeouts safely for supported standalone callers, without adding a CLI knob. | ddb-core/src/git_ops/remote.rs:72 | 1 | BOB |
| R8 | [1/4] | 🟡 | `RedbMirror::open_lock` guards only mirror writes. NoSQL reads open redb directly via `RedbIndex::open` (ddb-core/src/service/utility.rs:106) and bypass the lock. The new doc comment (ddb-core/src/nosql.rs:1109) says services sharing the mirror "never collide on DatabaseAlreadyOpen", which overclaims. In the current code only the write thread calls the mirror, since `mirror_*` is called only from ddb-core/src/service/write_helpers.rs:254 and ddb-core/src/service/delete.rs:90. Narrow the comment, or route reads through the mirror lock. | ddb-core/src/nosql.rs:1109 | 2 | ALICE |
| R9 | [1/4] | 🟡 | Simplification: `.unwrap_or(30)` duplicates `DEFAULT_REMOTE_TIMEOUT`. Use `DEFAULT_REMOTE_TIMEOUT.as_secs()` so there is one source of truth. | ddb-server/src/config.rs:92 | 1 | ALICE |
| R10 | [1/4] | 🟡 | The stalled-remote fixtures do not cover a push that stalls mid-pack-upload. `PUSH_PARTIAL` stalls inside the receive-pack ref advertisement, before any pack data is sent (`ddb-core/src/git_ops/remote_timeout_tests.rs:24-27`). The acceptance criterion names "fetch and push before progress and mid-transfer". The tests also exercise only the libgit2 idle timeout and never a wall-clock deadline. | ddb-core/src/git_ops/remote_timeout_tests.rs | 1 | BLAKE |
| R11 | [1/4] | 🟡 | The lost-update test does not exercise a git write by the worker. `integration_maintenance_and_actor_writes_both_land` races creates against `compact`, which only touches `.crdt/temp` files and runs `git gc` (`ddb-core/src/compaction/mod.rs:184-202,307-315,347-392`). It takes no write lease and makes no git commit. No test races a worker sync, with merge/commit/ref writes, against actor writes. The Phase 1 acceptance criterion says "a maintenance write and an actor write". | tests/e2e/integration_sync_decoupling.rs:101 | 2 | BLAKE |
| R12 | [1/4] | 🟡 | The slow-sync e2e test only asserts that `errors` is present (`tests/e2e/integration_sync_decoupling.rs:98`). It does not assert the REMOTE_TIMEOUT code or HTTP 504 through a transport. It runs at the 30s default and takes about 30s of wall time. | tests/e2e/integration_sync_decoupling.rs:98 | 4 | BLAKE |
| R13 | [1/4] | 🟡 | Timeout tests repeatedly mutate process-global libgit2 options from parallel tests, contrary to the design's initialization constraint. Isolate native-timeout fixtures in child processes with one startup installation before repository use. | ddb-core/src/git_ops/remote_timeout_tests.rs:76 | 1 | BOB |
| R14 | [1/4] | 🟡 | Required transfer coverage is missing: both "mid-transfer" fixtures stop during ref advertisement, before fetch pack receipt or push upload, and no progressing fetch fixture exercises callback cancellation. Add actual pack-transfer stalls and the required progressing-fetch deadline test, preserving consistency and connection-closure assertions. | ddb-core/src/git_ops/remote_timeout_tests.rs:21 | 1 | BOB |
| R15 | [1/4] | 🟡 | The NoSQL/compact test never establishes overlap or a compact duration exceeding the GET bound. It can pass with the original single actor when GET runs first or compact finishes within five seconds. Add a controlled maintenance stall and handshake before issuing GET; assert GET completes before releasing maintenance. | tests/e2e/integration_nosql_during_compact.rs:57 | 3 | BOB |
| R16 | [1/4] | 🟡 | The maintenance/write test neither establishes overlapping git-write attempts nor checks a maintenance-produced write survived; it checks only creates around an uncontrolled compact. Use a maintenance operation with a known write and synchronization, then verify both maintenance and actor changes in Git and derived reads. | tests/e2e/integration_sync_decoupling.rs:101 | 2 | BOB |
| R17 | [1/4] | 🟡 | The required executable walkthrough is absent from the supplied deliverables. Create a showboat walkthrough demonstrating responsive mutations during blocked sync and the resulting timeout error. | N/A | 4 | BOB |
| R18 | [1/4] | 🟡 | Function ServerConfig::load exceeds 50 lines (60 lines); extract TOML section mapping helpers | ddb-server/src/config.rs:45 | 1 | CARL |
| R19 | [1/4] | 🟡 | Function integration_nosql_get_stays_bounded_while_compact_runs exceeds 50 lines (66 lines); extract setup seeding loop into a helper | tests/e2e/integration_nosql_during_compact.rs:34 | 3 | CARL |
| R20 | [1/4] | ⚪ | The `remote_timeout_tests` fixtures call `set_libgit2_remote_timeouts` concurrently from parallel tests. The design says to guard this with `Once`, because libgit2 documents the opts as non-thread-safe. The server guards it with `OnceLock`; the tests do not. | ddb-core/src/git_ops/remote_timeout_tests.rs:75 | 1 | ALICE |
| R21 | [1/4] | ⚪ | The diff touches files already over the 800-line limit, and it only changes call arities there: ddb-core/src/git_ops/mod.rs (1135 lines), ddb-core/src/git_ops/tests.rs (4168), ddb-core/src/sync_manager/tests.rs (2143) and ddb-core/tests/property_tests.rs (2400). The new fixtures were correctly put in a separate file. | ddb-core/src/git_ops/tests.rs | general | ALICE |

## Alice

Consensus lens, implementation-aware. 8 findings (1 🟠 at 3/4 consensus after merge, 2 further 🟠, 2 🟡, 3 ⚪). Headline: the configured remote timeout only feeds the process-global libgit2 options, so the error classification and the fetch callback deadline both still use the 30s constant.

R1: fail
R2: fail
R3: pass
R4: fail
R6: pass
R7: pass
R8: pass
R9: fail
R10: pass
R11: pass
R12: pass
R13: fail

## Blake

Blind lens (PRD only, located the code himself). Confirmed as implemented: the second maintenance thread, the deferred `check_write_threshold`/`write_commit_graph`, the REMOTE_TIMEOUT/Conflict/504 registration, the documented redb constraint, both e2e modules registered, docs and CHANGELOG. 9 findings: the configured-timeout gap (independently), push having no `PushOptions`/deadline, the NoSQL read path bypassing the mirror lock, and four test-strength gaps.

B1: fail
B2: pass
B3: pass
B4: pass
B5: fail
B6: pass
B7: pass
B8: pass
B9: pass
B10: pass
B11: pass
B12: pass
B13: pass
B14: fail
B15: fail
B16: pass
B17: pass
B18: pass
B19: pass

## Bob

Doubt lens + de-slop (codex), after one retry (inlined). FIX bucket: 7 findings — the configured-timeout gap, silent-stall protection installed only at server startup, the process-global libgit2 mutation from parallel tests, missing real pack-transfer and progressing-fetch coverage, the two e2e tests not establishing overlap, and the missing showboat walkthrough. VERIFY: (none). KNOWN: (none). No verification-check queue was written for this cycle (empty VERIFY bucket).

R1: fail
R2: fail
R3: pass
R4: fail
R6: pass
R7: pass
R8: pass
R9: fail
R10: fail
R11: pass
R12: fail
R13: pass

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Carl

Gemini (copilot backend). 2 🟡 findings, both function-length (`ServerConfig::load` 60 lines, `integration_nosql_get_stays_bounded_while_compact_runs` 66 lines). No frontend surface in this diff.

R1: pass
R2: pass
R3: pass
R4: pass
R6: pass
R7: pass
R8: pass
R9: pass
R10: pass
R11: pass
R12: fail
R13: pass

Verdict: 21 findings
Tests: 1954 passed, 0 failed, 0 skipped (suite run this cycle)
