---
prd: docs/dev/project-management/prds/wip/00174-decouple-maintenance-from-write-actor-v1.md
review: 2
date: 2026-10-17
head_sha: 0f498bfc74717365b52e4c7f04dbd9de94955157
codex_thread_id: 01a14b97-848e-72a0-93bc-90669a024f55
reviewers: alice,blake,bob,carl
agents:
  alice: available
  blake: available
  bob: available (after one retry, inlined)
  carl: available
dispatch_rows:
  bob: 9b716045 (first run, lack-of-input), 4663bb2a (retry, ok)
  carl: 1e058f02
---

# Review: 00174-decouple-maintenance-from-write-actor-v1

Diff range: `0163cdc7366a5d6202f7fea611ffe486c3da8247..0f498bfc74717365b52e4c7f04dbd9de94955157` (incremental: rework commits since cycle 1)

codex_rung_guard: not fired

pack: ok

## Review Summary

Reviewed: 8 completed tasks (incremental review of the cycle-1 rework)
PRDs checked: 00174-decouple-maintenance-from-write-actor-v1.md

### Agent Status

- Alice: ✅ Available (consensus lens, Claude subagent). She reports she could NOT run `cargo clippy --workspace --all-targets` or the tests (build-lock contention; the 1.89 clippy run timed out at 600 s), so her findings come from code reading only.
- Blake: ✅ Available (blind lens, PRD-only)
- Bob: ✅ Available (doubt lens, codex) — after one retry (inlined). The first dispatch returned a lack-of-input refusal (`sandbox-exec: sandbox_apply: Operation not permitted` on both read attempts, no findings, all R-verdicts `fail`); the inlined retry prompt (context pack + review context + full diff inlined, 185 KB) produced the review consolidated here.
- Carl: ✅ Available (gemini via copilot backend)

Mechanical facts block: computed, but `compute_mech_facts.py` is Python-only, so every changed Rust file is listed as `skipped (non-python)`. No countable claim in this cycle is covered by the block, so none was discarded under the computed-facts rule (Alice says `diverge_remote` is 66 lines, Carl says 81 — neither is covered, so neither is discarded). Tautological-test check: 0 test functions in 0 files (Python-only too). Fail-first replay: skipped (no `--replay-cmd`; this is a Rust project).

Carry-forward of previous-cycle verification checks: none — cycle 1 wrote no `-checks-1.json` queue file (no doubt-lens VERIFY item named a runnable command).

