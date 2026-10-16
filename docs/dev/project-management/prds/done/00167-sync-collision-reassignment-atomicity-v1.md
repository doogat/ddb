# Atomic sync collision-loser reassignment

**Priority:** P1 · **Roadmap:** Phase 1 (convergence) · **Depends on:** 00162 (write lock — the fold runs inside the write critical section), 00164 (deterministic IDs make loser-reassignment agree); relates to 00165 + 00166 (agreed winner)

## Overview

### Problem Statement
Sync collision resolution can lose or duplicate the loser because the reassignment is a multi-step, non-atomic operation and the loser's new ID is wall-clock-minted.

- **Loser reassignment is two commits, not one.** `create_conflict_commit` commits the **winner** at the collided path (`ddb-core/src/sync_manager/mod.rs:329`, body `:340-360`, `commit_merge` at `:354-355`), then `reassign_collision_losers` commits the **loser** under a new ID in a **separate later** commit (`:331-332` → `reassign_single_loser` `:676-711`, `commit_files` at `:705`).
- A crash/kill between the two strands the loser: its content was never committed and never written to the working tree, so it lives at no path in HEAD or the working tree.
- **The loser's new ID is minted from the wall clock.** `reassign_single_loser` calls `parser::generate_unique_id` (`:680`) — repo-aware for existence but timestamped from `chrono` at resolution time. Combined with the add-add default "**theirs wins** on missing HLC" (`:133-136`) and create commits carrying **no HLC** (see 00166), two nodes can pick opposite winners and mint **different** new IDs, so the loser ends up duplicated under two IDs with divergently rewritten backlinks.

### Target Users
Multi-device users (jink) and anyone whose sync hits an ID collision; the losing side must not be stranded, and both nodes must converge on the same loser ID.

### Success Metrics
- Interrupting resolution between the winner-commit and loser-reassign points leaves **no stranded loser**: the loser is recoverable at a path in HEAD — pinned by a crash-injection test.
- The loser's new ID is deterministic (derived from old ID + losing blob OID, not wall clock): two nodes resolving the same collision agree on the loser's ID — pinned by a test.
- The winner content, the loser-at-new-path content, and all backlink rewrites land in **one** merge commit — asserted on the HEAD tree.
- No regression: existing sync/collision-resolution tests stay green.

## Functional Decomposition

### Capability: Atomic collision-loser reassignment

#### Feature: Single-commit winner + loser resolution
- **Description**: winner content, loser-at-new-path content, and all backlink rewrites land in one merge commit.
- **Inputs**: the resolved winner, the losing content, the deterministic loser ID, the scanned link rewrites.
- **Outputs**: one `commit_merge` whose tree holds both the winner (collided path) and the loser (new path) plus rewrites.
- **Behavior**: compute the loser's new ID and rewrites before the merge commit; fold them into the single `create_conflict_commit` call; drop the separate `reassign_single_loser` commit. The whole fold runs inside the 00162 repo-scoped write lease so no concurrent writer observes the intermediate half-state.

