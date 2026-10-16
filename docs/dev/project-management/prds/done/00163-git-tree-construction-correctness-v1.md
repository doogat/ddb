# Git tree-construction correctness: delete freshness, merge locking

**Priority:** P0 (data-loss) · **Roadmap:** Phase 0 (stop the bleeding) · **Depends on:** none · **Pairs with:** 00162 (write lock); 00200 follows immediately (merge-tree three-way rework).

> **Provenance:** H1/H2 from the 2026-07 audit; H3 (merge/sync lock + fresh merge staging) and the CLI `resurrected` visibility added by the 2026-07-16 assessment (F1/L10, `dev/local/audit-results/evolution-assessment-2026-07-16.md`) and folded into the body at the 2026-07-16 backlog review. Line refs verified at HEAD `8e4b4aa`.
>
> **Amended 2026-07-16 (maintainer decision: split).** Design-review dispatch 1 proved the
> prescribed H1 patch (`Delta::Deleted` arm gated by `resolved_set`) would itself destroy
> ours-created doogats, and found a worse pre-existing defect: the existing `Added|Modified`
> arms silently revert ours' non-conflicting edits on every conflicted merge — a two-way diff
> structurally cannot express the three-way merge it simulates. **H1 (merge-tree content
> correctness) moved to PRD `00200-make-merge-tree-three-way-v1.md`** together with the newly
> found defect and the resurrect-policy regression guard. This PRD keeps H2 (delete
> freshness), H3 (merge locking + fresh staging — the staging BASE only; delta-arm semantics
> stay byte-identical), and the CLI `resurrected` line. Evidence:
> `dev/local/designs/00200-make-merge-tree-three-way-v1-evidence.md` § Review log.

## Overview

### Problem Statement
Two write paths build git trees from a stale base or run outside the write lock, and silently lose data. All are small diffs against `ddb-core/src/git_ops/`.

**H1 — moved to 00200.** Conflicted-merge tree content (deletion preservation, rename handling, and the pre-existing non-conflicting-edit reversion found in design review) is PRD 00200's scope. This PRD does not change any delta-arm semantics in `collect_theirs_only_changes`.

**H2 — delete uses the stale cached git index.** `ddb-core/src/git_ops/mod.rs:401` (`delete_file`) and `:427` (`delete_files`) call `self.repo.index()` while every other commit path uses `fresh_index()` — whose own docstring (`:259-263`) warns the cached index "can silently revert those changes on the next commit." Scenario: `ddb serve` is running; a CLI commits new doogat F; the server's next `delete_doogat(G)` builds its tree from the pre-F index snapshot → the commit removes G *and* F. Silent loss.

**H3 — the merge/sync write path runs entirely OUTSIDE the 00162 write lock and also stages from the cached index.** `merge.rs:32` (`build_merge_tree_and_commit` stages via cached `self.repo.index()`), `:47-54` (merge commit to HEAD), `:169-178` (`perform_normal_merge` commit + forced checkout), `:199-211` (fast-forward `set_target` + forced checkout) — none inside `with_write_lock`; all lock sites live in `git_ops/mod.rs` and cover only the 8 CRUD/rename write functions. Callers: `sync_manager/mod.rs:246` (`merge_remote`), `:355` (`commit_merge`), and bundle import. Scenario: a locked CRUD writer sits between its worktree write and `index.add_path` when an unlocked merge runs `checkout_head(force)` — the writer's file is reverted, or the merge commits from a stale parent and `git maintenance --auto` prunes the orphaned commit. The exact silent-loss class 00162 shipped to kill, still open on the primary deployment shape (jink's `ddb serve` sync racing a CLI write).

Both are verified against source. Together with 00162 and 00200 they close the write-path data-loss class.

### Target Users
Any deployment running `ddb serve` alongside CLI writes (deletes and merges must not revert or orphan concurrent writes).

