# Make the conflicted-merge tree a true three-way merge

**Priority:** P0 (data-loss, SHIPPING today) · **Roadmap:** Phase 0 (stop the bleeding) · **Depends on:** 00163 (merge lock + fresh staging land first; this PRD reworks tree construction inside that locked section). · **Sequencing:** run IMMEDIATELY after 00163 — maintainer decision 2026-07-16 (split from 00163; see Provenance).

> **Provenance:** split out of 00163 by maintainer decision 2026-07-16 after design-review
> dispatch 1 proved the PRD-prescribed `Delta::Deleted` patch unsound AND found a pre-existing
> P0 the audit did not know about. Full empirical evidence (real-git repros at HEAD `8e4b4aa`)
> lives in `dev/local/designs/00200-make-merge-tree-three-way-v1-evidence.md` (§ Review log —
> the archived design doc of pre-split 00163, dispatch 1).

## Overview

### Problem Statement

`collect_theirs_only_changes` (`ddb-core/src/git_ops/merge.rs`) replays a TWO-WAY diff
(`diff_tree_to_tree(old=ours, new=theirs)`) as if it were a THREE-WAY merge. A two-way diff
structurally cannot distinguish "theirs changed it" from "ours changed it", so every delta arm
is wrong in some case, and patching arms cannot fix the class:

