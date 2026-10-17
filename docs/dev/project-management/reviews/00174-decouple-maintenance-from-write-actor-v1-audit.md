# Decision Audit Log: 00174-decouple-maintenance-from-write-actor-v1

PRD: `00174-decouple-maintenance-from-write-actor-v1.md`
Started: 2026-10-17T21:01:17Z
Completed: 2026-10-17T21:01:17Z
Autonomous: 21  |  Deferred: 32  |  Doubts: 0

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R1: configured remote timeout never reaches fetch/push

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R2: nosql-during-compact e2e does not prove overlap

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R3: rustfmt drift in actor/mod.rs:94

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R4: missing tests: progressing-fetch deadline, [remote] parsing, startup-failure path

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R6: NoSQL reads bypass the mirror open_lock; latent DatabaseAlreadyOpen race

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R8: nosql.rs doc comment overclaims no DatabaseAlreadyOpen collisions

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R9: config.rs duplicates the 30s default instead of DEFAULT_REMOTE_TIMEOUT

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R10: no push fixture stalls mid-pack-upload

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R11: lost-update test races compact, which performs no git write

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R12: slow-sync e2e asserts only that errors is present

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R13: timeout tests mutate process-global libgit2 opts from parallel tests

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R14: no real pack-transfer stall and no progressing-fetch cancellation test

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R15: NoSQL/compact test establishes no overlap or compact duration

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R16: maintenance/write test verifies no maintenance-produced write

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R17: required showboat walkthrough absent

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R18: ServerConfig::load exceeds the 50-line function limit

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R19: nosql-during-compact test fn exceeds the 50-line limit

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R20: test fixtures install libgit2 opts without a Once guard

**Choice**: auto-fix (queued as a [D1] rework task)

**Rationale**: additive or clear mechanical fix inside the PRD's stated scope

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: R1 touches the public DoogatService::sync / SyncManager signature

**Choice**: research-then-decide (Protocol D) -> proceed