Staging: `autopilot review-stage` exit 0, no errors; test gate ran this cycle (`cargo test-ci`, exit 0) but emitted no parseable PASS/FAIL/SKIP summary line, so the counts below are not read values (see the `Tests:` line).

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [3/4] | 🟠 | R4/R14 only partly resolved. Config parsing and the `.max(1)` floor now have tests, and the libgit2 option install is behind a `Once`. `remote_callbacks` (the fetch `transfer_progress`/`sideband_progress` wall-clock deadline) still has no test, and the module doc now just states that as a known limitation. A deterministic test needs no wire-format emulation. Fetch from a local `file://` repo that has objects, using `timeout = Duration::ZERO`, so the deadline is already past when the first transfer callback fires. Assert `REMOTE_TIMEOUT`, and assert that a normal timeout succeeds. This fails if the callback layer is removed. | ddb-core/src/git_ops/remote_timeout_tests.rs:8 | 1 | ALICE, BOB, CARL |
| R2 | [2/4] | 🟠 | R2/R15 not resolved. `probe_overlap_with_burst_gets` computes an overlap flag, and the test discards it as `_observed_overlap`. The test therefore still passes against the pre-change single-actor code whenever compact finishes in milliseconds, which the doc comment itself admits. Burst GETs do not make this deterministic. A deterministic alternative needs no hook in `ddb-core`. Reuse `stalled_remote()` from `integration_sync_decoupling.rs`: start a `sync` that blocks the maintenance worker for the libgit2 timeout, assert `!sync.is_finished()`, issue the NoSQL GET, and assert it returns well before the sync ends. At minimum, assert the overlap flag, or rename the test so it does not claim to prove decoupling. | tests/e2e/integration_nosql_during_compact.rs:130 | 3 | ALICE, BOB |
| R3 | [1/4] | 🟠 | R1 resolved (configured `[remote] timeout_secs` now reaches `sync_with_timeout` -> `fetch`/`push` -> `map_remote_error`), but nothing tests the plumbing. `ActorHandle::sync` could go back to `DEFAULT_REMOTE_TIMEOUT` and every test would still pass. The e2e stall test runs at the 30s default, which equals the hard-coded constant. Add a test with a short configured timeout (for example `ActorHandle::spawn(.., 2s)` or `ServerGuard` with `[remote] timeout_secs=2` against `stalled_remote()`) that asserts `REMOTE_TIMEOUT` well under 30s. | ddb-server/src/actor/mod.rs:483 | 1 | ALICE |
| R4 | [1/4] | 🟠 | Documentation names an API that does not exist. The new `nosql.rs` doc comment and `docs/src/technical/server.md:36` say NoSQL reads are serialized against mirror writes via `NoSqlMirrorPort::lock_for_read`, "which `DoogatService` holds for the read's full duration". `rg lock_for_read` finds only those two doc mentions. The code instead retries `RedbIndex::open` 20 times at 10 ms (`ddb-core/src/service/utility.rs:121`). The rework turned the earlier overclaim (R6/R8) into a false claim about a lock that is not there. Rewrite both texts to describe what the code does, or implement the lock. | ddb-core/src/nosql.rs:229 | 2 | ALICE |
| R5 | [1/4] | 🟠 | NoSQL docs and code comment describe a mechanism that does not exist. docs/src/technical/server.md:36 and ddb-core/src/nosql.rs:232 say NoSQL reads are serialized against mirror writes via `NoSqlMirrorPort::lock_for_read`. That method exists nowhere (rg finds only those two comments); the trait at ddb-core/src/traits.rs:352 has only the two mirror methods. Commit 92571f5 replaced it with a bounded retry (20 x 10 ms) on the read side only (ddb-core/src/service/utility.rs:121); docs were not updated. The "never two opens competing" claim is false | ddb-server/src/actor/mod.rs | 3 | BLAKE |
| R6 | [1/4] | 🟠 | NoSQL mirror writes can now be silently lost. RedbMirror::with_index (ddb-core/src/nosql.rs:261) opens redb with no retry, and nosql_index_doogat discards the error (ddb-core/src/service/write_helpers.rs:254, `let _ =`). A NoSQL read on the write-actor thread holds the exclusive redb handle for its whole rebuild (utility.rs:141; open_nosql_fresh rebuilds on every read). A maintenance-thread mirror write landing in that window fails with DatabaseAlreadyOpen and is dropped. Conversely a read can fail after 200 ms of retries. Single-threading used to make this impossible. Mitigated only because each read rebuilds the whole mirror | ddb-core/src/service/utility.rs | 2 | BLAKE |
| R7 | [1/4] | 🟠 | The `timeout` argument to GitRepo::fetch/push does nothing for push, and for fetch nothing until the first progress callback, unless the caller has installed the libgit2 globals. Only the server does (ddb-server/src/lib.rs:47). CLI/FFI/library callers (ddb-core/src/service/concrete_index.rs:22) pass DEFAULT_REMOTE_TIMEOUT and get no idle bound. The signature promises a bound it does not deliver. SSH transport (libssh2) may also not honor the libgit2 server-timeout option (unverified); all fixtures use http:// only | ddb-core/src/git_ops/remote.rs | 1 | BLAKE |
| R8 | [1/4] | 🟠 | NoSQL reads still bypass the mirror's `open_lock`. Retrying reads for 190ms does not serialize access: a read can acquire redb first and make a concurrent mirror write fail immediately, or exhaust retries during a longer write. The new documentation incorrectly claims `lock_for_read` provides serialization. Hold the shared mirror lock throughout each read's handle lifetime and add a read/write concurrency regression. | ddb-core/src/service/utility.rs:123 | 7 | BOB |
| R9 | [1/4] | 🟠 | integration_nosql_get_stays_bounded_while_compact_runs ignores probe_overlap_with_burst_gets and does not verify overlap | tests/e2e/integration_nosql_during_compact.rs:130 | 3 | CARL |
| R10 | [1/4] | 🟡 | `docs/src/technical/server.md:36` contains the same false `lock_for_read` claim, plus "never two opens competing". The retry only covers the read's own open. The read keeps its handle open through `rebuild` and the query, and a mirror write's `RedbIndex::open` has no retry. | docs/src/technical/server.md:36 | 4 | ALICE |
| R11 | [1/4] | 🟡 | The open-retry in `open_nosql` is speculative, untested and not behavior-preserving in intent. Mirror writes come only from `nosql_index_doogat` (create.rs:267, create.rs:522, write_helpers.rs:38) and `mirror_remove_doogat` (delete.rs:90). All of these run on the write-actor thread, the same thread as NoSQL reads, so the "maintenance-thread mirror write races a read" window the comments describe does not occur. Either drop the loop and narrow the comment, or keep it and add a test: hold a `RedbIndex` open in one thread, release it after about 50 ms, and assert `nosql_get` succeeds. A message-substring match on `"already open"` is also brittle. | ddb-core/src/service/utility.rs:121 | 2 | ALICE |
| R12 | [1/4] | 🟡 | Simplification: `open_nosql` ends with `last_err.expect("loop always runs at least once")`, a panic path in library code that AGENTS.md forbids ("No runtime panics in library/FFI code"). The `attempt < 19` bookkeeping is also hand-rolled. Restructure so the last attempt returns its own error, for example loop `0..19` with `continue` and a final plain `RedbIndex::open(..)` call. | ddb-core/src/service/utility.rs:136 | 2 | ALICE |
| R13 | [1/4] | 🟡 | R16 only partly resolved. The lost-update test now races a real `sync` that merges a diverged remote. It never asserts the maintenance-produced write landed (for example `remote-file.txt` present in the worktree or "remote change" in `git log`). It also does not force overlap: sync may complete before the creates start. Add the landed-write assertion at minimum. | tests/e2e/integration_sync_decoupling.rs:200 | 2 | ALICE |
| R14 | [1/4] | 🟡 | `diverge_remote` is 66 lines (over the 50-line limit) and repeats the same `Command::new("git")...status().unwrap(); assert!(status.success())` shape nine times. Extract a `git(args: &[&str])` helper. | tests/e2e/integration_sync_decoupling.rs:114 | 2 | ALICE |
| R15 | [1/4] | 🟡 | Compact does not take the 00162 write lease around its writes. The spec says git-write portions of sync/compact hold the lease. cleanup_crdt_temp (ddb-core/src/compaction/mod.rs:117), compact_crdt_group (:174-178) and run_gc (:308, `git gc`) run on the maintenance thread with no with_write_lock. Sync's own commits do lock. Intent is ambiguous (gc under the lock would violate the no-subprocess-in-hold rule), but neither the lease nor an explicit documented exemption exists, and the "no prune regression" metric is unproven | ddb-core/src/compaction/mod.rs | 2 | BLAKE |
| R16 | [1/4] | 🟡 | The NoSQL-during-compact e2e test cannot fail. tests/e2e/integration_nosql_during_compact.rs:56-62 admits compact finishes in single-digit ms, so overlap is best-effort and not asserted (:128-130). It passes even if NoSQL reads were still on the blocked thread, so the Phase 2 acceptance is not proven. No actor-level test pairs a blocked maintenance closure with a NoSQL read or create (ddb-server/src/actor/tests/mod.rs has no maintenance test) | tests/e2e/integration_nosql_during_compact.rs | 3 | BLAKE |
| R17 | [1/4] | 🟡 | The lost-update test is weak. integration_maintenance_and_actor_writes_both_land (tests/e2e/integration_sync_decoupling.rs:183) fires sync alongside 5 creates but does not force or assert overlap of a maintenance write and an actor write. Core-side, lock_hook_tests verify deferral via pending flags and the commit-graph file; none uses a deliberately slow `git maintenance`/commit-graph to show a concurrent writer acquiring the lock within the normal bound, as the Phase 0 acceptance describes | ddb-core/src/git_ops/mod.rs | 2 | BLAKE |
| R18 | [1/4] | 🟡 | A push timeout hides partial progress. Sync does fetch, merge, commit and singleton sweep locally, then push. If push times out, the error is only "remote push timed out after Ns" (ddb-core/src/error.rs:158); nothing says the local merge is already committed. The spec's risk section requires reporting completed progress honestly | ddb-core/src/error.rs | 1 | BLAKE |
| R19 | [1/4] | 🟡 | Switching the lost-update scenario to sync introduces a maintenance git write, but does not establish overlapping write attempts or assert that the remote-produced `remote-file.txt` survives. Synchronize the worker and actor write attempts, then verify both remote content and created records in committed Git state and derived reads. | tests/e2e/integration_sync_decoupling.rs:186 | 5 | BOB |
| R20 | [1/4] | 🟡 | The module-local `Once` prevents repeated timeout setters but does not establish initialization before other parallel tests use libgit2 repositories. Isolate native-timeout fixtures in child processes that install options before opening repositories. | ddb-core/src/git_ops/remote_timeout_tests.rs:38 | 6 | BOB |
| R21 | [1/4] | 🟡 | The previously required executable walkthrough remains absent from the supplied deliverables. Create a showboat walkthrough demonstrating a responsive mutation during blocked sync and the resulting REMOTE_TIMEOUT. | N/A | 7 | BOB |
| R22 | [1/4] | 🟡 | Function diverge_remote exceeds 50 lines (81 lines); extract bare remote setup and divergent commit helpers | tests/e2e/integration_sync_decoupling.rs:114 | 2 | CARL |
| R23 | [1/4] | 🟡 | Phantom method NoSqlMirrorPort::lock_for_read documented as concurrency mechanism but does not exist in code | ddb-core/src/nosql.rs:231 | 3 | CARL |
| R24 | [1/4] | 🟡 | integration_maintenance_and_actor_writes_both_land does not verify the maintenance write landed or survived in Git | tests/e2e/integration_sync_decoupling.rs:235 | 2 | CARL |
| R25 | [1/4] | ⚪ | Format drift: an extra blank line was added before `#[cfg(test)]`, and the single-line `ActorHandle::spawn(..., ddb_core::git_ops::DEFAULT_REMOTE_TIMEOUT).unwrap()` calls exceed 100 columns. Wrap them as in the other test files changed in this diff. | ddb-core/src/nosql.rs:276 | 2 | ALICE |
| R26 | [1/4] | ⚪ | The diff adds `docs/dev/project-management/reviews/...-review-1.md` and the ledger JSON. Confirm these belong in the rework commit. Autopilot state files normally land in separate chore commits. | N/A | general | ALICE |
| R27 | [1/4] | ⚪ | map_remote_error (ddb-core/src/git_ops/remote.rs:44) labels any git error arriving within 200 ms of the timeout as REMOTE_TIMEOUT; a real auth/protocol failure at about 29.8 s is misreported | ddb-core/src/git_ops/remote.rs | 1 | BLAKE |
| R28 | [1/4] | ⚪ | docs/src/technical/server.md:28 still says NoSQL lookups go through ReadPool, contradicting server.md:36 and :1026. The constraint note at :1026 only points to a code comment, which is itself stale. The sync.md update is one paragraph | docs/src/technical/server.md | 4 | BLAKE |
| R29 | [1/4] | ⚪ | CHANGELOG.md:18-19 records the timeout and the worker but omits the public API change (GitRepo::fetch/push now require a timeout argument) and the behavior change that commit-graph and write-threshold maintenance now run after guard drop for all callers, CLI included | CHANGELOG.md | 4 | BLAKE |
| R30 | [1/4] | ⚪ | The libgit2 timeout is process-global, installed once via a OnceLock (ddb-server/src/lib.rs:101). A second run with a different configured value in the same process silently keeps the first. Low risk | ddb-server/src/lib.rs | 1 | BLAKE |

