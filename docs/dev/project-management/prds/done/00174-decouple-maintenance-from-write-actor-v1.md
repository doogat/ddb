---
catchup: force
default_model: opus
model_tier_rationale: concurrency - maintenance worker under the write lease plus cancellable remote-operation deadlines
---

# Decouple sync/compact from the write actor; bound remote ops

**Priority:** P2 (availability) · **Roadmap:** Phase 2 (availability) · **Depends on:** 00162 (cross-process write lock), 00172 (closure-message actor: the maintenance worker is built on the closure model, not the twin enums)

> **Provenance:** the in-lock maintenance-subprocess move was added by the 2026-07-16 assessment (F12) and folded into the body's Features/tasks at the 2026-08-14 backlog review. Sites re-verified at `d77ad58`: `check_write_threshold` at `git_ops/mod.rs:354,:387` inside `with_write_lock` closures; `write_commit_graph` per-commit under the lock (`:353,:386` and siblings). `catchup: force` is set because this PRD is the first consumer of the post-00172 actor restructure.

## Overview

### Problem Statement
Every mutation, sync, and compaction serializes on one actor OS thread. A slow or hung git remote stalls every write on every transport until it returns:

- **One thread, one blocking loop, bounded queue**: `ActorHandle::spawn` (`ddb-server/src/actor/mod.rs:221-233`) creates an `mpsc::channel::<ActorMsg>(64)` (`:227`) and spawns a single `std::thread` running `actor_loop` (`:229-231`). `actor_loop` "runs on its own OS thread" (`:663-664`) and pulls work with `rx.blocking_recv()` (`:677`). All commands dispatch through one `handle_command(&mut DoogatService, ...)` (`ddb-server/src/actor/handlers.rs:9`).
- **Sync and compact run inline on that thread**: `ActorCommand::Compact` runs `svc.compact(...)` inline (`ddb-server/src/actor/handlers.rs:127-143`) and `ActorCommand::Sync` runs `svc.sync(&remote, &branch)` inline (`:155-163`). A network push/pull that hangs holds the single thread; once 64 messages queue behind it, HTTP handlers back-pressure and every mutation waits.
- **No timeout bounds the remote op**: no remote-op `timeout`/`deadline` guard exists in `ddb-core/src/git_ops/remote.rs` or `ddb-core/src/sync_manager/` (the only timeout in `git_ops/` is the 10 s `WRITE_LOCK_TIMEOUT`, `git_ops/mod.rs:40`), and the handler calls `svc.sync` directly (`handlers.rs:156`) with no wrapper. `remote.rs` installs NO options or callbacks today — `fetch` and `push` both pass `None` (`remote.rs:14`, `:22`), and there is no `pull` fn (sync composes fetch + merge) — so the deadline work includes installing `FetchOptions`/`PushOptions` with an abort-capable deadline mechanism; progress callbacks alone do not cover silent connection stalls or cancel push. A hung remote hangs forever.
- **Periodic maintenance lands on the same thread**: `spawn_maintenance` (`ddb-server/src/lib.rs:176-184`) spawns a tokio task running `maintenance_loop`, but the loop dispatches `actor.compact(...)` (`ddb-server/src/maintenance.rs:6,14`) — so even scheduled compaction executes inline on the write actor thread.
- **NoSQL reads share the actor, unlike the other read transports**: `NoSqlGet`/`NoSqlScanType`/`NoSqlScanTag`/`NoSqlBacklinks` route through the actor (`ddb-server/src/actor/handlers.rs:11-16`), driven by `actor.nosql_*` in `ddb-server/src/nosql_api.rs:38,70,71,94`. REST, GraphQL, and PgWire reads instead use the concurrent `ReadPool` — "bypassing the single-writer actor" (`ddb-server/src/read_pool.rs:1-4,21-34`), wired at `ddb-server/src/lib.rs:55,65,70` and `ddb-server/src/pgwire.rs:28,82,99-104`. So a NoSQL GET queues behind a running sync/compact while a GraphQL query does not.

Note: the line references above describe the pre-00172 twin-enum actor. After 00172 (closure-message actor) the dispatch mechanics differ (boxed closures; `ActorCommand`/`handlers.rs` are gone), but the problem is unchanged: maintenance closures still execute inline on the single write-loop thread with no timeout.

### Target Users
Downstream apps (jink) serving user-facing requests from `ddb serve` while background sync/compact runs; a long or blocked sync must not stall an unrelated create or a NoSQL GET. Operators who need a hung remote to fail loudly, not hang the server.

