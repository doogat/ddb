---
catchup: force
design: skip
default_model: opus
model_tier_rationale: merge-history predicate on the LWW/singleton winner path - a wrong turn silently mis-picks winners; design skipped because the fix is one predicate whose exact rule is pinned below
---

# Fix merge-aware HLC path attribution (singleton winner regression)

**Priority:** P1 (correctness — the HLC-later singleton write is quarantined as the loser; Nightly red on 3 platforms since the first post-00166 CI run) · **Roadmap:** inserted 2026-09-05 by maintainer decision (backlog review G1); **runs BEFORE 00171's task 13** (operator step: run this PRD explicitly via `/run-autopilot` while the batch is paused, then resume 00171) · **Depends on:** none (00166 done — this fixes a regression it introduced) · **Source:** `dev/local/autopilot/deferred/202607161128-deferred.json` topic `nightly-red-singleton-hlc-winner`; Nightly run 33851394880 (2026-09-04).

## Overview

### Problem Statement
`ddb-core/tests/sync_test.rs::two_node_singleton_post_sync_resolution` (`:219`) fails on linux-core, macos-full, and windows-full in every Nightly since 2026-08-06 (10 consecutive runs to 2026-09-04): the assertion at `:310` expects the HLC-later writer (`20260601000010`, HLC wall 2000, node B) to win the singleton conflict, but the sweep picks `20260601000000` (HLC wall 1000, node A). Node B's sync merges A's commit, then `singleton_sweep` (`ddb-core/src/consistency/singleton_sweep.rs:26`) ranks the two `app_config` rows by `repo.find_hlc_for_path(HEAD, path)` (`:67`, comparator `:149-154`) and quarantines the loser.

Root cause (verified by code reading and git history, confirmed by bisect in Phase 0): `find_hlc_for_path` (`ddb-core/src/git_ops/read.rs:81`) walks history topologically and asks `check_commit_for_hlc` (`:104-146`) whether each commit "touched" the path — by diffing the commit's tree against **`parent(0)` only** (`:120-123`). A merge commit therefore counts as touching every file that arrived from its second parent, and the walk stops there and returns the merge commit's own HLC. Since `c33b0a7` (00166, 2026-07-18: "absorb theirs' HLC and stamp both merge commits + fast-forward") merge commits carry an HLC trailer at least as late as either side, so on B the file merged in from A is attributed the merge's HLC (> 2000) and A wins. Before `c33b0a7` the merge commit had no trailer, `extract_hlc` returned `None`, the comparator fell back to id order, and B won by accident — which is why the test passed at the last green Nightly (`8d3868d`, 2026-07-17). The same helper feeds `lww_pick` for conflicted paths (`sync_manager/mod.rs:615-616`), so the mis-attribution can also skew last-writer-wins resolution after a prior merge.

Premise (re-check at execution): `check_commit_for_hlc` still diffs against `parent(0)` only, and merge commits still carry an HLC trailer. If either premise no longer holds, stop and report instead of forcing the change.

### Target Users
Every multi-device user (jink included): a singleton edit made later on device B must not be quarantined because device A's row arrived through a merge; last-writer-wins must rank by the writing commit's HLC, never by the merge that transported it.

### Success Metrics
- `cargo test -p ddb-core --test sync_test two_node_singleton_post_sync_resolution` passes at HEAD on the host (the Nightly regression pin), with no edit to that test's assertions.
- A new unit test on `find_hlc_for_path` fails on the old predicate and passes on the new one: a file introduced by the second parent of a merge resolves to the ORIGINAL commit's HLC, not the merge's.
- A merge commit that genuinely rewrites a path (blob differs from every parent — a conflict resolution) still resolves to the merge's HLC (existing behavior preserved, pinned).
- `cargo test-ci` and `cargo test -p ddb-core --test sync_test` are green locally; the next Nightly's three failing jobs go green (verified by the maintainer after the push, outside the loop).

## Functional Decomposition

### Capability: Merge-aware path attribution
#### Feature: Touch predicate ignores files a merge merely transported
- **Description**: a commit "touches" a path only when its blob for that path differs from ALL of its parents' blobs; a merge whose blob equals any parent's blob did not author the change and the walk continues past it.
- **Inputs**: a commit (`git2::Commit`) with 0..n parents and the target `path`.
- **Outputs**: `Some(Some(hlc))` / `Some(None)` / `None` exactly as `check_commit_for_hlc` returns today (`read.rs:100-103`), with the merge case corrected.
- **Behavior**: for `parent_count() <= 1` keep the existing `diff_tree_to_tree(parent(0), tree)` rule (root commits: the path exists in the tree). For `parent_count() > 1`: look up the path's blob OID in the commit tree and in every parent tree; touched ⇔ the commit's entry (present or absent) differs from EVERY parent's entry. Equal to at least one parent ⇒ not touched ⇒ return `None` so the topological walk reaches the authoring commit on that parent's side.

