# Make the HLC load-bearing for conflict ordering

**Priority:** P1 · **Roadmap:** Phase 1 (convergence) · **Depends on:** 00165 (sequence after; both concern deterministic convergence)

> **Scope addition (2026-07-16 assessment):** `find_hlc_for_path` gives up at revwalk depth 100 (`git_ops/read.rs:82-97`); a `None` HLC silently degrades add-add and binary LWW tie-breaks to theirs-wins (`sync_manager/mod.rs:133-135,461-463`). Handle the cap explicitly — raise it, index HLCs, or fail loud — and pin with a test. Evidence: evolution-assessment-2026-07-16.md L1.

## Overview

### Problem Statement
The Hybrid Logical Clock is vestigial for the last-writer-wins decisions it supposedly governs. Every ordering input it feeds is empty or contradictory:

- **No write commit carries an HLC trailer, and there is no shared commit helper**: every git write path in `git_ops/mod.rs` builds its commit from the bare `message` with no trailer. The eight write functions (`commit_file:254`, `commit_binary_file`, `commit_files`, `delete_file:349`, `delete_files:377`, `rename_file`, `commit_batch`, `commit_binary_and_text`) each call `repo.commit(...)` directly — seven distinct `.commit(` sites with no common chokepoint. Only conflict (and sweep) commits carry a trailer, via `tick_hlc` + `append_hlc_trailer` in `create_conflict_commit` (`ddb-core/src/sync_manager/mod.rs:346-347`). So delete/rename/batch commits are trailer-less exactly like plain creates — a partial fix that touches only `commit_file`/`commit_files` would still pass a naive create-only acceptance test yet leave the HLC non-load-bearing for every other write path.
- **So the HLC lookup almost always returns None**: `find_hlc_for_path` (`ddb-core/src/git_ops/read.rs:81`) walks history via `check_commit_for_hlc` (`read.rs:104-145`), whose contract (`read.rs:100-103`) is: a commit that **touches the path but has no trailer** returns `Some(None)` and the walk **stops**. Because ordinary content commits have no trailer, the walk stops at the first one and yields None.
- **Remote HLCs are never absorbed**: `recv_hlc` (`sync_manager/mod.rs:854-859`) has **zero** callers anywhere in the codebase (grep finds only its definition). A device that has seen a peer's future clock never advances its own.
- **The two missing-HLC tie-breaks disagree**: add-add `resolve_add_add_collision` defaults to **theirs wins** (`sync_manager/mod.rs:133-136`), while `pick_lww_winner` defaults to **ours wins** (`ddb-core/src/crdt_resolver.rs:578-589`). Opposite directions on different code paths.
- **The clock primitive itself is already correct**: `Hlc::now` (`ddb-core/src/hlc.rs:22-43`) is monotonic under wall-clock regression (it reuses `prev.wall_ms` and bumps the counter). The defect is entirely that nothing writes, reads, or absorbs the value.

### Target Users
Multi-device users (jink) who expect a correct, predictable last-writer-wins across their devices instead of a clock that never participates.

### Success Metrics
- Every write commit — create, update, delete, rename, and batch — carries a parseable HLC trailer; `find_hlc_for_path` returns `Some(hlc)` after a create AND after a delete/rename/batch commit — pinned by a test that FAILS if any commit-producing path omits the trailer.
- LWW picks the higher-HLC side deterministically across two devices (not by ours/theirs role) — pinned by a two-device test.
- `recv_hlc` is called on merge and advances the local clock past a remote's future HLC — pinned by a test asserting `local.wall_ms >= remote.wall_ms` after merge.
- A single documented missing-HLC tie-break, applied identically in both `resolve_add_add_collision` and `pick_lww_winner`; the previously-opposite defaults reconciled.
- A device skewed ahead does not win indefinitely: once its HLC is absorbed, the peer's next writes tie or beat it — pinned by a test.

## Functional Decomposition

### Capability: Load-bearing HLC ordering