### Success Metrics
- Sync and compact execute off the write-actor thread; an independent create issued during a blocked sync completes within a bounded time (not gated by the remote).
- Remote fetch/push are bounded by a timeout (default 30s, tunable via server config); on expiry a clear `DoogatError` surfaces (fail loud, never hang forever).
- No maintenance subprocess (`git maintenance run --auto`, commit-graph write) executes inside the write-lock hold on any caller path; a slow gc cannot push unrelated writers past `WRITE_LOCK_TIMEOUT`.
- The maintenance worker and an actor write cannot both write concurrently: both acquire the 00162 repo-scoped write lease, so decoupling introduces no lost-update/prune regression.
- A NoSQL GET is served while a compact runs (unblocked by the decoupling), OR the redb single-open constraint that prevents moving NoSQL reads to `ReadPool` is documented with the reason.
- No CLI/FFI change: those are separate/in-process and never touched the server actor.

## Functional Decomposition

### Capability: Off-thread maintenance
#### Feature: Maintenance worker
- **Description**: sync, compact, and git-maintenance run on a dedicated worker, not the write-actor thread.
- **Inputs**: `Sync`/`Compact`/`GitMaintenance` requests (client-triggered and from `maintenance_loop`).
- **Outputs**: the same results as today, returned over the closure reply channel (post-00172 there is no `ActorReply` enum), produced without holding the write-actor loop.
- **Behavior**: route maintenance commands to a separate thread/task; the write actor keeps servicing mutations while a maintenance op runs; both the client mutation path and the periodic loop target the worker.

#### Feature: Maintenance subprocesses run after guard drop
- **Description**: the in-core, in-lock maintenance subprocess calls move out of the write-lock hold, for ALL callers (CLI included) — distinct from the server-worker split above.
- **Inputs**: the write paths whose `with_write_lock` closures currently call `check_write_threshold` (`git_ops/mod.rs:354,:387` — synchronously running `git maintenance run --auto`) and per-commit `write_commit_graph`.
- **Outputs**: both run after the guard drops; the lock hold covers only tree/commit construction.
- **Behavior**: a slow maintenance/gc can no longer extend the hold past the 10 s `WRITE_LOCK_TIMEOUT` and fail unrelated writers with `Conflict`; commit results are unchanged, only the timing of the auxiliary subprocess calls moves.

#### Feature: Write-lease coordination
- **Description**: the maintenance worker and the write actor serialize their git writes through the 00162 lock.
- **Inputs**: a git-writing maintenance op (sync/compact) concurrent with an actor create/update.
- **Outputs**: serialized git writes; no interleaved tree/ref races.
- **Behavior**: actual git-write portions of sync/compact acquire the 00162 repo-scoped advisory write lease (the same lock guarding the eight git write functions) so a maintenance write and an actor write never overlap; contention resolves via the lease's bounded acquire timeout.

### Capability: Bounded remote ops
#### Feature: Remote timeout
- **Description**: fetch/push abort after a configurable deadline.
- **Inputs**: a configured remote timeout; newly-installed `RemoteCallbacks`/`FetchOptions`/`PushOptions` on the fetch/push calls (none exist today).
- **Outputs**: on expiry, REMOTE_TIMEOUT through the core error policy (Conflict category, HTTP 504, FFI Conflict hint, fixed non-secret timeout message); no raw credential or transport detail.
- **Behavior**: enforce the configurable 30s deadline with an abort-capable mechanism selected during design. Cover stalls before any progress callback and during both fetch and push; returning a timeout while detached work continues is forbidden. Keep one options/credentials construction point for 00191. Do not hold the repo write lease across blocked network I/O; all actual local tree/ref writes still acquire the repo lock from fresh state.

### Capability: NoSQL read concurrency
#### Feature: NoSQL read placement decision
- **Description**: determine whether NoSQL reads can move to `ReadPool` or must stay on the actor.
- **Inputs**: the redb access model (`ddb-core/src/nosql.rs`).
- **Outputs**: either NoSQL reads on `ReadPool`, or a documented constraint keeping them on the actor.
- **Behavior**: VERIFIED — redb "holds an exclusive file lock while a handle is open" (`ddb-core/src/nosql.rs:227-228`); `RedbMirror` (constructed at `:234`) opens per call (`:241,:245`) and `Database::create` takes the lock (`:38`). A naive `ReadPool` fan-out (fresh `DoogatService` per task) would contend on that exclusive open. Decision: keep NoSQL reads on the actor and document the constraint; the off-thread-maintenance fix already unblocks them from sync/compact stalls. A shared read-only redb handle is a separate follow-up.

## Structural Decomposition

### Repository Structure

