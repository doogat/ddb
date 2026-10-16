# Serialize Cold-Start Index Rebuild Across Processes

## Problem

Concurrent writers on a cold repo (no stored HEAD in `_ddb_meta`) each take the destructive full-rebuild path in `rebuild_if_stale` (`ddb-core/src/indexer/rebuild.rs:442-450`): `drop_all_tables()` then recreate FTS. Nothing serializes this across processes, so concurrent cold starts stomp on each other's FTS bootstrap and fail with `sql: no such table: _ddb_fts` / `_ddb_boost`. Found while writing the PRD 00162 concurrent-write e2e (8 concurrent `ddb create` on a cold repo). The 00162 git write lock serializes git commits, not the SQLite rebuild, so it does not cover this. Not data loss — the index is a derived cache, rebuildable from git — but an availability gap: concurrent cold starts can error. The 00162 e2e currently works around it by warming the index with one sequential create before going concurrent; that workaround masks the gap.

## Solution

Guard the destructive full-rebuild path with a repo-scoped cross-process advisory lock, reusing the lock pattern shipped in 00162 (`ddb-core/src/git_ops/write_lock.rs::acquire(repo_root, timeout)` → RAII guard). After acquiring the lock, re-check staleness/integrity before dropping tables (double-checked rebuild): a waiter that blocked behind another process's rebuild finds a fresh index and returns without touching FTS. Design phase decides whether to generalize the existing helper (parameterized lock-file name) or add a sibling index-rebuild lock, and fixes the lock-ordering rule relative to the git write lock so the two can never deadlock.

## Requirements

### Must have

- N concurrent `ddb create` processes on a cold repo (no stored HEAD) all succeed; no `no such table: _ddb_fts` / `_ddb_boost` errors.
- Double-checked rebuild: after lock acquisition, re-verify the rebuild is still needed; waiters must not redo a rebuild another process just completed.
- Explicit lock-ordering rule with the 00162 git write lock (documented and enforced); no deadlock when a path holds both.
- Lock acquisition failure/timeout surfaces as a structured error, never a panic (library-code no-panic invariant).
- Regression e2e: concurrent cold-start creates with no warm-up; verified to fail against pre-change code once.
- Remove (or demote to a comment) the warm-up workaround in the 00162 concurrent-write e2e so that scenario also exercises the cold path.

### Nice to have

- Same guard covers concurrent explicit full-rebuild invocations (`ddb reindex`-style maintenance) — same code path, confirm rather than build.
- Trace/log line when a process waits on the rebuild lock, for support diagnostics.

## Implementation

### Module: cross-process lock helper
- **Location**: `ddb-core/src/git_ops/write_lock.rs` (generalize) or a sibling module if design prefers separation
- **Responsibility**: repo-scoped advisory file lock with timeout, RAII guard; extended/reused for the index-rebuild lock file
- **Exports**: `acquire()` (existing), plus whatever parameterization design lands on

### Module: indexer/rebuild
- **Location**: `ddb-core/src/indexer/rebuild.rs`
- **Responsibility**: take the rebuild lock around the destructive full-rebuild path in `rebuild_if_stale`/`rebuild`; re-check staleness+integrity after acquisition; public API unchanged
- **Exports**: `rebuild_if_stale()`, `rebuild()` (signatures unchanged)

### Module: tests and docs
- **Location**: `tests/e2e/`, `docs/src/technical/invariants.md`, `docs/src/technical/indexer.md`
- **Responsibility**: concurrent cold-start e2e; document the new invariant (destructive index rebuild holds the rebuild lock) alongside the git write-lock invariant
- **Exports**: n/a

### Dependencies

- cross-process lock helper: foundation (exists from 00162; extend only)
- indexer/rebuild guard: depends on [cross-process lock helper]
- tests and docs: depends on [indexer/rebuild guard]

## Tasks

### Phase 0: Foundation
- [ ] Extend/reuse the 00162 lock helper for an index-rebuild lock file; define lock ordering vs the git write lock - unit test proves two handles serialize and guard releases on drop

### Phase 1: Core
- [ ] Wrap the destructive full-rebuild path in the lock with post-acquisition staleness/integrity re-check - concurrent cold-start unit/integration coverage passes; no behavior change for the warm incremental path
- [ ] Structured error on lock timeout - error surfaces through the one error-policy table, no panic

### Phase 2: Validation
- [ ] Concurrent cold-start e2e (no warm-up) - fails against pre-change code, passes after; 00162 e2e warm-up workaround removed
- [ ] Docs: invariant entry + indexer doc update - invariants page lists the rebuild-lock rule with lock ordering

## Success Criteria

- 8 concurrent `ddb create` on a cold repo: all succeed, final index row count matches git, zero FTS bootstrap errors.
- `cargo test-ci` and clippy clean; e2e suite green (rebuild path is replaced, so run `cargo test -p ddb-e2e`).
- Invariants doc names the rebuild lock and its ordering relative to the git write lock.

## Transport Impact

No public API, response-shape, or error-code change on any transport (CLI, GraphQL, REST, PgWire, FFI, NoSQL HTTP). The fix sits below `DoogatService` in the indexer, so all six transports gain the same cold-start robustness through the shared code path; the CLI-driven e2e is the conformance vehicle and no per-transport work is needed.