#### Feature: Deterministic loser ID
- **Description**: the loser's new ID is content-addressed, not wall-clock.
- **Inputs**: the loser's old ID + losing blob OID.
- **Outputs**: an agreed new ID, unique in the repo.
- **Behavior**: derive the ID from `(old_id, losing_blob_oid)`; mint through the 00164 repo-aware oracle so a collision of the derived ID advances deterministically. Two nodes derive the same ID.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/sync_manager/
└── mod.rs             # Maps to: fold winner (:329) + loser (:676-711) into one commit_merge; deterministic loser id replaces wall-clock generate_unique_id (:680)
```

### Module: sync_manager (ddb-core/src/sync_manager/mod.rs)
- **Maps to capability**: Atomic collision-loser reassignment
- **Responsibility**: commit winner + loser-reassignment + rewrites in one merge commit with a deterministic loser ID, inside the 00162 write lease.
- **Exports**:
  - `create_conflict_commit` / `reassign_collision_losers` - folded into one commit
  - the loser-ID derivation - content-addressed via the 00164 oracle

## Dependency Graph

### Foundation Layer (Phase 0)
Depends on 00164 (repo-aware mint oracle) and 00162 (write lease) - both land first.

- **deterministic loser ID**: content-addressed `(old_id, losing_blob_oid)` via the 00164 oracle.

### Core Layer (Phase 1)
- **single-commit reassignment**: Depends on [deterministic loser ID] - fold winner + loser + rewrites into one `commit_merge` under the 00162 lease.

### Integration Layer (Phase 2)
- **crash-injection test + docs + walkthrough**: Depends on [single-commit reassignment].

## Implementation Phases

### Phase 0: Foundation
**Goal**: an agreed loser ID.

**Tasks**:
- [ ] Derive the loser's new ID from `(old_id, losing_blob_oid)`, minted through the 00164 repo-aware oracle (depends on: 00164) - Acceptance: two nodes resolving the same collision derive the same loser ID; a collision of the derived ID advances deterministically; unit test pins it.

**Exit Criteria**: loser ID is deterministic and repo-safe.

### Phase 1: Core
**Goal**: atomic reassignment in one commit.

**Tasks**:
- [ ] Fold the winner commit, loser-reassignment, and link rewrites into the single `create_conflict_commit` merge commit; drop the separate `reassign_single_loser` commit; run the fold inside the 00162 write lease (depends on: Phase 0, 00162) - Acceptance: after resolution, HEAD holds the winner (collided path) and the loser (deterministic new path) in ONE commit; no intermediate state exposes a stranded loser.

**Exit Criteria**: reassignment is one atomic commit.

### Phase 2: Integration
**Goal**: prove no data loss and document it.

**Tasks**:
- [ ] Crash-injection unit test (interrupt between winner-commit and loser-reassign → loser recoverable at a path in HEAD; two nodes agree on the loser ID); add a sync collision scenario to `integration.sh`/`.ps1`; author the collision-reassignment e2e under `tests/e2e/` (depends on: Phase 1) - Acceptance: all fail against the old code and pass after.
- [ ] Update `docs/src/technical/sync.md` (atomic reassignment; deterministic loser ID) + CHANGELOG (depends on: Phase 1) - Acceptance: docs describe the single-commit fold and the content-addressed loser ID.

**Exit Criteria**: crash-injection coverage + docs landed.

## Test Strategy

### Critical Scenarios
- **Happy path**: two nodes create colliding IDs, then sync → Expected: one winner at the collided path, one loser at a deterministic new path, both in a single merge commit; both nodes agree on the loser ID.
- **Edge case**: crash between the winner-commit and loser-reassign points → Expected: the loser is recoverable at a path in HEAD, never stranded.
- **Error case**: the derived loser ID itself collides in-repo → Expected: the 00164 oracle advances it deterministically; both nodes still agree.

## Risks

- **Two-commit → one-commit refactor**: touches `commit_merge` parents/paths. Mitigation: reuse the existing 2-parent `commit_merge` with the combined file set; assert the HEAD tree holds both winner and loser.
- **Fold must sit inside the write lease**: the combined commit must not race a concurrent local write. Mitigation: hard dependency on 00162; run the fold inside `with_write_lock`.
- **Agreed winner needs 00165/00166**: this PRD makes the loser ID and the commit atomic; the winner is only fully agreed once 00165 (deterministic tie-break) + 00166 (HLC on creates) land. Mitigation: sequence within Phase 1 convergence; note the interplay so this PRD is not expected to fix winner disagreement alone.

## Transport Impact

- **CLI**: Guaranteed — `ddb sync` is the primary fix site for atomic reassignment.
- **GraphQL**: Guaranteed — the sync mutation gains atomic reassignment.
- **REST**: Intentionally absent — there is no sync endpoint to gain the fix. `ddb-server/src/rest.rs` registers only `/doogats`, `/doogats/:id`, and `/schema/apply`. (Corrected in review cycle 1: this row previously claimed "Guaranteed — sync passthrough", describing a surface that does not exist.)
- **PgWire**: Intentionally absent — no sync surface.
- **FFI**: Intentionally absent — `DoogatDriver` exposes no sync method, so there is nothing for this PRD to change on that surface. Its replication-adjacent methods (`register_node`, `import_bundle`, `export_full_bundle`, `export_delta_bundle`) are a separate bundle-based path and are untouched here; note that bundle import merges outside the repo write lock and is tracked separately as 00168. (Corrected in review cycle 1: this row previously claimed "Guaranteed — driver sync".)
- **NoSQL HTTP**: Intentionally absent — read-only document mirror; no sync surface.
- **Cross-interface conformance plan**: `integration.sh`/`.ps1` sync scenarios exercise the CLI only — they contain zero FFI invocations, and no FFI sync surface exists for them to exercise. `add_add_collision_resolution_is_independent_of_push_direction` (`ddb-core/src/sync_manager/tests.rs`) pins the agreed loser ID by having two nodes resolve the same collision independently, with only the push direction varied. `tests/e2e/` covers collision reassignment. No crash-injection test exists: the fold removed the interrupt point it would target (winner, loser, and link rewrites are one commit, so there is no winner-committed/loser-uncommitted seam), and loser-recoverability is instead pinned structurally by the single-commit assertions — recorded as a settled deferral in review cycle 1. No public response/error shape change.