```
ddb-server/src/actor/
└── mod.rs               # Maps to: Maintenance worker (spawn worker alongside the write loop; maintenance closure messages route to it, not to the write loop)
ddb-server/src/
├── maintenance.rs       # Maps to: Maintenance worker (periodic loop targets the worker)
├── config.rs            # Maps to: Remote timeout (config value + default)
└── nosql_api.rs         # Maps to: NoSQL read placement decision (documented constraint)
ddb-core/src/git_ops/
├── mod.rs               # Maps to: Maintenance subprocesses run after guard drop (check_write_threshold :354,:387; write_commit_graph out of the hold)
└── remote.rs            # Maps to: Remote timeout (abort-capable fetch/push deadlines and one options/credentials factory)
ddb-core/src/
└── nosql.rs             # Maps to: NoSQL read placement decision (redb single-open constraint)
```

### Module: actor (ddb-server/src/actor/mod.rs)
- **Maps to capability**: Off-thread maintenance
- **Responsibility**: keep the write loop free by dispatching maintenance to a dedicated worker that holds the 00162 write lease during git writes.
- **Exports**:
  - `ActorHandle::spawn(...)` - now also spawns the maintenance worker
  - maintenance dispatch - `Sync`/`Compact`/`GitMaintenance` handled off the write loop

### Module: remote (ddb-core/src/git_ops/remote.rs, app_contract/error.rs)
- **Maps to capability**: Bounded remote ops
- **Responsibility**: install `RemoteCallbacks`/options on fetch/push and abort past a deadline.
- **Exports**: bounded fetch/push that return a timeout `DoogatError`

### Module: nosql (ddb-core/src/nosql.rs)
- **Maps to capability**: NoSQL read concurrency
- **Responsibility**: document the redb exclusive-open constraint that keeps NoSQL reads on the actor.
- **Exports**: unchanged `RedbMirror`; a documented constraint note

### Module: scheduling, configuration, and lock boundaries (ddb-server/src/maintenance.rs, config.rs, nosql_api.rs; ddb-core/src/git_ops/mod.rs)
- **Maps to capability**: Off-thread maintenance; Bounded remote ops; NoSQL read concurrency
- **Responsibility**: route scheduled maintenance to the worker, supply deadline configuration, move auxiliary subprocesses after guard release, and retain the documented NoSQL placement.
- **Exports**: maintenance scheduling, remote timeout setting, and existing write/NoSQL handlers.

## Dependency Graph

### Foundation Layer (Phase 0)
- **remote timeout**: Depends on nothing new; bounds the network op independently.
- **lock-hold slimming (subprocesses after guard drop)**: Depends on nothing new; core-side, all callers.

### Core Layer (Phase 1)
- **maintenance worker + write-lease coordination**: Depends on [00162 write lock, 00172 closure actor] - the newly-concurrent maintenance thread and actor writes must serialize through the same lease; the worker consumes closure messages.

### Integration Layer (Phase 2)
- **NoSQL decision + tests + docs**: Depends on [maintenance worker] - confirm NoSQL reads are unblocked by the decoupling; document the redb constraint.

## Implementation Phases

### Phase 0: Foundation
**Goal**: bound the remote op so a hung remote fails loud.

**Tasks**:
- [ ] Add the configurable 30s remote deadline in server config and an abort-capable implementation in core remote operations, retaining one options/credentials factory (no deps) - Acceptance: deterministic local stalled-remote fixtures exercise fetch and push before progress and mid-transfer with a short deadline; each returns REMOTE_TIMEOUT within the bound through a single new row in 00173's core policy, leaves no operation running after return, preserves repository consistency, and permits an unrelated writer during blocked network I/O.
- [ ] Move `check_write_threshold` and the per-commit `write_commit_graph` calls out of the `with_write_lock` closures (`git_ops/mod.rs:354,:387` and siblings) so both run after guard drop, for every caller (no deps) - Acceptance: a test proves the write lock is released before the maintenance subprocess runs (e.g. a stubbed slow `git maintenance` does not extend the hold — a concurrent writer acquires the lock within the normal bound); commit behavior otherwise unchanged; existing git_ops tests green.

**Exit Criteria**: remote fetch/push are bounded and fail loud on expiry; no maintenance subprocess runs inside the write-lock hold.

### Phase 1: Core
**Goal**: move sync/compact off the write-actor thread under the 00162 lease.

