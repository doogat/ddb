# Bundle-import conflict recovery

**Priority:** P1 · **Roadmap:** Phase 1 (convergence) · **Depends on:** 00162 (write lock — merge + resolve + ref-delete run inside the write critical section)

## Overview

### Problem Statement
A conflicted bundle import reports success while silently losing the imported data.

- **Conflict detection reads `stderr`, not state.** `merge_bundle_and_resolve` (`ddb-core/src/bundle.rs:113-138`) shells out to `git merge refs/remotes/bundle/master` (`:118-126`) and, on failure, decides conflict-vs-error by **string-matching `stderr`** for `"CONFLICT"` / `"Automatic merge failed"` (`:132-137`). This is locale- and version-fragile, and there is **no** `git merge --abort` anywhere.
- **A conflicted merge is reported as clean success.** A conflicted `git merge` creates **no commit**, so `resolve_post_merge_conflicts` (`sync_manager/mod.rs:883-887`) → `validate_clean_merge_or_fallback` sees `commit_parent_count < 2` (`:544`) and returns `Ok(0)`: the import reports **success with 0 conflicts** while nothing merged.
- **The bundle ref is deleted regardless, pruning the data.** `import_bundle` (`bundle.rs:160-197`) then **unconditionally** deletes `refs/remotes/bundle/master` (`:181-184`), making the unbundled commits unreachable, so auto-gc prunes the offline-synced data.

### Target Users
Multi-device users (jink) syncing offline via bundles; a conflicted import must recover the data through the resolver, not silently discard it.

### Success Metrics
- A conflicted bundle import produces a **real merge commit** via the resolver, reports the **true** conflict count, leaves the bundle data reachable, and never reports false success — pinned by a conflicted-bundle test.
- Conflicts are detected from the **git index / merge state**, never from `stderr` strings — asserted by a test that a locale/wording change in git output does not flip the detection.
- On an unresolvable bundle merge, `git merge --abort` runs and a real `Sync` error surfaces; the repo is left clean (no dangling `MERGE_HEAD`, no partial index) — pinned by a test.
- `refs/remotes/bundle/master` is deleted **only after** a successful merge commit, so unreachable objects never exist for gc to prune — pinned by a reachability test.
- No regression: existing clean-import bundle tests stay green.

## Functional Decomposition

### Capability: Bundle-import conflict recovery

#### Feature: Index-state conflict detection + resolution
- **Description**: bundle-merge conflicts are detected from the git index/merge state and driven through the existing resolver.
- **Inputs**: the unmerged index left by the bundle merge.
- **Outputs**: resolved files committed via `commit_merge`; the true conflict count.
- **Behavior**: read conflicts from the index (unmerged entries / `MERGE_HEAD` presence), not `stderr` strings; run the existing conflict pipeline + `commit_merge`; the returned count reflects real conflicts.