#### Feature: Regression pins
- **Description**: tests that fail on the old predicate and pass on the new one, at both the helper and the sync level.
- **Inputs**: a temp repo with a two-parent merge (one file authored on each side, HLC trailers on both authoring commits and on the merge, as `c33b0a7` stamps them); the existing `two_node_singleton_post_sync_resolution`.
- **Outputs**: `git_ops/tests.rs` unit tests + the unchanged sync test green.
- **Behavior**: (a) merged-in file resolves to the side commit's HLC; (b) a merge that rewrites a path (differs from both parents) resolves to the merge's HLC; (c) the sync test's assertions at `:302-329` pass without modification.

### Capability: Verification and record
#### Feature: Bisect confirmation and changelog
- **Description**: the regression range is confirmed, not assumed, and the fix is recorded.
- **Inputs**: `c33b0a7^` and `c33b0a7`; the failing test.
- **Outputs**: a one-line bisect record in the PRD workdir notes; a `CHANGELOG.md` `Fixed` entry; a sentence in `docs/src/technical/crdt-resolver.md` on how per-path HLC attribution treats merges.
- **Behavior**: run the sync test at `c33b0a7^` (expected pass) and `c33b0a7` (expected fail) via `git worktree` under `dev/local/worktrees/` (link-ok: created on demand, gitignored); if the range differs, record the actual first-bad commit and proceed with the same fix only if the mechanism still matches the premise.

## Structural Decomposition

### Repository Structure

```text
ddb-core/src/git_ops/
├── read.rs                # Maps to: Touch predicate (check_commit_for_hlc :104-146; find_hlc_for_path :81 unchanged)
└── tests.rs               # Maps to: Regression pins (find_hlc_for_path_* tests, :585-663 today)
ddb-core/tests/
└── sync_test.rs           # Maps to: Regression pins (two_node_singleton_post_sync_resolution :219, unchanged)
docs/src/technical/
└── crdt-resolver.md       # Maps to: Bisect confirmation and changelog (merge attribution rule)
CHANGELOG.md               # Maps to: Bisect confirmation and changelog (Fixed entry)
```

### Module: git_ops read (ddb-core/src/git_ops/read.rs)
- **Maps to capability**: Merge-aware path attribution
- **Responsibility**: decide, per commit, whether it authored a change to `path`; merges that only transport a file are transparent to the HLC walk.
- **Exports**: `find_hlc_for_path(&self, commit, path) -> Option<Hlc>` (signature unchanged); `check_commit_for_hlc` (private, corrected).

### Module: git_ops tests (ddb-core/src/git_ops/tests.rs)
- **Maps to capability**: Regression pins
- **Responsibility**: pin the merge-transport and merge-rewrite cases at the helper level.
- **Exports**: test fns only.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **bisect confirmation**: proves the premise before the predicate changes.

### Core Layer (Phase 1)
- **touch predicate + helper tests**: Depends on [bisect confirmation] (premise held).

### Integration Layer (Phase 2)
- **sync-level verification + docs + changelog**: Depends on [touch predicate].

## Implementation Phases

### Phase 0: Foundation
**Goal**: the regression range and mechanism are confirmed.

**Tasks**:
- [ ] Bisect: in a `git worktree` under `dev/local/worktrees/` (link-ok: created on demand, gitignored), run `cargo test -p ddb-core --test sync_test two_node_singleton_post_sync_resolution` at `c33b0a7^` and at `c33b0a7`; re-check the premise (`check_commit_for_hlc` diffs against `parent(0)` only; merge commits carry an HLC trailer) at HEAD; remove the worktree afterwards (no deps) - Acceptance: the workdir note records pass at `c33b0a7^` and fail at `c33b0a7` (or the actual first-bad commit if it differs) plus the premise re-check result; if the premise fails, the note says so and Phase 1 is skipped and reported, never forced.

**Exit Criteria**: regression range and premise recorded.

### Phase 1: Core
**Goal**: merges that only transport a file no longer own its HLC.