#### Feature: HLC trailer on all write commits
- **Description**: repo write commits append an HLC trailer sourced from the node's HLC state.
- **Inputs**: the commit message; the node HLC (already persisted in `NodeConfig.hlc` and ticked by `tick_hlc`).
- **Outputs**: a commit message carrying `append_hlc_trailer(msg, tick_hlc())`.
- **Behavior**: route every commit-producing path through ONE shared commit helper in `git_ops` that appends `append_hlc_trailer(msg, tick_hlc())` centrally, OR explicitly append the trailer in each of the eight write functions (`commit_file`, `commit_binary_file`, `commit_files`, `delete_file`, `delete_files`, `rename_file`, `commit_batch`, `commit_binary_and_text` — the same set 00162 wraps in `with_write_lock`). The shared-helper route is preferred so no future commit path can silently skip the trailer. Each write ticks and persists the node HLC.

#### Feature: Absorb remote HLC on merge
- **Description**: merging a peer's history advances the local clock via `recv_hlc`.
- **Inputs**: theirs' HLC, extracted from the incoming commit trailer (now present) or the conflict file's `theirs_hlc`.
- **Outputs**: an advanced, persisted local HLC.
- **Behavior**: call `recv_hlc(theirs_hlc)` on the merge path so a device that has seen a future clock advances; wire the one call site that today does not exist.

