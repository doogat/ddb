---
catchup: force
default_model: opus
model_tier_rationale: concurrency - OID re-check under the write lock across five call sites; catchup forced because 00183 splits git_ops/mod.rs
---

# Guard batch read-modify-write operations against lost updates

**Priority:** P1 (correctness, silent lost-update) · **Roadmap:** appended 2026-07-16 (runs after the tracked backlog; class severity is bounded — admin-frequency operations, seconds-wide window) · **Depends on:** 00162 (write lock, done).

Source: `dev/local/audit-results/evolution-assessment-2026-07-16.md` F8 (lens 2).

## Overview

### Problem Statement

Five batch operations read the whole corpus OUTSIDE the write lock, compute rewrites for seconds, then commit full replacement contents under the lock — silently overwriting any concurrent write that landed in between (line refs at `8fe51c9`):

- `consistency::fix_all` (`consistency/mod.rs:550-613`; reads at `:551,:565`, commits via `commit_batch` at `:611`)
- `consistency::migrate_all` (`consistency/migrations.rs:87-169`; commits at `:169`)
- `consistency::zone_migrate_all` (`consistency/zone_migrate.rs:12-96`; commits at `:96`)
- `consistency::singleton_sweep` (`consistency/singleton_sweep.rs:26`; commits via `commit_batch` at `:143`; called from `sync_manager/mod.rs:398` — same shape, added 2026-09-05 backlog review)
- rename backlink rewrite (`git_ops/rename.rs:68-107`, `rewrite_backlinks`; commits via `commit_files` at `:100-104`, NOT `commit_batch`)

The 00162 lock serializes tree construction only, not the read-compute-commit span. A doogat edited by another process during the compute phase is reverted to the pre-edit content with no error. (The sync collision-loser variant of this class is TRACKED separately in 00167.) `docs/src/technical/git-ops.md:80` currently over-claims that the lock means concurrent writers "cannot lose commits"; this PRD corrects that sentence.

### Target Users

Anyone running `ddb fix` / migrations / renames while a server or second process writes — i.e. jink's standing deployment shape.

### Success Metrics

- A write landing between read and commit is never reverted: skip the affected logical operation with structured CONCURRENT_EDIT_SKIPPED warnings. Quarantine moves stay atomic; a deferred migration cannot advance its version marker and succeeds on a later retry after contention stops.
- One regression test per operation fails on the old code and passes after.
- No behavior change when no concurrent write occurs (existing consistency/rename tests stay green).

## Functional Decomposition

### Capability: OID-guarded batch commit

#### Feature: Guarded commit helper
- **Description**: guarded variants of commit_batch and commit_files validate complete logical units under the repo write lock before applying any unit's writes/deletions.
- **Inputs**: logical units containing writes, deletions, and expected path entries (present OID or expected absence), plus a commit message. Capture source bytes and OIDs from the same read snapshot; guard every source that determined the unit, including a quarantine winner/destination.
- **Outputs**: a GuardedCommitResult with commit: Option<CommitHash>, applied units, and skipped-unit details. No applied change means no commit.
- **Behavior**: under with_write_lock, compare each unit's expected entries with fresh HEAD; any mismatch skips the entire unit. Apply accepted writes/deletions together using a fresh tree, preserving concurrent changes. A quarantine source deletion and destination write are inseparable and guard expected destination absence. A migration version's rewrites and version marker form one unit: on any stale input defer that version and all dependent later versions without advancing the marker.

#### Feature: Call-site adoption with structured warnings
- **Description**: the five operations capture coherent read snapshots, declare logical units, and surface skipped operations as structured warnings.
- **Inputs**: each operation's guarded units and their applied/skipped results.
- **Outputs**: CONCURRENT_EDIT_SKIPPED warnings naming each skipped unit and conflicting paths; counters and success entries describe applied changes only.
- **Behavior**: fix_all, migrate_all, zone_migrate_all, singleton_sweep, and rename backlink rewrites adopt unit-aware guards. Independent fixes may commit while one is skipped; compound moves and versioned migrations cannot be partially applied. A retry after contention stops completes the deferred migration.

## Structural Decomposition

### Repository Structure

```text
ddb-core/src/git_ops/
├── mod.rs                 # Maps to: Guarded commit helper (commit_batch_guarded + commit_files_guarded; commit_batch at :483, commit_files at :361)
└── rename.rs              # Maps to: Call-site adoption (backlink rewrite, via commit_files_guarded)
ddb-core/src/consistency/
├── mod.rs                 # Maps to: Call-site adoption (fix_all)
├── migrations.rs          # Maps to: Call-site adoption (migrate_all)
├── zone_migrate.rs        # Maps to: Call-site adoption (zone_migrate_all)
└── singleton_sweep.rs     # Maps to: Call-site adoption (singleton_sweep)
docs/src/technical/
└── git-ops.md             # Maps to: the guarantee documented; the :80 over-claim corrected
```

### Module: git_ops (ddb-core/src/git_ops/mod.rs)
- **Maps to capability**: OID-guarded batch commit
- **Responsibility**: own the guarded commit variants; keep the plain `commit_batch`/`commit_files` untouched for callers with no read phase.
- **Exports**: commit_batch_guarded(units, message) and commit_files_guarded(units, message) -> Result<GuardedCommitResult>; the result has optional commit, applied units, and structured skipped-unit details. Private unit representation is settled in design.