### Success Metrics
- `delete_file`/`delete_files` build their tree from `fresh_index()`, so a delete concurrent with an external create removes only the intended path.
- The three merge write sections (conflicted merge, normal merge, fast-forward) run inside the re-entrant `with_write_lock` and stage from fresh state: a concurrent locked write during a merge is neither reverted by the forced checkout nor orphaned by a stale-parent commit — pinned by a concurrency regression test.
- `ddb sync` output prints `SyncReport.resurrected` (today `ddb-cli/src/commands/sync.rs:12-38` omits the single most safety-relevant field).
- Regression tests for all scenarios fail against the current code and pass after the fix.
- Existing delta-arm behavior in `collect_theirs_only_changes` is byte-identical (its correctness rework is 00200; this PRD must not touch arm semantics).
- `docs/src/technical/invariants.md` I1's lock leg narrows to bundle-import-only (00168); the deletion-preservation leg stays OPEN, owned by 00200 — I1 is NOT marked closed by this PRD.

## Functional Decomposition

### Capability: Sync outcome visibility
#### Feature: Resurrected count visible in CLI sync output
- **Description**: the deliberate resurrect policy's outcome is visible to the operator.
- **Inputs**: `SyncReport.resurrected` (`sync_manager/mod.rs:312`), already populated by the resurrect policy.
- **Outputs**: one output line in `ddb sync` naming the resurrected count when non-zero.
- **Behavior**: one `outln!` in `ddb-cli/src/commands/sync.rs` alongside the existing direction/commits/conflicts lines; zero stays silent or prints 0 consistently with the neighboring fields.

### Capability: Delete builds from a fresh index
#### Feature: fresh_index in delete paths
- **Description**: deletes stage removal against the HEAD-rebased index.
- **Inputs**: `delete_file`, `delete_files`.
- **Outputs**: both use `fresh_index()` before `remove_path`.
- **Behavior**: replace `self.repo.index()` with `self.fresh_index()` in both functions so externally-committed entries are visible and not reverted; no other change.

### Capability: Merge path locked and fresh
#### Feature: Merge writes hold the write lock
- **Description**: every merge-path git write (commit, ref move, forced checkout) runs inside the repo-scoped advisory write lock.
- **Inputs**: `commit_merge`, `perform_normal_merge`, and the fast-forward branch of `merge_remote`.
- **Outputs**: the three write sections wrapped in the re-entrant `with_write_lock` (re-entrancy depth already supports nesting; sites are in `merge.rs`, the lock lives in `git_ops/mod.rs`).
- **Behavior**: acquire around the write section (resolve/stage/commit/checkout), not around the whole fetch; callers in `sync_manager` and bundle import inherit the protection with no signature change.

#### Feature: Merge tree staged from fresh state
- **Description**: the merge tree is built from fresh repository state, not the cached `self.repo.index()`.
- **Inputs**: `build_merge_tree_and_commit` (`merge.rs:32`).
- **Outputs**: a merge tree that reflects concurrently-landed commits (inside the lock, HEAD is stable for the section).
- **Behavior**: swap the staging BASE from the cached index to `fresh_index()` before staging. Delta-arm semantics stay byte-identical (00200 owns their rework); combined with the lock, no stale-parent commit is possible.

## Structural Decomposition

### Repository Structure
```text
ddb-core/src/git_ops/
├── merge.rs   # Maps to: Merge writes hold the write lock; Merge tree staged from fresh state
├── mod.rs     # Maps to: fresh_index in delete_file / delete_files (with_write_lock stays here; merge.rs calls it)
ddb-cli/src/commands/
└── sync.rs    # Maps to: Resurrected count visible in CLI sync output
tests/e2e/
├── delete_concurrency.rs  # Maps to: delete-vs-external-create regression (new)
└── merge_lock.rs          # Maps to: merge-vs-concurrent-write regression (new; or colocated unit test if two processes are not required)
```

### Module: merge (ddb-core/src/git_ops/merge.rs)
- **Maps to capability**: Merge path locked and fresh
- **Responsibility**: run every merge write section inside the write lock, staging from fresh state. Delta-arm content semantics untouched (00200).
- **Exports**: `commit_merge` / `merge_remote` — unchanged signatures, locked bodies.

### Module: git_ops (ddb-core/src/git_ops/mod.rs)
- **Maps to capability**: Delete builds from a fresh index
- **Responsibility**: `delete_file`/`delete_files` use `fresh_index()`; `with_write_lock` remains the single lock primitive.
- **Exports**: unchanged signatures.