**W1 — conflicted merges silently revert ours' non-conflicting edits (pre-existing, worst).**
A file edited only by ours since the merge base reports as `Modified` (ours ≠ theirs), is not a
conflict (not in `resolved_set`), and the existing `Added|Modified` arms overwrite ours' edit
with theirs' blob — which still holds BASE content. Verified empirically: `git show
theirs:Z.md` = `base-Z` overwrites ours' `OURS-IMPORTANT-EDIT`. Every conflicted sync loses
local edits made since the merge base. By symmetry the `Added` arm resurrects ours' clean
deletes (ours deleted D → absent in ours, present in theirs → reports `Added` → rewritten).

**W2 — conflicted merges resurrect theirs' deletions (the original 00163 H1).** The arms skip
`Delta::Deleted`, so a file theirs deleted stays in the merge tree and resurrects on every
node, bypassing the intentional delete-vs-edit resurrect policy
(`ddb-core/src/sync_manager/mod.rs`, `resolve_delete_edit_conflicts`). A theirs-side rename
(Deleted+Added pair) leaves the old path alive → duplicated doogat.

**W3 — the naive fix for W2 is itself a data-loss bug (why 00163 could not just patch it).**
Adding a `Delta::Deleted` arm gated only by `resolved_set` deletes every doogat OURS created
since the merge base (ours-added → "in old, not in new" → `Deleted`, never a conflict).
Verified empirically: ours-created `N.md`/`Z.md` report `D` in `git diff OURS THEIRS`.
`resolved_set` is necessary but NOT sufficient to gate any arm.

The correct classification needs the merge base. It is already computed and discarded at the
`let _ancestor = self.repo.merge_base(...)` line in `merge.rs` (pre-split line 153).

### Target Users

Multi-device users: local edits, creates, and deletes made since the merge base must survive a
conflicted sync, and the other side's deletions must stay deleted.

### Success Metrics

- Ours' non-conflicting edits since the merge base survive a conflicted merge (W1 repro fails
  on old code, passes after).
- Ours' clean deletes stay deleted through a conflicted merge (W1 symmetry case).
- Ours' newly-created doogats survive a conflicted merge (W3 repro).
- A file edited by BOTH sides in different regions (auto-mergeable, no conflict entry) keeps
  both sides' line edits through a conflicted merge (W4 repro; fails on old code and on any
  per-path whole-blob classification).
- Theirs' deletions are absent from the merge tree (W2), and a theirs-side rename yields one
  doogat at the new path, none at the old.
- The intentional resurrect policy (`sync_manager` delete-vs-edit) still fires for genuine
  edit-vs-delete conflicts — resurrect-with-marker preserved, no regression.
- All regression tests fail against pre-fix code.
- `docs/src/technical/invariants.md` I1's "Merges preserve the other side's deletions" leg
  flips to closed (the lock leg is 00163's; bundle import stays 00168's).

### Design decision — TAKEN (maintainer, 2026-07-17): Alternative B

> **Maintainer decision 2026-07-17 (interactive, pre-loop):** build the conflicted-merge tree
> from **libgit2's `merge_commits` in-memory index, overlaying the CRDT-resolved blobs onto
> its conflict entries** (Alternative B). The design phase implements this decision; it does
> NOT re-open A-vs-B. Rationale, in force:
>
> 1. **Symmetry/reuse**: the clean path already builds its tree exactly this way
>    (`perform_normal_merge`, `merge.rs:155-166` at v0.2.7 / 58d7d22). B makes the conflicted
>    path use the construction the codebase already trusts; the hand-rolled rebuild is the
>    outlier.
> 2. **Kills the class**: hand-rolled classification was wrong twice (the original two-way
>    arms; the prescribed `Delta::Deleted` patch). B deletes the "we forgot a case" class
>    instead of patching it.
> 3. **W4 (decisive)**: when both sides edit the SAME file in different regions and a real
>    conflict exists elsewhere, `merge_commits` auto-merges that file line-level; any per-path
>    whole-blob classification (A+) silently drops ours' line edits in it. Only B preserves
>    them without reimplementing per-file merge. (Verified by code reading, `merge.rs:111-137`
>    vs `:157-163`; the W4 matrix row below turns it into a runtime repro.)
> 4. **Fail loud**: if the re-run merge's conflict set diverges from the resolved set (HEAD
>    moved in the resolve→commit window), `write_tree_to` refuses the index — a loud,
>    retryable error, never a silent wrong tree.
>
> Rejected — **A+ (ancestor-gated three-way)**: smallest diff, passes W1/W2/W3 and both rename
> directions, but structurally loses W4 (whole-blob choice per path) and stays in the
> twice-wrong hand-rolled class. The evidence doc's original rejection of B was made on a
> false premise (its Blocker 2) and is superseded by this decision.
>
> Design-phase obligations that stay open (implementation shape, not direction): overlay
> mechanics for CRDT text blobs, `binary_paths` (chosen-side blob oids), and resurrect-marker
> files; conflict-set divergence handling; lock-hold note (`merge_commits` inside the locked
> `commit_merge` is in-memory, milliseconds at ddb scale).

The evidence doc's repro matrix (W1/W2/W3 + renames both directions) **plus W4** is the
acceptance floor.

## Functional Decomposition

### Capability: Conflicted merge preserves all non-conflicting work
#### Feature: Three-way classification of merge-tree contents
- **Description**: the merge tree reflects, per path: theirs' change when only theirs changed
  it, ours' state when only ours changed it, and the CRDT-resolved blob when both did.
- **Inputs**: base (merge-base tree), ours tree, theirs tree, `resolved_set`.
- **Outputs**: a merge tree where no non-conflicting local work is lost and no theirs-deletion
  is resurrected.
- **Behavior**: per the taken decision — B: `merge_commits` index + resolved-blob overlay.
  Both-edited auto-mergeable files carry git's line-merged blob (W4). Rename pairs
  (Deleted+Added) resolve to a single moved path in both directions (ours-side and
  theirs-side).

#### Feature: Resurrect policy regression guard
- **Description**: the deliberate delete-vs-edit resurrect-with-marker behavior is pinned.
- **Inputs**: existing resurrect tests in `ddb-core/src/sync_manager/tests.rs`.
- **Outputs**: a test distinguishing "theirs deleted, ours untouched → stays deleted" from
  "theirs deleted, ours edited → resurrect with marker".
- **Behavior**: both paths asserted; runs under `cargo test-ci`.

## Structural Decomposition

### Repository Structure
```text
ddb-core/src/git_ops/
├── merge.rs   # collect_theirs_only_changes removed; tree built from merge_commits' index + resolved-blob overlay (B, decided 2026-07-17)
ddb-core/src/sync_manager/
└── tests.rs   # resurrect-policy regression pair
tests/e2e/     # merge data-preservation e2e if two processes are required; else colocated unit tests
```

### Module: merge (ddb-core/src/git_ops/merge.rs)
- **Maps to capability**: Conflicted merge preserves all non-conflicting work
- **Responsibility**: build the merge tree as a true three-way inside the write lock 00163
  established, staged from fresh state.
- **Exports**: `commit_merge` / `merge_remote` signatures unchanged; internal tree-construction
  reworked.

## Dependency Graph

### Foundation Layer (Phase 0)
- **Three-way tree construction**: depends on 00163's lock + fresh staging being merged (the
  locked section is where this rework lives).

### Core Layer (Phase 1)
- **Regression suite (W1/W2/W3, renames both directions, resurrect policy)**: depends on
  Phase 0.

### Integration Layer (Phase 2)
- **Sync scenario + docs**: depends on Phase 1.

## Implementation Phases

### Phase 0: Foundation
**Goal**: replace the two-way replay with a correct three-way construction.

**Tasks**:
- [ ] Rework merge-tree construction to B (`merge_commits` index + resolved-blob overlay;
  decided 2026-07-17) inside the locked merge section — Acceptance: W1 (ours-edit survives),
  W1-symmetry (ours-delete stays deleted), W3 (ours-create survives), W2 (theirs-delete
  absent), W4 (both-edited auto-mergeable file keeps both sides' line edits) all pass; each
  fails on pre-fix code.
- [ ] Rename handling both directions — Acceptance: theirs-side and ours-side renames yield
  one doogat at the new path; both fail on pre-fix code.

**Exit Criteria**: repro matrix green; pre-fix code fails every case.

### Phase 1: Core
**Goal**: pin the policy boundary.

**Tasks**:
- [ ] Resurrect-policy regression pair (stays-deleted vs resurrect-with-marker) — Acceptance:
  both asserted under `cargo test-ci`.

**Exit Criteria**: policy behavior pinned.

### Phase 2: Integration
**Goal**: scenario coverage and docs.

**Tasks**:
- [ ] e2e sync scenario: theirs-deletes-then-conflict, plus ours-edit-survives-conflict
  (Rust e2e harness — post-00171 target; shell harness only if 00171 has not landed yet) —
  Acceptance: scenario asserts deletion survives AND local edit survives the conflicted merge.
- [ ] Docs: `docs/src/technical/crdt-resolver.md` + `git-ops.md` + `sync.md` describe the
  three-way rule; `invariants.md` I1 deletion leg flips closed; `CHANGELOG.md` `Fixed` entries
  for W1 and W2 — Acceptance: docs match implementation; CHANGELOG present.

**Exit Criteria**: scenario + docs landed.

## Test Strategy

### Critical Scenarios
- **Happy path**: theirs deletes Y, ours edits X (conflict on X) → Y absent, X resolved, ours'
  other edits intact.
- **Edge case**: renames on either side during a conflict → single doogat at new path.
- **Error case (W1)**: ours edited Z only, conflict elsewhere → Z keeps ours' edit (fails
  pre-fix).
- **Error case (W3)**: ours created N, conflict elsewhere → N survives (fails on the naive
  Deleted-arm patch; guards the fix's own regression).
- **Error case (W4)**: ours edits X's body, theirs edits X's frontmatter (auto-mergeable),
  conflict on Y → X contains BOTH edits (fails pre-fix; kills per-path whole-blob approaches).
- **Policy boundary**: delete-vs-edit conflict → resurrect-with-marker still fires.

## Risks
- **Case-enumeration correctness (A+)**: moot — B decided 2026-07-17; no hand-rolled
  classification remains.
- **Conflict-entry handling (B)**: `write_tree_to` refuses an index with conflicts; overlay
  logic must re-map resolved blobs (CRDT text, binary chosen-side oids, resurrect markers).
  Mitigation: early implementation spike in the design phase — mechanics only, the direction
  is decided. Conflict-set divergence (HEAD moved in the resolve→commit window) surfaces as a
  loud retryable error, never a silent tree.
- **Resurrect-policy interaction**: preserving deletions must not swallow the legitimate
  resurrect-with-marker case. Mitigation: the Phase 1 regression pair.

## Transport Impact
- **CLI / GraphQL / REST / FFI / PgWire**: Guaranteed — every sync/merge routes through this
  path; the fix is transport-agnostic behavior correctness with no surface change.
- **NoSQL HTTP**: Intentionally absent — read-only surface; unaffected.
- **Cross-interface conformance plan**: the merge unit/e2e repro matrix pins behavior for all
  transports (single code path); no per-transport surface change, no new conformance driver.