Consolidator note: row R1 merged citations that matched only after suffix stripping (`remote_timeout_tests.rs:8` ~ `:9`).

### Auto-dismissed (ledger)

- [BLAKE] 🟠 The remote timeout is a socket-idle timer, not a deadline. libgit2 timeouts are process-wide integers set via set_libgit2_remote_timeouts (ddb-core/src/git_ops/remote.rs:16). The only wall-clock bound is the fetch progress callback (remote.rs:29), which fires only when data arrives. push passes None options (remote.rs:75). A remote that trickles bytes below the idle threshold is never aborted on push, and fetch is aborted only on the next callback. The spec requires installing FetchOptions/PushOptions with an abort-capable deadline and one options construction point for 00191; push has none. The test file admits the trickle path is untested (ddb-core/src/git_ops/remote_timeout_tests.rs:9-18) | File: ddb-core/src/git_ops/remote.rs — Scoped out by the reviewed design doc on purpose: git2-rs's push_transfer_progress/pack_progress are void FnMuts and cannot abort, so push's bound is the bidirectional libgit2 idle timeout. The design states the residual trickle-only push gap explicitly and instructs the implementor not to close it in this PRD. Deferred to batch end as a known limitation, not re-argued per cycle.

## Alice

Consensus lens, implementation-aware. 11 findings (4 🟠, 5 🟡, 2 ⚪). Headline: the rework replaced the cycle-1 doc overclaim with a false claim — both `nosql.rs` and `server.md` now name `NoSqlMirrorPort::lock_for_read`, a method that exists nowhere; the code is a 20x10 ms open retry instead. The timeout plumbing is real but untested, and the two overlap tests still cannot fail.

