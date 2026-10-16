# Cross-process write lock for git commit paths

**Priority:** P0 (data-loss) · **Roadmap:** Phase 0 (stop the bleeding) · **Depends on:** none · **Must precede:** all other new PRDs (this is the lowest-numbered backlog PRD).

## Overview

### Problem Statement
Concurrent git writes silently lose commits. Every write path builds a tree from the current index, resolves HEAD as the parent, and commits with no cross-process lock, no compare-and-swap on the ref, and no retry:

- `ddb-core/src/git_ops/mod.rs:292-322` (`commit_files`): `fresh_index()` reads the HEAD tree at T0, `head_commit()` re-resolves HEAD at T1, `repo.commit(Some("HEAD"), …, &[&parent])` updates the ref with no CAS.
- Same shape in `commit_file:254`, `commit_binary_file:259-288`, `delete_file:343-363`, `delete_files:367-393`, `rename_file:396`, `commit_batch:415`, `commit_binary_and_text:463`.

The only cross-process serialization anywhere in the workspace is `BEGIN IMMEDIATE` around SINGLETON writes (`ddb-core/src/service/create.rs:265-276`); normal creates/updates/deletes have none. There is no advisory lock file (rg for flock/lockfile/fd-lock returns only in-process mutexes).

Failure scenario (the product's intended deployment — a downstream app runs `ddb serve` while the user also runs the CLI, or a shell script fires two `ddb create`s):
- **Lost update**: process P2 builds its tree from HEAD=H0; P1 commits C1; P2 resolves parent=C1 and commits a tree that lacks P1's file → P1's doogat is silently absent from HEAD.
- **Permanent prune**: P2 resolves parent=H0 after P1 committed C1 → HEAD moves H0→C2, C1 becomes unreachable, and `git maintenance --auto` (fired from `check_write_threshold`, `ddb-core/src/maintenance.rs`, on every Nth commit) prunes C1 permanently.

No error surfaces in either case. This is a silent-data-loss class and the highest-priority defect in the repository.

### Target Users
Every downstream (jink and future apps) that runs `ddb serve` alongside CLI use, and every user whose scripts or multiple devices touch one repo concurrently. Maintainers who must trust that a committed create is durable.

### Success Metrics
- All eight git write functions hold a repo-scoped, cross-process advisory lock across the entire write→stage→commit critical section, including the ID-mint→exists-check→commit window (interlocks with 00164).
- A concurrency test that fires N concurrent writers at one repo ends with all N doogats present in HEAD (zero lost updates) and HEAD reachable from every intermediate commit (zero pruned commits).
- Lock acquisition has a bounded timeout with a clear `DoogatError` on contention (fail loud, never hang forever).
- No single-writer throughput regression beyond the lock's own overhead (uncontended acquire is sub-millisecond).

## Functional Decomposition

### Capability: Serialized repo writes
#### Feature: Repo-scoped advisory write lock
- **Description**: a cross-process OS advisory lock that serializes all git write critical sections on one repo.
- **Inputs**: the repo root; a bounded acquire timeout.
- **Outputs**: a guard held for the critical section; a `DoogatError::Conflict`/`Git` on timeout.
- **Behavior**: acquire an exclusive advisory lock on `.git/ddb-write.lock` (create if absent) before the first filesystem write; hold it across stage + `write_tree` + parent-resolve + `commit`; release on guard drop (including on error/panic). Blocking with a timeout, not a spin.

#### Feature: All write paths take the lock
- **Description**: every commit/delete/rename path runs inside the lock.
- **Inputs**: the eight write functions in `git_ops/mod.rs`.
- **Outputs**: each function's body wrapped in `with_write_lock(|| { … })`.
- **Behavior**: a single private `with_write_lock<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T>` helper wraps the existing bodies verbatim; no logic change inside the closure. Because the loser of a lock race re-runs its body, each body must re-resolve HEAD and re-read the index inside the closure (they already do via `fresh_index()`/`head_commit()`; deletes are fixed to `fresh_index()` in 00163).

### Capability: Fail-loud contention
#### Feature: Bounded acquire with clear error
- **Description**: contention beyond the timeout returns a structured error, never a hang.
- **Inputs**: the acquire timeout (config default, e.g. 10s).
- **Outputs**: `DoogatError` naming write contention.
- **Behavior**: on timeout, return a retryable structured error the transports can surface; log the holder's pid if available.

## Structural Decomposition

### Repository Structure
```text
ddb-core/src/git_ops/
├── mod.rs            # Maps to: with_write_lock helper + wrap all 8 write fns
├── write_lock.rs     # Maps to: Repo-scoped advisory write lock (new file)
ddb-core/Cargo.toml   # Maps to: add fs2 (cross-platform advisory locking)
tests/e2e/
└── concurrent_writes.rs  # Maps to: concurrency no-lost-update test (new)
```

### Module: write_lock (ddb-core/src/git_ops/write_lock.rs)
- **Maps to capability**: Serialized repo writes; Fail-loud contention
- **Responsibility**: open/create `.git/ddb-write.lock`, acquire an exclusive advisory lock with a timeout, release on drop.
- **Exports**: `WriteLockGuard`; `acquire(repo_root: &Path, timeout: Duration) -> Result<WriteLockGuard>`.

### Module: git_ops (ddb-core/src/git_ops/mod.rs)
- **Maps to capability**: All write paths take the lock
- **Responsibility**: wrap `commit_file`, `commit_binary_file`, `commit_files`, `delete_file`, `delete_files`, `rename_file`, `commit_batch`, `commit_binary_and_text` in `with_write_lock`.
- **Exports**: unchanged public signatures; new private `with_write_lock`.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies — built first.
- **write_lock.rs + fs2 dependency**: the lock primitive.

### Core Layer (Phase 1)
- **wrap the 8 write fns**: Depends on [write_lock]. Mechanical wrap, no body change.

### Integration Layer (Phase 2)
- **concurrency test + docs**: Depends on [wrapped write fns].

## Implementation Phases

### Phase 0: Foundation
**Goal**: a working cross-process lock primitive.

**Tasks**:
- [x] Add `fs2` to `ddb-core/Cargo.toml` and implement `write_lock.rs` (`acquire` + `WriteLockGuard`, exclusive lock on `.git/ddb-write.lock`, bounded timeout) — Acceptance: a unit test shows a second `acquire` on the same path blocks until the first guard drops, and times out with a `DoogatError` if the first is held past the timeout. (`rustix` is already present but its `flock` is Unix-only; `fs2` covers the Windows smoke/CI matrix.)

**Exit Criteria**: the lock serializes two in-process acquirers and times out under contention.

### Phase 1: Core
**Goal**: every write path is serialized.

**Tasks**:
- [x] Add `with_write_lock` to `git_ops/mod.rs` and wrap all eight write functions' bodies verbatim (depends on: Phase 0) — Acceptance: each write function acquires the lock before its first `std::fs::write`/`remove_file` and holds it through `commit`; `cargo test-ci` stays green; the moved bodies are byte-equivalent to before (diff the closures).

**Exit Criteria**: all writes serialize on the repo lock; existing tests green.

### Phase 2: Integration
**Goal**: prove no lost updates and document the guarantee.

**Tasks**:
- [x] Add `tests/e2e/concurrent_writes.rs`: spawn N threads (and/or N `ddb create` processes) writing distinct doogats to one repo concurrently; assert all N appear in HEAD and no commit is unreachable (depends on: Phase 1) — Acceptance: the test fails against the pre-lock code and passes with the lock.
- [x] Author a smoke/integration scenario for concurrent CLI + serve writes; update `docs/src/technical/git-ops.md` with the write-lock invariant (depends on: Phase 1) — Acceptance: docs state the invariant; scenario added to `tests/integration.sh`/`.ps1`.

**Exit Criteria**: concurrency test + scenario + docs landed.

## Test Strategy

### Critical Scenarios
- **Happy path**: two threads each create a distinct doogat → both present in HEAD, HEAD linear/mergeable, no data lost.
- **Edge case**: a delete concurrent with a create → both reflected in HEAD (no revert of the create by the delete's tree — coordinates with 00163).
- **Error case**: a writer holds the lock past the timeout → the second writer returns a clear `DoogatError`, does not hang, does not corrupt the repo.

## Risks
- **Cross-platform lock semantics**: advisory locks differ Unix vs Windows. Mitigation: use `fs2`'s `lock_exclusive`/`try_lock_exclusive` (both platforms) rather than raw flock; test on the CI Windows job.
- **Deadlock with SINGLETON `BEGIN IMMEDIATE`**: the SQLite transaction and the git lock are different resources acquired in create paths. Mitigation: define one ordering (git write-lock outermost, then SQLite) and document it; audit the singleton path for the reverse order.
- **Held-lock during slow ops**: a hung remote must not hold the write lock forever (see 00173, decouple-maintenance). Mitigation: sync/compact acquire the same lease with a timeout, on a maintenance thread; the write lock is for the local commit critical section only, not network I/O.
- **Lock file in `.git/`**: ensure it is not committed and survives `git gc`. Mitigation: `.git/` is never tracked; the file is a runtime artifact.

## Transport Impact
- **CLI**: Guaranteed — CLI writes take the lock; concurrent CLI + serve is safe.
- **GraphQL**: Guaranteed — actor writes take the lock (the actor is one writer among possibly many processes).
- **REST**: Guaranteed — routes through the same actor/core write paths.
- **FFI**: Guaranteed — FFI writes take the lock; a mobile app + a desktop CLI on a synced repo are serialized locally.
- **PgWire**: Guaranteed — INSERT/UPDATE/DELETE land through the same git write paths.
- **NoSQL HTTP**: Intentionally absent for writes (read-only surface); reads are unaffected.
- **Cross-interface conformance plan**: the `concurrent_writes` e2e drives the CLI binary; an integration scenario drives concurrent CLI + `ddb serve` writes; both assert zero lost updates. The invariant is documented in `git-ops.md` and enforced as a guardrail in AGENTS.md so no future write path skips the lock.