### Module: cli sync (ddb-cli/src/commands/sync.rs)
- **Maps to capability**: Sync outcome visibility
- **Responsibility**: print the full `SyncReport`, `resurrected` included.
- **Exports**: unchanged command surface; one added output line.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies.
- **H2 fresh_index swap**: two-line change, independent.
- **H3 merge lock + fresh staging**: independent of H2 mechanically.

### Core Layer (Phase 1)
- **regression tests**: Depends on [H2, H3].

### Integration Layer (Phase 2)
- **CLI visibility + docs**: Depends on [tests].

## Implementation Phases

### Phase 0: Foundation
**Goal**: fix the delete-freshness and merge-locking bugs.

**Tasks**:
- [ ] `delete_file`/`delete_files`: swap `self.repo.index()` → `self.fresh_index()` (no other change) — Acceptance: a unit/e2e test where an external commit lands a file F between repo open and a delete of G shows the delete commit removes only G, F survives; fails on the old code.
- [ ] Wrap the three merge write sections (`commit_merge` body, `perform_normal_merge`, the fast-forward branch) in `with_write_lock` and stage the merge tree from `fresh_index()` (staging base only; delta-arm semantics byte-identical — 00200 owns their rework) — Acceptance: a concurrency regression test where a locked CRUD write races a merge shows the write neither reverted by the forced checkout nor orphaned by a stale-parent merge commit; fails on the old code.

**Exit Criteria**: both fixes in; targeted tests green; the pre-fix code fails them.

### Phase 1: Integration
**Goal**: visibility, docs.

**Tasks**:
- [ ] Print `SyncReport.resurrected` in `ddb sync` output (one line, consistent with neighboring fields) — Acceptance: a sync that resurrects a file shows the count; the CLI output test covers it.
- [ ] Update `docs/src/technical/git-ops.md` and `sync.md` for the merge-locking + fresh-staging rules; narrow `invariants.md` I1's lock leg to bundle-import-only (00168) while stating the deletion-preservation leg stays OPEN (owned by 00200); add a `CHANGELOG.md` `Fixed` entry covering the delete-freshness and merge-locking regressions (depends on: Phase 0) — Acceptance: docs + invariants wording match; CHANGELOG entry present. (The deletion-preservation sync scenario and `crdt-resolver.md` update move to 00200 with the behavior itself.)

**Exit Criteria**: visibility + docs landed.

## Test Strategy

### Critical Scenarios
- **Error case**: delete G while an external process committed F → delete commit removes G only; F intact (this is also a 00162 concurrency case; here it is the tree-construction half).
- **Concurrency case**: a locked CRUD write races `ddb sync`'s merge → the write survives the merge's forced checkout, and the merge commit's parent is current HEAD (no orphaned commit for maintenance to prune).
- **Behavior-preservation guard**: an ordinary conflicted merge produces the same tree before and after this PRD (arm semantics untouched; only staging base + locking change).

## Risks
- **Lock hold spans the merge write section**: a long conflict resolution inside the lock extends the hold. Mitigation: acquire around the write section only (post-resolution staging/commit/checkout), not the fetch or the CRDT resolution compute; the 10 s acquire timeout stays the loud-failure backstop.
- **Commit-graph subprocess inside the lock** (design-review NB-3): the wrapped `merge_remote` body calls `write_commit_graph()`, which shells out to `git commit-graph write --reachable` — suppressed on the sync path but NOT for other `merge_remote` callers, who could push a concurrent writer into a spurious `Conflict` on a large repo under the 10 s timeout. The design must address (suppress inside the lock, or move outside the locked section).
- **Accidental semantic drift in `build_merge_tree_and_commit`**: swapping the staging base must not alter arm behavior. Mitigation: the behavior-preservation guard scenario above.

## Transport Impact
- **CLI / GraphQL / REST / FFI**: Guaranteed — all deletes and all syncs route through these git paths; the fix is transport-agnostic (behavior correctness, no surface change beyond the CLI's added `resurrected` line).
- **PgWire**: Guaranteed — SQL DELETE lands through `delete_*`.
- **NoSQL HTTP**: Intentionally absent — read-only surface; unaffected.
- **Cross-interface conformance plan**: a `delete_concurrency` e2e (delete-vs-external-create) plus the merge-vs-concurrent-write regression pin the behavior; the only public surface change is the additive CLI `resurrected` line (documented in `sync.md`). No new conformance driver is required. (Merge-content conformance moves to 00200.)