#### Feature: Unified missing-HLC tie-break
- **Description**: one documented rule when an HLC is absent on one or both sides, applied at both decision sites.
- **Inputs**: `ours_hlc`, `theirs_hlc` (one or both None).
- **Outputs**: the chosen winner.
- **Behavior**: decide and implement ONE rule at `resolve_add_add_collision` (`:133-136`) and `pick_lww_winner` (`:578-589`). Recommended: a content-deterministic fallback (e.g. higher blob hash wins) so it converges under role-swap like 00165; minimum: one fixed direction. Record the decision.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/git_ops/
├── mod.rs             # Maps to: HLC trailer on all write commits (route ALL eight write fns through one commit helper that appends the trailer — the same set 00162 wraps; not just commit_file/commit_files)
├── read.rs            # Maps to: (now productive) find_hlc_for_path :81 / check_commit_for_hlc :104-145 find trailers on content commits
ddb-core/src/sync_manager/
├── mod.rs             # Maps to: Absorb remote HLC (call recv_hlc :854-859 on merge); Unified missing-HLC tie-break (resolve_add_add_collision :133-136)
ddb-core/src/
├── crdt_resolver.rs   # Maps to: Unified missing-HLC tie-break (pick_lww_winner :578-589)
└── hlc.rs             # (unchanged) Hlc::now :22-43 already monotonic; Hlc::recv is the merge primitive
```

### Module: git_ops (ddb-core/src/git_ops/mod.rs, read.rs)
- **Maps to capability**: Load-bearing HLC ordering
- **Responsibility**: append an HLC trailer to every write commit so `find_hlc_for_path` returns a value for content commits.
- **Exports**: `commit_file` / `commit_files` (trailer-appending); `find_hlc_for_path` (now yields `Some`)

### Module: sync_manager (ddb-core/src/sync_manager/mod.rs)
- **Maps to capability**: Load-bearing HLC ordering
- **Responsibility**: absorb the remote HLC on merge; own the unified missing-HLC tie-break for add-add.
- **Exports**: `recv_hlc` (now called on merge); `resolve_add_add_collision` (unified fallback)

### Module: crdt_resolver (ddb-core/src/crdt_resolver.rs)
- **Maps to capability**: Load-bearing HLC ordering
- **Responsibility**: apply the identical unified missing-HLC tie-break in the LWW path.
- **Exports**: `pick_lww_winner` (unified fallback)

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **HLC trailer on write commits**: the source of truth; without it every downstream read is None.

### Core Layer (Phase 1)
- **absorb remote HLC (recv_hlc)**: Depends on [HLC trailer] - theirs' HLC is only extractable once commits carry it.
- **unified missing-HLC tie-break**: Depends on [HLC trailer] - the fallback fires far less once trailers exist, so unify it against the reduced surface.

### Integration Layer (Phase 2)
- **two-device tests + docs**: Depends on [recv_hlc, unified tie-break].

## Implementation Phases

### Phase 0: Foundation
**Goal**: make the HLC observable.

**Tasks**:
- [ ] Append an HLC trailer on all repo write commits by routing every commit-producing path through one shared `git_ops` commit helper (or appending in each of the eight write functions — the set 00162 wraps in `with_write_lock`), sourced from node HLC state (no deps) - Acceptance: `find_hlc_for_path` returns `Some(hlc)` not only after a create but also after an update, a **delete**, a **rename**, and a **batch** commit; the test FAILS if any commit-producing path (delete/rename/batch included) omits the trailer; the node HLC advanced and persisted.

**Exit Criteria**: content commits carry HLC trailers; lookups return values.

### Phase 1: Core
**Goal**: absorb remote clocks and reconcile the tie-break.

**Tasks**:
- [ ] Call `recv_hlc(theirs_hlc)` on the merge path (depends on: Phase 0) - Acceptance: merging a commit whose HLC is far ahead leaves `local.wall_ms >= remote.wall_ms`; a skewed-ahead peer stops winning once absorbed.
- [ ] Decide + implement ONE missing-HLC tie-break at `resolve_add_add_collision` and `pick_lww_winner`; record the decision (depends on: Phase 0) - Acceptance: both sites use the same rule; a test pins it; the previously-opposite defaults no longer disagree.

**Exit Criteria**: remote HLCs advance the local clock; one documented tie-break.

### Phase 2: Integration
**Goal**: prove ordering across two devices and document it.

**Tasks**:
- [ ] Add two-device tests: LWW picks the higher-HLC side; `recv_hlc` advances the clock; a skewed device does not win indefinitely; add a two-device LWW scenario to `integration.sh`/`.ps1` (depends on: Phase 1) - Acceptance: all fail against the vestigial-HLC code and pass after.
- [ ] Update `docs/src/technical/sync.md` (HLC now written on all commits, absorbed on merge, one tie-break) + `docs/src/technical/crdt-resolver.md` + CHANGELOG (depends on: Phase 1) - Acceptance: docs match. Internal sync change, no new CLI command, so no walkthrough required.

**Exit Criteria**: two-device ordering tests + docs landed.

## Test Strategy

### Critical Scenarios
- **Happy path**: create a doogat, read its HLC via `find_hlc_for_path` → Expected: `Some(hlc)`, monotonic with prior writes.
- **Edge case**: two devices edit the same doogat; device B's HLC is higher → Expected: LWW picks B on both devices, independent of local ours/theirs role.
- **Error case**: device B's wall clock is far ahead; device A merges B, then B merges A → Expected: A absorbs B's HLC (`recv_hlc`), A's next write ties/beats B; B does not win indefinitely.

## Risks

- **Layer boundary**: `git_ops` should not own HLC/node state. Mitigation: inject an HLC source into the write path (DI, consistent with the guardrails) or compose the trailer above `git_ops`; do not construct node state inside `GitRepo`.
- **Every commit now ticks + persists the HLC**: more node-toml writes. Mitigation: `tick_hlc` already persists on conflict commits; the cost is one small write per commit; measure in the two-device scenario.
- **Tie-break role-dependence**: an ours/theirs fallback does not converge under role-swap (same flaw 00165 fixes). Mitigation: prefer the content-deterministic fallback; if a fixed direction is chosen, document that it only converges once HLCs are present (the common case after Phase 0).
- **Legacy commits without trailers**: pre-fix history stays trailer-less. Mitigation: the walk already handles `Some(None)`; the fallback covers legacy pairs; no history rewrite.

## Transport Impact

- **CLI**: internal — `ddb sync` ordering becomes correct; no public surface change.
- **GraphQL**: internal — sync mutations unchanged in shape; only ordering becomes deterministic.
- **REST**: internal — no public surface change.
- **PgWire**: internal — no public surface change.
- **FFI**: internal — driver sync converges by HLC; no signature change.
- **NoSQL HTTP**: internal — read-only mirror reflects the HLC-ordered winner; no surface change.
- **Cross-interface conformance plan**: core two-device tests pin higher-HLC-wins, `recv_hlc` advancement, and bounded skew; an `integration.sh`/`.ps1` two-device LWW scenario pins end-to-end ordering. No new transport surface, no response/error shape change.