**Tasks**:
- [ ] Add a failing unit test in `ddb-core/src/git_ops/tests.rs`: build a two-parent merge where side B authored `ddb/x.md` with HLC 1000 (raw commit, one trailer — the file's existing raw-commit pattern at `:594-611`), merge it into A with a merge commit carrying HLC 5000; assert `find_hlc_for_path(merge, "ddb/x.md")` == HLC 1000; add a second test where the merge commit rewrites `ddb/y.md` to content differing from both parents and assert its HLC (5000) is returned (depends on: Phase 0) - Acceptance: the first test fails against the current predicate (run once before the fix and record the failure), the second passes before and after.
- [ ] Change `check_commit_for_hlc` (`read.rs:104-146`): for `c.parent_count() > 1`, compare the path's tree entry OID in the commit against each parent's entry; touched only when it differs from every parent (present/absent counts as a difference); single-parent and root commits keep the current diff rule (depends on: the failing test) - Acceptance: both new unit tests pass; the existing `find_hlc_for_path_*` tests (`tests.rs:585-663`) stay green unchanged; `cargo clippy --workspace --all-targets` clean.

**Exit Criteria**: helper-level pins green; the predicate is merge-aware.

### Phase 2: Integration
**Goal**: the Nightly pin passes; the rule is recorded.

**Tasks**:
- [ ] Run `cargo test -p ddb-core --test sync_test` (the isolated Tier 2 command that was red) and `cargo test-ci` (depends on: Phase 1) - Acceptance: `two_node_singleton_post_sync_resolution` passes with its assertions untouched (`rg -c "assert_eq!(resolution.winner, \"20260601000010\")" ddb-core/tests/sync_test.rs` == 1); the whole sync_test binary is green; Tier 1 green.
- [ ] Add a `CHANGELOG.md` `Fixed` entry ("singleton and last-writer-wins resolution rank by the authoring commit's HLC, not the merge that transported the file") and a paragraph in `docs/src/technical/crdt-resolver.md` stating the merge attribution rule; push `master` (depends on: Phase 1) - Acceptance: CHANGELOG entry present under `[Unreleased]`; docs sentence present; `master` pushed so the next Nightly exercises the fix.

**Exit Criteria**: sync_test green locally; CHANGELOG + docs landed; pushed.

## Test Strategy

### Critical Scenarios
- **Happy path**: two devices each create a singleton row, B's later (HLC 2000 vs 1000); B syncs → Expected: B's row wins, A's is quarantined under `ddb/_conflicts/` (the existing sync test).
- **Edge case**: a merge commit that itself rewrites a path (conflict resolution) → Expected: that merge's HLC is the path's HLC (unchanged behavior, pinned).
- **Error case**: a merge commit without an HLC trailer that transported a file → Expected: the walk continues to the authoring commit instead of stopping at the trailer-less merge (today it stops and returns `None`; the new predicate never stops at a transport-only merge).

## Risks

- **Bisect disagrees with the hypothesis**: the first-bad commit may differ (the 00164-00166 range also changed minting and actor ids). Mitigation: Phase 0 records the real range; the fix proceeds only if the premise (parent(0)-only diff on a trailer-stamped merge) still explains the failure, else the PRD stops and reports.
- **Revwalk cost**: merge commits now look up the path in every parent tree instead of one diff. Mitigation: `tree.get_path` per parent is O(depth) and only runs on merge commits; the 1000-commit depth cap (`read.rs:82`) is unchanged.
- **Behavior change on genuinely resolved merges**: none intended — a merge whose blob differs from every parent is still "touched". Mitigation: the second unit test pins it.
- **Sequencing outside the numeric order**: this PRD must run before 00171's task 13 or the shell-file deletion waits on a red Nightly. Mitigation: the operator runs it explicitly while the batch is paused (header note); it depends on nothing.

## Transport Impact

Core-only correctness fix inside `git_ops`; no public surface changes on CLI, GraphQL, REST, PgWire, FFI, or NoSQL HTTP. Every transport that triggers a sync (CLI `ddb sync`, GraphQL/REST sync mutations, future FFI sync via 00191) inherits the corrected winner selection. Cross-interface conformance plan: the existing `sync_test` integration binary is the gate; no new interface surface exists to conform.

## Execution record (attended, 2026-09-05)

- **Phase 0 bisect**: `two_node_singleton_post_sync_resolution` passes at `c33b0a7^` (`83aefcd`) and fails at `c33b0a7` — first bad commit confirmed (note: the second worktree needed a forced rebuild; cargo's shared target dir had judged its older-mtime sources fresh). Premise re-checked at HEAD `5154234`: `check_commit_for_hlc` diffed against `parent(0)` only; merge commits stamped.
- **Phase 1**: three helper tests added (`find_hlc_for_path_ignores_merge_that_only_transports_a_file`, `..._keeps_merge_hlc_when_merge_rewrites_the_file`, `..._walks_past_a_trailerless_transport_merge`); the first and third failed against the old predicate (5000 vs 1000; `None` vs `Some(1000)`), committed as `f4926ac`. Predicate fix + CHANGELOG `Fixed` + `crdt-resolver.md` paragraph committed as `b1a047f`. A fourth caller exists beyond the three named: `git_ops/merge.rs:527-528` (binary-ref LWW) — inherits the fix.
- **Phase 2 gates (first pass)**: helper tests 7/7; `cargo test -p ddb-core --test sync_test` 6/6 (the pin passes with its assertions untouched); `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test-ci` 42 + 1552 + 222 passed, 0 failed; `cargo test -p ddb-e2e` 559 passed, 0 failed; pushed to `origin/master` at `b1a047f`.
- **B01 follow-up (second backlog review pass, 2026-09-05)**: the one-predicate fix still walked the whole DAG, so after a transparent merge the discarded side's later rewrite could answer first (ours-kept merge over a descendant's rewrite → 2000 instead of 1000; reproduced by `find_hlc_for_path_follows_the_retained_parent_not_the_discarded_one`, failing-first commit `52759bd`). `find_hlc_for_path` is now a breadth-first walk that follows only the parent(s) whose entry for the path equals the current commit's (parent order, deduplicated, same 1000-commit cap); `check_commit_for_hlc` and the interim `merge_authored_path` were removed as orphans. Fix commit `a7e5179` (read.rs + CHANGELOG + docs). Second-pass gates: helper tests 10/10, `sync_test` 6/6, clippy clean; `cargo test-ci` and e2e re-run recorded in the final line below. The PRD's "keeps `find_hlc_for_path` unchanged" framing and its `design: skip` premise were wrong on this point; executed attended, so no design phase ran — the traversal rule is documented in `crdt-resolver.md` instead. The acceptance line's `rg -c` should have been `rg -F -c` (parentheses are regex syntax); the check was done by reading the assertion instead.
- **Final gates (second pass, at `a7e5179`)**: `cargo test-ci` 42 + 1555 + 222 passed, 0 failed; `cargo test -p ddb-e2e` from a clean worktree of `a7e5179` 559 passed, 0 failed. The dirty main tree had shown 563/564 with `sync_crdt::frontmatter_conflict_creates_fm_crdt_file` failing; cause is the concurrent 00171 task-12 WIP (uncommitted), which inserts a `ddb read` before the `.crdt/temp` assertion — `GitRepo::open` runs `cleanup_orphaned_crdt_temp` and purges the `_fm.crdt` file the test then looks for. Not this PRD. Pushed to `origin/master` at `a7e5179`. Harness note: the e2e binary resolves as `<manifest>/../target/debug/ddb` (`tests/e2e/common.rs:15-32`), so a worktree run needs its own `target` (a symlink to the shared one works) even when `--target-dir` is passed.
- **CI follow-up**: the Test workflow on `b1a047f` (run 33957131105) went red at its clippy step — `needless_borrows_for_generic_args` on `.and_then(&entry_id)` under the toolchain `rust-toolchain.toml` pins (1.89.0). Local Tier 1 had run clippy 1.97.1 because the shell exports `RUSTUP_TOOLCHAIN=1.97.1`, which overrides the pin; 1.97.1 does not fire that lint. Reproduced with `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings` at `a7e5179` (same construct, `read.rs:125`), fixed by passing the `Copy` closure by value, clean under both 1.89.0 and 1.97.1, helper tests 10/10; commit `75cde1c`, pushed. The Test run on `a7e5179` (33965243110) is expected to fail the same way. Test run 33965868002 on `75cde1c`: success — `test` job 25 min (clippy through the e2e legs) and `coverage` job 21 min, both green (checked 2026-09-05 12:48 UTC). Nightly (`full-validation.yml`) still needs its next scheduled run at or after `75cde1c`; check headSha, duration, and jobs, not just the conclusion.
