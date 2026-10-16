# Close cross-process write-safety gaps deferred by PRD 00140

## Overview

### Problem Statement

PRD 00140 closed the cross-process SINGLETON TOCTOU by wrapping five
write paths in `BEGIN IMMEDIATE` windows
(`ddb-core/src/service/crud.rs`: `create_doogat_raw` ~271,
`upsert_singleton` ~336, `create_doogat_with_extra` ~483,
`batch_create_with_message` ~853/878, `update_doogat_raw` ~1323) and
deferred the rest. Verified still open on 2026-06-09:

1. **Update-path parity gap**: `update_doogat_parsed` (crud.rs:610) and
   `batch_update_with_message` (crud.rs:698) have no `BEGIN IMMEDIATE`
   wrap. A typed update that retypes a doogat into a SINGLETON typedef
   (or two concurrent updates converging on one) can race past the
   constraint window that creates are protected by.
2. **Two-server startup race**: known bug, pinned by the `#[ignore]`d
   e2e test `two_server_processes_racing_upsert_on_singleton_converge_on_
   one_row` (`tests/e2e/singleton_cross_process_upsert.rs:202-214`). Two
   fresh `ddb serve` processes racing an upsert in their startup window
   can both win.
3. **CLI-vs-PgWire INSERT race flake**: `tests/integration.sh` §55.D
   (lines 2377-2401, "loser missing SINGLETON constraint message") was
   the red-nightly family of 2026-05-23..27 — same SINGLETON race
   surface, nondeterministically losing the structured constraint
   message.

These share one invariant (at most one materialized row per SINGLETON
typedef, enforced across processes) and one test surface (integration §55,
`singleton_cross_process_*` e2e). The SchemaReloader concurrency redesign
and the PgWire handshake-under-race auth failure that PRD 00140 also
deferred are *not* in scope (different subsystems; see Risks).

### Target Users

- Multi-device / multi-process users (desktop app + `ddb serve` + CLI on
  one repo): the SINGLETON primitive (PRD 00139) promises at-most-one-row
  semantics; these gaps are the remaining ways to break the promise.
- The maintainer: §55.D flake noise erodes trust in nightly signal.

### Success Metrics

- All SINGLETON-affecting service write paths run their check→commit→index
  window under `BEGIN IMMEDIATE` (audited list, not just the two known).
- The `#[ignore]` attribute comes off
  `two_server_processes_racing_upsert_on_singleton_converge_on_one_row`
  and it passes deterministically (10/10 local runs).
- integration.sh §55.D passes 10 consecutive local runs; no §55.D nightly
  failure in the two weeks after merge.

## Functional Decomposition

### Capability: Complete write-path audit

Know every path that can write a SINGLETON row, and its protection status.

#### Feature: Write-path × wrap matrix

- **Description**: Enumerate all service/SQL write paths and their
  `BEGIN IMMEDIATE` status (PRD 00136's lesson: audits must grep all
  public methods, not just the flagged ones).
- **Inputs**: `service/crud.rs` (or its post-00156 split files), SQL
  `UPDATE`/`INSERT` paths in `sql_engine/dml.rs`, `consistency` fixers
  that touch typed rows.
- **Outputs**: matrix in the PRD/plan doc: path → SINGLETON-reachable? →
  wrapped? → action.
- **Behavior**: any unwrapped SINGLETON-reachable path becomes a Phase 1
  task; paths that cannot reach SINGLETON rows get a one-line
  justification.

### Capability: Update-path protection

#### Feature: BEGIN IMMEDIATE on typed updates

- **Description**: Extend 00140's Approach-C wrap to the two update paths
  (plus any the audit adds).
- **Inputs**: `update_doogat_parsed`, `batch_update_with_message`, the
  existing wrap helper pattern (factored phases at crud.rs ~878).
- **Outputs**: same transaction discipline as the create paths; structured
  `SINGLETON_VIOLATION` for losers.
- **Behavior**: wrap only when the target/result type is a registered
  SINGLETON typedef (mirroring the create-path conditional, so untyped
  updates pay no cost); nested-transaction tolerance preserved
  (`batch_index` nesting rules from 00140).

### Capability: Startup-window race closure

#### Feature: Two-server upsert convergence

- **Description**: Fix the startup window where two fresh server processes
  both pass the upsert pre-check.
- **Inputs**: the `#[ignore]`d e2e test (exact repro), actor startup /
  `ensure_fresh` sequencing, the `<table>_singleton_lock` UNIQUE index
  (layer-3 backstop from PRD 00139).
- **Outputs**: deterministic convergence: one winner, one structured
  loser, or post-hoc sweep convergence — whichever the investigation
  shows is achievable without cross-process locks beyond SQLite's.
- **Behavior**: investigation-first feature: the test names the symptom;
  root cause (likely each process's fresh index pre-check racing the
  shared git/SQLite state) must be confirmed before the fix is chosen.

#### Feature: §55.D determinism

- **Description**: Make the CLI-vs-PgWire race scenario deterministic in
  its assertions.
- **Inputs**: integration.sh §55.D + .ps1 mirror.
- **Outputs**: either the underlying fix (above) makes the loser's
  constraint message reliable, or the scenario's assertion is corrected to
  the actual contract — with the contract documented.
- **Behavior**: no flake-tolerant retry loops; the assertion must encode
  the real promise.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/service/