### Module: consistency + rename (ddb-core/src/consistency/, git_ops/rename.rs)
- **Maps to capability**: OID-guarded batch commit
- **Responsibility**: capture read-time OIDs; adopt the guard; surface drops as `CONCURRENT_EDIT_SKIPPED` warnings.
- **Exports**: existing operation signatures unchanged; reports/outputs gain the warning entries.

### Module: warning contract and guarantee documentation (ddb-core/src/app_contract/output.rs; docs/src/technical/git-ops.md)
- **Maps to capability**: OID-guarded batch commit
- **Responsibility**: define the structured skipped-unit warning and document logical atomicity, no-commit outcomes, and migration retry behavior.
- **Exports**: CONCURRENT_EDIT_SKIPPED and documented report semantics.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first (00162's lock already shipped).
- **guarded commit helpers**: the primitives every call site adopts.

### Core Layer (Phase 1)
- **fix_all adoption**: depends on [helpers].
- **migrate_all + zone_migrate_all + singleton_sweep + rename adoption**: depends on [helpers]; independent of each other.

### Integration Layer (Phase 2)
- **regression tests + docs**: depends on [all adoptions].

## Implementation Phases

### Phase 0: Foundation
**Goal**: the guard primitives exist.

**Tasks**:
- [ ] Implement guarded logical units for commit_batch_guarded/commit_files_guarded: coherent source bytes/OIDs, expected absence, writes, deletions, fresh-tree lock protection, and optional commit result (no deps) - Acceptance: interleaved external writes survive, any changed dependency skips its entire unit, a concurrently created destination is preserved, independent fresh units commit, and an all-stale/no-change batch returns no commit with accurate warnings/counters; regressions fail against plain variants.

**Exit Criteria**: guard primitives unit-tested; plain `commit_batch`/`commit_files` callers unaffected.

### Phase 1: Core
**Goal**: all five operations guarded.

**Tasks**:
- [ ] Adopt the guard in `fix_all` with `CONCURRENT_EDIT_SKIPPED` warnings on its report (depends on: Phase 0) - Acceptance: a regression test with a concurrent edit shows the edit surviving + the warning present; fails on old code.
- [ ] Adopt unit guards in migrate_all, zone_migrate_all, singleton_sweep, and rename backlink rewrites; audit remaining plain commit callers (depends on: Phase 0) - Acceptance: quarantine races preserve both the edited source and competing destination without a half-move; a stale migration leaves its marker unchanged and a retry completes after contention ends; dependent versions do not run past a deferred version; each operation reports only applied work plus structured skips.

**Exit Criteria**: all five regression tests fail-on-old / pass-on-new; caller audit recorded.

### Phase 2: Integration
**Goal**: contract documented.

**Tasks**:
- [ ] Register the `CONCURRENT_EDIT_SKIPPED` warning code beside REINDEX_SKIPPED_FILES in app_contract/output.rs (the warning vocabulary; 00173's table classifies errors); document the guarantee in `docs/src/technical/git-ops.md` and correct its over-claim at `:80` ("cannot lose commits" holds for tree construction only, not for the read-compute-commit span); CHANGELOG `Fixed` entry; author an e2e scenario in the Rust e2e harness (`tests/e2e/`) running `ddb fix` under a concurrent create (the shell harness is removed by 00171) (depends on: Phase 1) - Acceptance: docs match; CHANGELOG present; e2e authored for Tier 2 CI.

**Exit Criteria**: Tier 1 green; docs + e2e landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: `ddb fix` with no concurrent writes → Expected: identical behavior and commit content to today.
- **Edge case**: an external commit lands on file X between read and commit of a batch touching X and Y → Expected: Y committed, X dropped with a warning; X's external content intact.
- **Error case**: every file in the batch went stale → Expected: no empty garbage commit (skip the commit), warnings for all paths, operation reports honestly.

## Risks

- **`commit_batch`/`commit_files` callers outside the five sites**: the guard must stay opt-in so no other behavior changes. Mitigation: the Phase 1 caller audit; the plain variants untouched.
- **Warning-code home**: CONCURRENT_EDIT_SKIPPED belongs in app_contract/output.rs beside existing warning codes; do not treat a skipped unit as an error-table entry.
- **Line refs move before execution**: 00183 splits `git_ops/mod.rs` first (`catchup: force` is set). Mitigation: re-ground the five sites at execution; the function names are stable.

## Transport Impact

The new structured warning is a public warning-contract addition: update developer guidance, CHANGELOG, and warning-parity fixtures through the compatibility checklist; the new warning surfaces through the existing per-transport warning channels (CLI stderr, GraphQL `extensions.warnings`, REST `warnings`, FFI records — whichever exist at execution time). PgWire/NoSQL HTTP: no fix/migrate/rename surface — Intentionally absent. Cross-interface conformance plan: the class is transport-neutral core behavior; the five regression tests + the e2e scenario pin it; the warning shape rides the already-tracked warning-parity work.