#### Feature: Ref-delete-after-success + abort-on-error
- **Description**: the bundle ref is deleted only after a successful merge commit; any failure aborts and errors.
- **Inputs**: the merge/commit result.
- **Outputs**: bundle data reachable until success; clean repo on error.
- **Behavior**: delete `refs/remotes/bundle/master` only after a real merge commit; on any error run `git merge --abort` (when `MERGE_HEAD` exists, else reset the index) and return a `Sync` error. The merge → resolve → ref-delete sequence runs inside the 00162 write lease.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/
├── bundle.rs          # Maps to: merge_bundle_and_resolve (:113-138) index-state detection + --abort; import_bundle (:160-197) delete ref only after success
ddb-core/src/sync_manager/
└── mod.rs             # Maps to: resolve_post_merge_conflicts (:883-887) / validate_clean_merge_or_fallback (:538-544) stop reporting false success
```

### Module: bundle (ddb-core/src/bundle.rs)
- **Maps to capability**: Bundle-import conflict recovery
- **Responsibility**: detect conflicts from index/merge state, drive the resolver, delete the bundle ref only after a successful merge, abort + error on failure.
- **Exports**: `merge_bundle_and_resolve`, `import_bundle`

### Module: sync_manager (ddb-core/src/sync_manager/mod.rs)
- **Maps to capability**: Bundle-import conflict recovery
- **Responsibility**: refuse to report success when no merge commit exists; report the honest count from real merge state.
- **Exports**: `resolve_post_merge_conflicts` - honest count from real merge state

## Dependency Graph

### Foundation Layer (Phase 0)
Depends on 00162 (write lease) - the merge/resolve/ref-delete critical section acquires it.

- **index-state detection**: read conflicts from the index, not `stderr`; independent of the reassignment work.

### Core Layer (Phase 1)
- **honest recovery + ref-after-success + abort**: Depends on [index-state detection] - drive the resolver, commit, delete the ref only on success, abort on error.

### Integration Layer (Phase 2)
- **conflicted-bundle test + docs + walkthrough**: Depends on [honest recovery].

## Implementation Phases

### Phase 0: Foundation
**Goal**: detect conflicts from state, not strings.

**Tasks**:
- [ ] Replace the `stderr` string-match in `merge_bundle_and_resolve` with index/merge-state detection (unmerged entries / `MERGE_HEAD`) (depends on: 00162) - Acceptance: a conflicted bundle merge is detected from the index; a unit test asserts detection is unaffected by git output wording/locale.

**Exit Criteria**: conflict detection is state-based.

### Phase 1: Core
**Goal**: honest recovery, ref-after-success, abort-on-error.

**Tasks**:
- [ ] Drive the detected conflicts through the existing conflict pipeline + `commit_merge`; report the true count; delete `refs/remotes/bundle/master` only after a successful merge commit; on any error run `git merge --abort` and return a `Sync` error; run the sequence inside the 00162 write lease (depends on: Phase 0) - Acceptance: a conflicted import produces a real merge commit, reports the true count, keeps bundle data reachable, and never reports false success; on an unresolvable merge the repo is left clean and an error surfaces.

**Exit Criteria**: bundle import recovers or aborts honestly; no false success.

### Phase 2: Integration
**Goal**: prove no data loss and document it.

**Tasks**:
- [ ] Conflicted-bundle test (true count, no prune, no false success; abort-on-error leaves a clean repo) + a reachability test (bundle data reachable until success); add a bundle-import scenario to `integration.sh`/`.ps1`; author the import-bundle conflict-recovery e2e under `tests/e2e/` (depends on: Phase 1) - Acceptance: all fail against the old code and pass after.
- [ ] Update `docs/src/technical/sync.md` (bundle-import recovery) + CHANGELOG; add/refresh a showboat walkthrough for bundle import (user-facing behavior change) (depends on: Phase 1) - Acceptance: docs + walkthrough landed.

**Exit Criteria**: conflicted-bundle coverage + docs + walkthrough landed.

## Test Strategy

### Critical Scenarios
- **Happy path**: bundle import with no conflicts → Expected: merge commit, bundle ref deleted after success, data reachable, count 0.
- **Edge case**: git emits differently-worded merge output → Expected: state-based detection still fires; the import does not misclassify the conflict as an error or as false success.
- **Error case**: conflicted bundle import → Expected: real merge commit via the resolver, true conflict count, data not pruned; on an unresolvable merge, `git merge --abort` + surfaced error, repo clean.

## Risks

- **CLI git vs libgit2 index**: reading conflicts after a CLI `git merge` mixes toolchains. Mitigation: read conflicts from the shared `.git/index` via libgit2, or route the bundle merge through the existing libgit2 conflict path; detect from state, never `stderr`.
- **`git merge --abort` preconditions**: abort requires a merge in progress. Mitigation: abort only when `MERGE_HEAD` exists, else reset the index; test the abort path.
- **Auto-gc timing**: pruning happens on gc, not immediately. Mitigation: keep the bundle ref until success so unreachable objects never exist; test reachability post-import.
- **False-success regression is silent**: the old bug reported `Ok(0)`. Mitigation: the conflicted-bundle test asserts a non-zero true count and a real 2-parent merge commit, so a regression fails loudly.

## Transport Impact

- **CLI**: Guaranteed — the bundle-import path is the primary fix site.
- **GraphQL**: Intentionally absent — bundle import is a CLI/FFI workflow, not a GraphQL surface; noted, not regressed.
- **REST**: Intentionally absent — bundle import is not a REST surface.
- **PgWire**: Intentionally absent — no bundle surface.
- **FFI**: Guaranteed — driver bundle import gains conflict recovery.
- **NoSQL HTTP**: Intentionally absent — read-only document mirror; no bundle surface.
- **Cross-interface conformance plan**: `integration.sh`/`.ps1` bundle scenarios exercise CLI + FFI; a conflicted-bundle test pins true count + no prune + no false success + clean abort; a reachability test pins ref-after-success; `tests/e2e/` covers bundle-import conflict recovery. The one public change: a conflicted bundle import now returns a real error instead of false success (CHANGELOG note).

## Closure evidence (2026-09-29, PRD 00203 / FT-7)

This PRD stays on hold; it is NOT fully done. Prior failed evidence (review-02, deferred ledger 202607161128) is retained.

- **CLOSED: the cycle-2 CRITICAL.** It covered stale-target merge and foreign-ref cleanup. `import_bundle` merged the hardcoded `refs/remotes/bundle/master`, so a masterless bundle merged another failed import's stale master, reported success, and swept every bundle ref.
  - Fixed by commit `3b27002` (FT-7a runtime).
  - Proven by commit `ab14498` (FT-7b CLI/FFI conformance).
  - Core tests: `ddb-core/src/bundle/tests.rs::failed_bundle_then_masterless_bundle_never_merges_stale_master`, `successful_bundle_import_preserves_other_failed_and_legacy_refs`, `failed_bundle_retry_uses_same_namespace_and_releases_import_lease`.
  - CLI tests: `tests/e2e/integration_bundle_import_ref_namespace.rs::bundle_import_{masterless_bundle_never_consumes_failed_bundle_refs,success_preserves_sibling_and_legacy_refs,retry_cleans_only_its_payload_namespace}`.
  - FFI tests: `ddb-core/src/ffi/tests.rs::bundle_import_ffi_*`, the same three.
  - Fail-first replay on pre-FT-7 `a739c29`: 6/6 red at prune=true, then 6/6 red at prune=false, on masterless-B false success.
  - Evidence: `dev/local/reviews/FT-7a-fast-track-review.md` and `FT-7b-fast-track-review.md`.
  - CI on exact HEAD `ab14498`: Test 36590016013 and Docs 36590016016 both succeeded.
- **STILL OPEN: the three-write-lease HIGH.** The merge, resolve and ref-delete are not one write-lease critical section. The new `ddb-bundle-import.lock` serializes only cooperating importers and does not close it. The unlocked preparation leg and the general rebuild races with other writers also stay open.