└── crud.rs (or update.rs post-00156)  # Maps to: update-path protection
ddb-core/src/sql_engine/dml.rs         # Maps to: audit (SQL UPDATE path status)
ddb-server/src/actor/ (startup path)   # Maps to: startup-window race closure
tests/e2e/singleton_cross_process_upsert.rs  # Maps to: un-ignore + extend
tests/integration.sh + tests/integration.ps1 # Maps to: §55.D determinism
docs/src/technical/sql-engine.md (SINGLETON section)  # Maps to: contract notes
```

### Module: service update paths

- **Maps to capability**: Update-path protection
- **Responsibility**: same transactional discipline as create paths
- **Exports**: unchanged public signatures

### Module: server startup/upsert sequencing

- **Maps to capability**: Startup-window race closure
- **Responsibility**: no upsert decision before the process's view is
  race-safe
- **Exports**: none (internal sequencing)

### Module: race test surface

- **Maps to capability**: all
- **Responsibility**: deterministic multi-process regression coverage
- **Exports**: none (tests)

## Dependency Graph

### Foundation Layer (Phase 0)

No dependencies - built first.

- **Write-path audit**: produces the authoritative task list for Phase 1.

### Core Layer (Phase 1)

- **Update-path wraps**: Depends on [audit]. Strongly prefer landing
  after PRD 00156's crud.rs split (same-file churn; see Risks).

### Integration Layer (Phase 2)

- **Startup race + §55.D**: Depends on [update-path wraps] only for a
  clean test baseline; investigation can start in parallel.

## Implementation Phases

### Phase 0: Foundation

**Goal**: Authoritative protection matrix.

**Tasks**:

- [ ] Audit every write path for SINGLETON reachability and wrap status,
  including SQL `UPDATE` via dml.rs and consistency fixers (no deps) -
  Acceptance: matrix lists every `pub`/`pub(crate)` write method on
  `DoogatService` + SQL DML entry points; each row has wrapped/justified/
  task-created status.

**Exit Criteria**: No write path with unknown status.

### Phase 1: Core

**Goal**: Update paths protected (TDD).

**Tasks**:

- [ ] Write failing cross-process tests for typed update and batch update
  racing into a SINGLETON typedef (depends on: Phase 0) - Acceptance:
  tests mirror 00140's create-path test shape and fail on current master.
- [ ] Wrap `update_doogat_parsed` + `batch_update_with_message` (+ audit
  additions) in conditional `BEGIN IMMEDIATE` (depends on: failing tests)
  - Acceptance: new tests green; Tier 1 + `cargo test -p ddb-e2e` green;
  non-SINGLETON update benchmarks unchanged within noise (`cargo bench`
  crud, compile-check at minimum).

**Exit Criteria**: Update paths have create-path parity.

### Phase 2: Integration

**Goal**: Startup race closed; §55 deterministic.

**Tasks**:

- [ ] Root-cause the two-server startup race using the ignored test as
  repro (depends on: Phase 1 baseline) - Acceptance: written root cause in
  the PRD/plan; fix approach chosen against it.
- [ ] Fix and un-ignore
  `two_server_processes_racing_upsert_on_singleton_converge_on_one_row`
  (depends on: root cause) - Acceptance: 10/10 local passes; `#[ignore]`
  removed.
- [ ] Stabilize integration.sh §55.D + .ps1 mirror in the same commit
  (depends on: fix) - Acceptance: 10 consecutive local §55 runs green;
  assertion encodes the documented contract, no retries added.
- [ ] Document the cross-process SINGLETON guarantee + its boundaries in
  `docs/src/technical/sql-engine.md` (depends on: all) - Acceptance: doc
  states which paths are transactional and what a loser observes per
  interface.

**Exit Criteria**: Success metrics measurable; nightly §55 family quiet.

## Test Strategy

### Critical Scenarios

- **Happy path**: concurrent typed updates to distinct non-SINGLETON
  doogats → Expected: both succeed, no new contention or deadlock
  (IMMEDIATE held only for SINGLETON-reachable writes).
- **Edge case**: two processes race `update` retyping different doogats
  into the same SINGLETON typedef → Expected: one winner; loser gets
  structured `SINGLETON_VIOLATION` with `existing_id`.
- **Edge case**: two fresh `ddb serve` processes race `upsert_<type>` →
  Expected: exactly one row exists after both settle; per-process results
  match the documented contract.
- **Error case**: `BEGIN IMMEDIATE` acquisition timeout under load →
  Expected: structured busy/contention error, no partial git commit, no
  ghost index row.

## Risks

- **Deadlock/contention regression**: broadening IMMEDIATE windows can
  serialize unrelated writes. Mitigation: keep the wrap conditional on
  SINGLETON reachability (00140's pattern); add the happy-path concurrency
  scenario above.
- **Startup race may not be fixable inside SQLite's model**: if the window
  is git-side, the honest outcome may be "sweep converges post-sync, both
  callers told the truth" — then the test's expected contract changes and
  docs say so. Mitigation: investigation-first task with written root
  cause before the fix.
- **File churn collision with PRDs 00156/00149**: same crud.rs region.
  Mitigation: run after 00156 (soft dependency); if run first, expect
  manual-context execution per the oversized-file memory.
- **Deferred items left out**: SchemaReloader redesign (lost done-wakeup,
  version counting) and PgWire handshake-under-race auth failure stay
  deferred — different subsystems, separately testable. They remain on
  the deferred list and should get their own PRD when their flake family
  resurfaces.

## Transport Impact

Service-layer change; transports inherit it. CLI, GraphQL, REST, FFI all
reach updates through the wrapped service paths — losers see the existing
structured `SINGLETON_VIOLATION` in each interface's documented error
shape (GraphQL `extensions.code`, REST/NoSQL `{ error, message }` via the
shared helper, CLI text, FFI string until `ffi-typed-errors-v1`). PgWire:
SQL `UPDATE` protection status comes out of the Phase 0 audit; §55.D pins
the PgWire-visible outcome. NoSQL HTTP is read-only — no impact. No wire
shapes change; only race outcomes become deterministic.