**Rationale**: the PRD task text explicitly requires threading the configured timeout through sync_manager callers; the CLI keeps DEFAULT_REMOTE_TIMEOUT, so no CLI/FFI behaviour change

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: Bob's issue lines arrived as markdown bullets and did not match consolidate_findings.py's ^[ anchor

**Choice**: stripped the leading '- ' from his output file and re-consolidated

**Rationale**: all 7 of his findings would otherwise have been silently dropped from the table

### [autonomous] 2026-10-17T21:01:17Z

**Decision**: Bob's first codex dispatch returned a lack-of-input refusal (sandbox blocked both readers)

**Choice**: one inlined retry per references/retry-policy.md

**Rationale**: the doubt lens must not silently drop; the inlined prompt carried the context and full diff and produced a complete review

### [deferred] 2026-10-17T21:01:17Z

**Decision**: Push has no deadline, PushOptions or callbacks; push relies only on the process-global libgit2 idle timeout, so a trickling remote never aborts a push.

**Rationale**: deferred by review-close

### [deferred] 2026-10-17T21:01:17Z

**Decision**: Silent-stall protection is installed only by server startup; CLI/FFI callers use the changed core operations without installing the native timeout options.

**Rationale**: deferred by review-close

### [deferred] 2026-10-17T21:01:17Z

**Decision**: The diff touches files already over the 800-line limit (git_ops/mod.rs 1135, git_ops/tests.rs 4168, sync_manager/tests.rs 2143, property_tests.rs 2400), changing only call arities there.

**Rationale**: deferred by review-close

### [deferred] 2026-10-17T21:01:17Z

**Decision**: R4/R14 only partly resolved. Config parsing and the `.max(1)` floor now have tests, and the libgit2 option install is behind a `Once`. `remote_callbacks` (the fetch `transfer_progress`/`sideband_progress` wall-clock deadline) still has no test, and the module doc now just states that as a known limitation. A deterministic test needs no wire-format emulation. Fetch from a local `file://` repo that has objects, using `timeout = Duration::ZERO`, so the deadline is already past when the first transfer callback fires. Assert `REMOTE_TIMEOUT`, and assert that a normal timeout succeeds. This fails if the callback layer is removed.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: R2/R15 not resolved. `probe_overlap_with_burst_gets` computes an overlap flag, and the test discards it as `_observed_overlap`. The test therefore still passes against the pre-change single-actor code whenever compact finishes in milliseconds, which the doc comment itself admits. Burst GETs do not make this deterministic. A deterministic alternative needs no hook in `ddb-core`. Reuse `stalled_remote()` from `integration_sync_decoupling.rs`: start a `sync` that blocks the maintenance worker for the libgit2 timeout, assert `!sync.is_finished()`, issue the NoSQL GET, and assert it returns well before the sync ends. At minimum, assert the overlap flag, or rename the test so it does not claim to prove decoupling.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: R1 resolved (configured `[remote] timeout_secs` now reaches `sync_with_timeout` -> `fetch`/`push` -> `map_remote_error`), but nothing tests the plumbing. `ActorHandle::sync` could go back to `DEFAULT_REMOTE_TIMEOUT` and every test would still pass. The e2e stall test runs at the 30s default, which equals the hard-coded constant. Add a test with a short configured timeout (for example `ActorHandle::spawn(.., 2s)` or `ServerGuard` with `[remote] timeout_secs=2` against `stalled_remote()`) that asserts `REMOTE_TIMEOUT` well under 30s.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: Documentation names an API that does not exist. The new `nosql.rs` doc comment and `docs/src/technical/server.md:36` say NoSQL reads are serialized against mirror writes via `NoSqlMirrorPort::lock_for_read`, "which `DoogatService` holds for the read's full duration". `rg lock_for_read` finds only those two doc mentions. The code instead retries `RedbIndex::open` 20 times at 10 ms (`ddb-core/src/service/utility.rs:121`). The rework turned the earlier overclaim (R6/R8) into a false claim about a lock that is not there. Rewrite both texts to describe what the code does, or implement the lock.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: NoSQL docs and code comment describe a mechanism that does not exist. docs/src/technical/server.md:36 and ddb-core/src/nosql.rs:232 say NoSQL reads are serialized against mirror writes via `NoSqlMirrorPort::lock_for_read`. That method exists nowhere (rg finds only those two comments); the trait at ddb-core/src/traits.rs:352 has only the two mirror methods. Commit 92571f5 replaced it with a bounded retry (20 x 10 ms) on the read side only (ddb-core/src/service/utility.rs:121); docs were not updated. The "never two opens competing" claim is false

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: NoSQL mirror writes can now be silently lost. RedbMirror::with_index (ddb-core/src/nosql.rs:261) opens redb with no retry, and nosql_index_doogat discards the error (ddb-core/src/service/write_helpers.rs:254, `let _ =`). A NoSQL read on the write-actor thread holds the exclusive redb handle for its whole rebuild (utility.rs:141; open_nosql_fresh rebuilds on every read). A maintenance-thread mirror write landing in that window fails with DatabaseAlreadyOpen and is dropped. Conversely a read can fail after 200 ms of retries. Single-threading used to make this impossible. Mitigated only because each read rebuilds the whole mirror

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: NoSQL reads still bypass the mirror's `open_lock`. Retrying reads for 190ms does not serialize access: a read can acquire redb first and make a concurrent mirror write fail immediately, or exhaust retries during a longer write. The new documentation incorrectly claims `lock_for_read` provides serialization. Hold the shared mirror lock throughout each read's handle lifetime and add a read/write concurrency regression.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: integration_nosql_get_stays_bounded_while_compact_runs ignores probe_overlap_with_burst_gets and does not verify overlap

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: `docs/src/technical/server.md:36` contains the same false `lock_for_read` claim, plus "never two opens competing". The retry only covers the read's own open. The read keeps its handle open through `rebuild` and the query, and a mirror write's `RedbIndex::open` has no retry.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: The open-retry in `open_nosql` is speculative, untested and not behavior-preserving in intent. Mirror writes come only from `nosql_index_doogat` (create.rs:267, create.rs:522, write_helpers.rs:38) and `mirror_remove_doogat` (delete.rs:90). All of these run on the write-actor thread, the same thread as NoSQL reads, so the "maintenance-thread mirror write races a read" window the comments describe does not occur. Either drop the loop and narrow the comment, or keep it and add a test: hold a `RedbIndex` open in one thread, release it after about 50 ms, and assert `nosql_get` succeeds. A message-substring match on `"already open"` is also brittle.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: Simplification: `open_nosql` ends with `last_err.expect("loop always runs at least once")`, a panic path in library code that AGENTS.md forbids ("No runtime panics in library/FFI code"). The `attempt < 19` bookkeeping is also hand-rolled. Restructure so the last attempt returns its own error, for example loop `0..19` with `continue` and a final plain `RedbIndex::open(..)` call.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: R16 only partly resolved. The lost-update test now races a real `sync` that merges a diverged remote. It never asserts the maintenance-produced write landed (for example `remote-file.txt` present in the worktree or "remote change" in `git log`). It also does not force overlap: sync may complete before the creates start. Add the landed-write assertion at minimum.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: `diverge_remote` is 66 lines (over the 50-line limit) and repeats the same `Command::new("git")...status().unwrap(); assert!(status.success())` shape nine times. Extract a `git(args: &[&str])` helper.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: Compact does not take the 00162 write lease around its writes. The spec says git-write portions of sync/compact hold the lease. cleanup_crdt_temp (ddb-core/src/compaction/mod.rs:117), compact_crdt_group (:174-178) and run_gc (:308, `git gc`) run on the maintenance thread with no with_write_lock. Sync's own commits do lock. Intent is ambiguous (gc under the lock would violate the no-subprocess-in-hold rule), but neither the lease nor an explicit documented exemption exists, and the "no prune regression" metric is unproven

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: The NoSQL-during-compact e2e test cannot fail. tests/e2e/integration_nosql_during_compact.rs:56-62 admits compact finishes in single-digit ms, so overlap is best-effort and not asserted (:128-130). It passes even if NoSQL reads were still on the blocked thread, so the Phase 2 acceptance is not proven. No actor-level test pairs a blocked maintenance closure with a NoSQL read or create (ddb-server/src/actor/tests/mod.rs has no maintenance test)

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: The lost-update test is weak. integration_maintenance_and_actor_writes_both_land (tests/e2e/integration_sync_decoupling.rs:183) fires sync alongside 5 creates but does not force or assert overlap of a maintenance write and an actor write. Core-side, lock_hook_tests verify deferral via pending flags and the commit-graph file; none uses a deliberately slow `git maintenance`/commit-graph to show a concurrent writer acquiring the lock within the normal bound, as the Phase 0 acceptance describes

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: A push timeout hides partial progress. Sync does fetch, merge, commit and singleton sweep locally, then push. If push times out, the error is only "remote push timed out after Ns" (ddb-core/src/error.rs:158); nothing says the local merge is already committed. The spec's risk section requires reporting completed progress honestly

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: Switching the lost-update scenario to sync introduces a maintenance git write, but does not establish overlapping write attempts or assert that the remote-produced `remote-file.txt` survives. Synchronize the worker and actor write attempts, then verify both remote content and created records in committed Git state and derived reads.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: The module-local `Once` prevents repeated timeout setters but does not establish initialization before other parallel tests use libgit2 repositories. Isolate native-timeout fixtures in child processes that install options before opening repositories.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: The previously required executable walkthrough remains absent from the supplied deliverables. Create a showboat walkthrough demonstrating a responsive mutation during blocked sync and the resulting REMOTE_TIMEOUT.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: Function diverge_remote exceeds 50 lines (81 lines); extract bare remote setup and divergent commit helpers

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: Phantom method NoSqlMirrorPort::lock_for_read documented as concurrency mechanism but does not exist in code

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: integration_maintenance_and_actor_writes_both_land does not verify the maintenance write landed or survived in Git

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: Format drift: an extra blank line was added before `#[cfg(test)]`, and the single-line `ActorHandle::spawn(..., ddb_core::git_ops::DEFAULT_REMOTE_TIMEOUT).unwrap()` calls exceed 100 columns. Wrap them as in the other test files changed in this diff.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: The diff adds `docs/dev/project-management/reviews/...-review-1.md` and the ledger JSON. Confirm these belong in the rework commit. Autopilot state files normally land in separate chore commits.

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: map_remote_error (ddb-core/src/git_ops/remote.rs:44) labels any git error arriving within 200 ms of the timeout as REMOTE_TIMEOUT; a real auth/protocol failure at about 29.8 s is misreported

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: docs/src/technical/server.md:28 still says NoSQL lookups go through ReadPool, contradicting server.md:36 and :1026. The constraint note at :1026 only points to a code comment, which is itself stale. The sync.md update is one paragraph

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: CHANGELOG.md:18-19 records the timeout and the worker but omits the public API change (GitRepo::fetch/push now require a timeout argument) and the behavior change that commit-graph and write-threshold maintenance now run after guard drop for all callers, CLI included

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-17T21:01:17Z

**Decision**: The libgit2 timeout is process-global, installed once via a OnceLock (ddb-server/src/lib.rs:101). A second run with a different configured value in the same process silently keeps the first. Low risk

**Rationale**: rework cap reached with this finding unresolved