Resolved from the prior cycle, per Alice: R1 plumbing (`SyncManager::sync_with_timeout`, `DoogatService::sync_with_timeout`, `ActorHandle::spawn(.., remote_timeout)`, all callers), R3 rustfmt, R9 `DEFAULT_REMOTE_TIMEOUT.as_secs()`, R18 `ServerConfig::load` split, R19 seeding helper, R20 `Once` guard, R12 REMOTE_TIMEOUT code assertion. The three settled deferrals were not re-raised.

Not run by Alice: `cargo clippy --workspace --all-targets` and the tests (build-lock contention; the 1.89 clippy run timed out at 600 s). Her findings are from code reading only.

R1: fail
R2: fail
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

## Blake

Blind lens, PRD-only (no diff, no file list, no review history). 12 findings (4 🟠, 4 🟡, 4 ⚪), one auto-dismissed against the ledger (push trickle gap).

Verified as implemented, per Blake: the worker split (separate maintenance thread and channel; sync/compact/run_maintenance route via `call_maintenance`), subprocess deferral after the outermost guard drop on both Ok and Err, the single REMOTE_TIMEOUT error row (Conflict, HTTP 504, fixed non-secret message), `[remote] timeout_secs` default 30 floored at 1, no lock held over network I/O, no CLI/FFI source change, and no new dependencies. The out-of-scope shared read-only redb handle is correctly absent.

B1: fail
B2: pass
B3: pass
B4: fail
B5: pass
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
B16: fail
B17: pass
B18: pass
B19: pass

## Bob

Doubt lens + de-slop (codex), after one retry (inlined). 6 findings (3 🟠, 3 🟡). FIX bucket as above; VERIFY and KNOWN both `(none)`, so no verification-check queue was written this cycle.

R1: fail
R2: fail
R3: pass
R4: pass
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

Gemini via copilot backend. 5 findings (1 🟠, 4 🟡): the ignored overlap probe, the phantom `lock_for_read` method, the unverified maintenance write, the missing trickling-fetch coverage, and the oversize `diverge_remote` helper.

R1: fail
R2: fail
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

Verdict: 30 findings
Tests: 0 passed, 0 failed, 0 skipped (suite run this cycle: `cargo test-ci` exit 0, green, but it emitted no parseable PASS/FAIL/SKIP summary line, so no counts were read)