**Tasks**:
- [ ] Spawn a dedicated maintenance worker and route the `Sync`/`Compact`/`GitMaintenance` closure calls to it (client-triggered mutations + periodic `maintenance_loop`), keeping the write loop free; the worker acquires the 00162 write lease around its git writes (depends on: 00162, 00172, Phase 0) - Acceptance: an integration test issues a create while a stubbed slow sync runs and asserts the create completes within a bounded time; a concurrency test asserts no lost update between a maintenance write and an actor write (00162 lease holds).

**Exit Criteria**: sync/compact no longer stall independent mutations; writes stay serialized via the lease.

### Phase 2: Integration
**Goal**: settle NoSQL read placement, add scenarios and docs.

**Tasks**:
- [ ] Confirm NoSQL reads are unblocked by the decoupling and document the redb exclusive-open constraint (`ddb-core/src/nosql.rs:227-228`) that keeps them on the actor rather than `ReadPool` (depends on: Phase 1) - Acceptance: an integration test serves a NoSQL GET while a compact runs and asserts bounded latency; the constraint + follow-up (shared read-only redb handle) is documented.
- [ ] Update `docs/src/technical/server.md` (maintenance worker, remote timeout, NoSQL-vs-ReadPool constraint) + `docs/src/technical/sync.md` + CHANGELOG; add the scenarios to the Rust e2e harness (`tests/e2e/`; the shell harness was removed by 00171): create-during-slow-sync bounded latency; NoSQL GET during compact (depends on: Phase 1) - Acceptance: docs match; e2e scenarios authored and authored for Tier 2 CI.

**Exit Criteria**: NoSQL decision documented; scenarios + docs landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: a normal sync runs on the worker while creates flow through the actor → Expected: creates return promptly; sync result unchanged.
- **Edge case**: a NoSQL GET is issued during an in-progress compact → Expected: the GET is served within a bounded time (not gated by the compact).
- **Error case**: a remote that never completes → Expected: the sync fails with a timeout `DoogatError` within the configured bound; the actor thread is never held hostage; a concurrent create still completes.

## Risks

- **Concurrency correctness with 00162**: making maintenance concurrent with actor writes is only safe if both take the same write lease. Mitigation: hard dependency on 00162; the worker acquires the lease for each actual git-write portion, never across blocked network I/O; a concurrency test asserts zero lost updates.
- **Timeout cancels a partial transfer**: aborting a push/pull mid-transfer must leave the repo consistent. Mitigation: use atomic ref updates; report any already-completed remote/local progress honestly, preserve committed work for retry, and test pre-progress and mid-transfer aborts without orphaned background work.
- **redb constraint blocks true NoSQL read concurrency**: the exclusive-open lock (`nosql.rs:227-228`) means NoSQL reads cannot naively join `ReadPool`. Mitigation: document the constraint; the decoupling still removes the sync/compact stall; defer a shared read-only redb handle to a follow-up PRD.
- **Worker starvation vs the actor**: a long maintenance op could starve a queued maintenance request. Mitigation: single serialized maintenance worker (maintenance ops are already mutually exclusive); mutations are unaffected since they stay on the actor.
- **Closure-model worker**: 00172 (collapse-actor-plumbing) lands first, so this worker routes boxed closure messages rather than enum commands. Mitigation: give maintenance its own channel targeted by a maintenance-specific `call` variant; the create-not-stalled-by-slow-sync concurrency test pins the split.

## Transport Impact

- **GraphQL**: Guaranteed — mutations no longer stall behind sync/compact; latency under maintenance improves; behavior otherwise unchanged.
- **REST**: Guaranteed — same latency improvement; `Sync`/`Compact` REST mutations return via the worker.
- **NoSQL HTTP**: Guaranteed (availability) — GETs/scans are unblocked from sync/compact stalls by the decoupling; they remain on the actor (redb single-open constraint documented), not moved to `ReadPool`.
- **PgWire**: Guaranteed (availability) — SELECTs already run on `ReadPool`, but DML/DDL go through `actor.execute_sql` (`pgwire.rs:110-114`) and stall behind sync/compact today exactly like GraphQL mutations; the decoupling unblocks them too. No PgWire surface change.
- **CLI**: Intentionally absent — the CLI is a separate process with its own `DoogatService`; it never used the server actor. The 00162 lease (cross-process) is what serializes CLI writes against a running `ddb serve`.
- **FFI**: Intentionally absent — FFI embeds `DoogatService` in-process via the driver; there is no server actor to decouple.
- **Cross-interface conformance plan**: an integration test issues a create (GraphQL/REST) while a stubbed slow sync runs and asserts bounded create latency; a second asserts a NoSQL GET is served during a compact; a concurrency test asserts the maintenance worker and actor writes serialize via the 00162 lease (zero lost updates); the remote timeout is pinned by a stalled-transfer test.
