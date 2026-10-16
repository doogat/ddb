# Historical PRD00203 before approved FT-7 scope — 2026-09-27

Preserved verbatim below for provenance; not the current implementation spec.
The approved backlog PRD and FT-7 cards supersede its guessed identity and
CLI-only abort requirements. Nothing below is evidence of implementation.

---
default_model: opus
model_tier_rationale: destructive blast radius (ref deletion decides data reachability) plus an invented ref-namespace contract and a new recovery surface across fetch, merge target, and cleanup.
design: run
---

# Per-import bundle ref namespace

**Priority:** P0 (live silent data loss on master) · **Roadmap:** pre-flight (assessment 2026-09-06) · **Depends on:** none

**Preparation note, 2026-09-26:** reviewed against master `a739c29`; no runtime
work started. Draft cards are `dev/local/tmp/FT-7a-card.md` and
`dev/local/tmp/FT-7b-card.md`; rationale and the pending recovery-scope decision
are in `dev/local/designs/00203-bundle-import-ref-namespace-preflight.md`.
The old phase split below is not independently shippable: FT-7a must keep
fetch, delivered-target selection and cleanup together. The cards propose a
full Git-payload digest and a separate import lease instead of the guessed
12-character identity and unsupported no-contention claim. They assume abort
is deferred, pending the maintainer's answer; the original abort requirements
below are deliberately retained and must not be silently checked off. Reconcile
scope before build/closure; this PRD remains in backlog.

## Overview

### Problem Statement

Importing a bundle can import a **different, older** bundle's data and report success.

Provenance: `dev/local/audit-results/refactor-assessment-2026-09-06.md:67` ("00168 CRITICAL is live on master") and the deferred ledger `dev/local/autopilot/deferred/202607161128-deferred.json:117` (stall, `cap_critical`), which records verbatim: "import_bundle merges the hardcoded refs/remotes/bundle/master rather than the refs this bundle fetched, so with a stale bundle ref present (the state the cycle-1 CRITICAL fix now creates) importing a different bundle reports success while importing the OLD bundle's data and leaving the requested commits unreachable. Reproduced by Blake and independently confirmed by the orchestrator (/tmp/ddb-ffprobe-a, git 2.55.0). Same root cause as 2 of the 3 HIGHs; adding + to the fetch refspec is NOT the fix (it reintroduces the cycle-1 CRITICAL)."

The mechanism, confirmed at HEAD:

- Every import fetches into one shared namespace: `refs/heads/*:refs/remotes/bundle/*` with `--no-prune` (`ddb-core/src/bundle/mod.rs:92-106`). `--no-prune` is load-bearing (it fixed 00168's cycle-1 CRITICAL: a global `fetch.prune=true` deleted the ref a failed import kept), so stale refs now persist by design.
- The merge target is hardcoded, not the ref this import delivered: `merge_remote_allowing_unrelated("bundle", "master")` (`ddb-core/src/bundle/mod.rs:155`). A bundle carrying no `master` therefore merges the **previous** bundle's stale `refs/remotes/bundle/master` and reports success.
- Cleanup is namespace-wide: `delete_all_bundle_refs` walks all of `refs/remotes/bundle/` (`ddb-core/src/bundle/mod.rs:115-134`) and runs after a success (`:227`), so one import's success orphans the data another failed import deliberately kept.
- Forcing the refspec is excluded as a fix (`dev/local/reviews/00168-bundle-import-conflict-recovery-v1-review-02.md:312-323`); the fix must make the merge target and the cleanup scope follow *this* import's refs.

This reopens held `dev/local/prds/hold/00168-bundle-import-conflict-recovery-v1.md` for that one CRITICAL only. The three-write-lease HIGH (ledger `:85`) stays **out of scope** — collapsing the re-entrant acquisitions is a sync-wide change needing its own PRD.

### Target Users

Multi-device and air-gapped users (jink) who import bundles from more than one peer, and anyone recovering a repo from a `ddb compact` backup bundle.

### Success Metrics

- Importing bundle B never merges bundle A's refs: pinned by a colocated regression that fails against HEAD.
- A failed import keeps exactly its own namespace; a successful import deletes exactly its own namespace and no other.
- Behavior is identical with `fetch.prune=true` and `false` in the repo config, with no `+` in the refspec.
- A masterless bundle fails loudly (`Sync`, `bundle merge failed:` prefix) instead of merging someone else's `master`.
- Stale namespaces are removable without hand-running git: `ddb bundle import --abort`.

## Functional Decomposition

### Capability: Per-import ref isolation

#### Feature: Deterministic import id
- **Description**: each import derives a stable id from the bundle it is importing.
- **Inputs**: the extracted bundle directory.
- **Outputs**: `import_id`, the first 12 hex chars (guess) of the sha256 `verify_extracted_checksum` already computes (`ddb-core/src/bundle/mod.rs:395-410`).
- **Behavior**: `verify_extracted_checksum` returns the verified checksum instead of `()`; no new hashing pass, no clock, no randomness — the same bundle file always yields the same id.

#### Feature: Namespaced fetch
- **Description**: the fetch lands this bundle's heads under `refs/remotes/bundle/<import-id>/*`.
- **Inputs**: `import_id`, the extracted `objects.bundle`.
- **Outputs**: refs only under this import's namespace.
- **Behavior**: refspec `refs/heads/*:refs/remotes/bundle/<import-id>/*`, keeping `--no-prune` and **no** leading `+`. Two imports can never contend for one ref name, so pruning and forcing both stop mattering.

### Capability: Import-scoped merge and cleanup

#### Feature: Merge the ref this import delivered
- **Description**: the merge target is read from the namespace the fetch just wrote.
- **Inputs**: `refs/remotes/bundle/<import-id>/*` after the fetch.
- **Outputs**: `MergeResult` for this bundle's `master`, or a `Sync` error.
- **Behavior**: enumerate the namespace (`references_glob`), then call `merge_remote_allowing_unrelated("bundle", "<import-id>/master")` — `merge_remote_with_policy` builds `refs/remotes/{remote}/{branch}` by `format!` (`ddb-core/src/git_ops/merge.rs:448`), so a slashed branch needs no engine change. No delivered `master` → `Sync` naming the branches this bundle did deliver, keeping the `bundle merge failed:` prefix contract.

#### Feature: Own-namespace-only cleanup and recovery
- **Description**: success deletes only this import's namespace; failure keeps only this import's namespace; stale ones are removed on request.
- **Inputs**: `import_id`; for recovery, an operator command.
- **Outputs**: deleted refs; a count of namespaces cleared.
- **Behavior**: `delete_all_bundle_refs` is scoped to `refs/remotes/bundle/<import-id>/`, deleting through the write-locked `GitRepo::delete_remote_ref` (`ddb-core/src/git_ops/remote.rs:27-39`). Earlier failed namespaces are never merged implicitly and never deleted implicitly. `ddb bundle import --abort` clears one named namespace or all of them: `--import-id <id>` selects one, bare `--abort` clears all (guess).

## Structural Decomposition

### Repository Structure

```
ddb-core/src/
├── bundle/
│   ├── mod.rs           # Maps to: Deterministic import id, Namespaced fetch,
│   │                    #   Merge the ref this import delivered, Own-namespace-only cleanup
│   └── tests.rs         # Maps to: colocated regressions (CRITICAL repro, fetch.prune pin)
├── service/
│   └── concrete_index.rs   # Maps to: abort entry point beside DoogatService::import_bundle (:41-44)
ddb-cli/src/
├── main.rs              # Maps to: BundleAction::Import gains --abort (:553-571)
└── commands/sync.rs     # Maps to: import/abort dispatch (:100-107)
tests/e2e/
└── bundle_import_ref_namespace.rs   # Maps to: CLI-level namespace isolation + abort
```

### Module: bundle (ddb-core/src/bundle/mod.rs)
- **Maps to capability**: Per-import ref isolation; Import-scoped merge and cleanup
- **Responsibility**: derive the import id, fetch into its namespace, merge the ref that namespace delivered, delete only that namespace.
- **Exports**:
  - `import_bundle()` - unchanged signature, now namespace-scoped end to end
  - `abort_bundle_import()` - delete one named stale namespace, or all
  - `verify_extracted_checksum()` - now returns the verified sha256

### Module: service bundle verbs (ddb-core/src/service/concrete_index.rs)
- **Maps to capability**: Import-scoped merge and cleanup
- **Responsibility**: expose the abort recovery verb beside `DoogatService::import_bundle` (`:41-44`); the export verbs stay in `service/ops.rs:101-109`.
- **Exports**:
  - `abort_bundle_import()` - service-level wrapper returning the number of namespaces cleared

### Module: bundle CLI (ddb-cli/src/commands/sync.rs)
- **Maps to capability**: Import-scoped merge and cleanup
- **Responsibility**: dispatch `ddb bundle import --abort` and keep the existing import output line.
- **Exports**:
  - `bundle()` - existing dispatcher, extended with the abort arm

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **import id + namespaced fetch**: gives every later step a per-import ref scope.

### Core Layer (Phase 1)
- **merge target + cleanup scope**: Depends on [import id + namespaced fetch]

### Integration Layer (Phase 2)
- **abort surface, e2e, docs, walkthrough**: Depends on [merge target + cleanup scope, import id + namespaced fetch]

## Implementation Phases

### Phase 0: Foundation
**Goal**: every import owns a ref namespace derived from its own bundle.

**Tasks**:
- [ ] Return the verified sha256 from `verify_extracted_checksum` and derive `import_id` from its first 12 hex chars in `import_bundle` (no deps) - Acceptance: `cargo test -p ddb-core bundle::tests::import_id_is_stable_per_bundle_and_differs_across_bundles` passes.
- [ ] Fetch into `refs/heads/*:refs/remotes/bundle/<import-id>/*`, keeping `--no-prune` and no `+` (depends on: Phase 0 task 1) - Acceptance: `cargo test -p ddb-core bundle::tests::namespaced_fetch_is_unaffected_by_fetch_prune_true` passes; `rg -n 'refs/heads/\*:refs/remotes/bundle/\*' ddb-core/src/bundle/mod.rs` returns nothing (the shared refspec literal at `:97` is gone).

**Exit Criteria**: two imports in one repo write disjoint ref namespaces under any `fetch.prune` setting.

### Phase 1: Core
**Goal**: merge and delete only what this import delivered.

**Tasks**:
- [ ] Merge the `master` ref found in this import's namespace via `merge_remote_allowing_unrelated("bundle", "<import-id>/master")`; a masterless bundle returns `Sync` naming the delivered branches (depends on: Phase 0) - Acceptance: `cargo test -p ddb-core bundle::tests::masterless_bundle_errors_instead_of_merging_another_imports_master` passes.
- [ ] Scope `delete_all_bundle_refs` to `refs/remotes/bundle/<import-id>/` so success clears only its own namespace and failure keeps only its own (depends on: Phase 1 task 1) - Acceptance: `cargo test -p ddb-core bundle::tests::successful_import_deletes_only_its_own_namespace` passes.
- [ ] Add the CRITICAL regression: import A fails mid-merge (reuse `import_with_unresolvable_collision`, `ddb-core/src/bundle/tests.rs:220-291`), then import a different bundle B and assert B's commits are reachable at HEAD and A's doogat was never merged (depends on: Phase 1 task 2) - Acceptance: `cargo test -p ddb-core bundle::tests::stale_namespace_is_never_merged_by_a_later_import` fails against HEAD and passes after.

**Exit Criteria**: the ledger's reproduction no longer reports success, and no import touches another import's refs.

### Phase 2: Integration
**Goal**: operators can clear stale namespaces, and the contract is proven and documented.

**Tasks**:
- [ ] Add `abort_bundle_import` (core + `DoogatService`) and the `ddb bundle import --abort [--import-id <id>]` arm, printing the count cleared (depends on: Phase 1) - Acceptance: `cargo test -p ddb-core bundle::tests::abort_clears_named_namespace_and_leaves_others` passes and `cargo clippy --workspace --all-targets` is clean.
- [ ] Author `tests/e2e/bundle_import_ref_namespace.rs` (failed masterless import keeps its namespace, a later different import lands its own data, `--abort` clears the stale one) and register it in `tests/e2e/main.rs` (depends on: Phase 2 task 1) - Acceptance: `rg -n 'mod bundle_import_ref_namespace;' tests/e2e/main.rs` matches.
- [ ] Update `docs/src/technical/bundle.md` (import steps 4/5/8 at `:55-59` and the Conflict Recovery section at `:64-70`), the bundle clauses in `docs/src/technical/invariants.md:14` and `docs/src/technical/sync.md:56`, and add a CHANGELOG `Fixed` entry; correct the 00168 `Unreleased` lines at `CHANGELOG.md:48-49` where they claim namespace-wide delete or a `bundle/master` target. Premise: those two lines still describe the pre-00203 shared namespace — re-read at execution and skip with a report if already corrected (depends on: Phase 2 task 2) - Acceptance: `rg -n 'refs/remotes/bundle/<import-id>' docs/src/technical/bundle.md` matches and CHANGELOG has no remaining claim that import deletes the whole `refs/remotes/bundle/` namespace.
- [ ] Append to `dev/local/prds/hold/00168-bundle-import-conflict-recovery-v1.md`: "Superseded for the CRITICAL by 00203; write-lease HIGH still needs its own PRD". Premise: that hold file exists — re-check at execution and report a skip if it has moved (depends on: Phase 2 task 3) - Acceptance: `rg -n 'Superseded for the CRITICAL by 00203' dev/local/prds/hold/00168-bundle-import-conflict-recovery-v1.md` matches.
- [ ] Create the showboat walkthrough `dev/local/walkthroughs/00203-bundle-import-ref-namespace.md` (link-ok: created by this task) demonstrating failed import → different import → `--abort` (depends on: Phase 2 task 2) - Acceptance: the file exists and `dev/bin/safe-showboat-verify dev/local/walkthroughs/00203-bundle-import-ref-namespace.md` exits 0.

**Exit Criteria**: namespace isolation is proven at unit and CLI level, documented, and recoverable by command.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root.

### Critical Scenarios
- **Happy path**: clean bundle import → Expected: merge commit, only `refs/remotes/bundle/<import-id>/*` created and then deleted, `refs/remotes/bundle/` empty of that id.
- **Edge case**: repo config sets `fetch.prune=true` and a prior failed import's namespace exists → Expected: the new import fetches, merges and cleans its own namespace; the kept one is untouched (extends `kept_bundle_ref_survives_a_later_import_under_fetch_prune`, `ddb-core/src/bundle/tests.rs:466-509`).
- **Error case**: import A fails mid-merge, then bundle B (no `master`) is imported → Expected: a `Sync` error with the `bundle merge failed:` prefix naming B's delivered branches; A's data is NOT merged and stays reachable (today: success reporting A's data).
- **Recovery**: `ddb bundle import --abort` with two stale namespaces → Expected: both deleted, count reported, exit 0; a repeat run is a no-op.
- **Regression**: `bundle_import_recovers_real_conflict_and_reimport_is_noop` (`tests/e2e/bundle_import_conflict_recovery.rs`), `full_bundle_import_on_new_repo`, `successful_import_deletes_every_bundle_ref_not_just_master`, and `bundle_import_from_main_branch_repo_reports_sync_not_raw_not_found` stay green (the last renamed only if its namespace assertion changes).

## Risks

- **Ref-name length and validity**: `refs/remotes/bundle/<sha-prefix>/master` adds a path segment. Mitigation: hex-only ids, fixed 12 chars, exercised by every namespaced test.
- **Deleting the wrong namespace**: cleanup is the data-loss surface (I1, whose heading covers ref-delete paths: `docs/src/technical/invariants.md:9`). Mitigation: cleanup takes `import_id` as an argument, never a glob over `refs/remotes/bundle/`; a test asserts a sibling namespace survives.
- **Stale-namespace accumulation**: failed imports now leave refs pinning objects indefinitely. Mitigation: `--abort` clears them, and the import error message names the id to abort.
- **Out-of-scope drift**: the three-lease HIGH and the unlocked `unbundle_git_objects` leg remain open. Mitigation: stated here as out of scope; unchanged code, unchanged docs claims.

## Transport Impact

- **CLI**: Guaranteed — `ddb bundle import` is the fix site; `--abort` is added here.
- **GraphQL**: no change — bundle import has no GraphQL surface (`ddb-cli` and FFI only); not regressed.
- **REST**: no change — no bundle endpoint.
- **PgWire**: no change — SQL surface only.
- **FFI**: Guaranteed for behavior — `DoogatDriver::import_bundle` (`ddb-core/src/ffi/driver.rs:271`) inherits namespace isolation with no signature change; the pre-existing discarded `SyncReport` (ledger `:94`) stays out of scope, and no abort binding is added in this PRD.
- **NoSQL HTTP**: no change — document mirror, no bundle surface.
- **Cross-interface conformance plan**: the colocated `ddb-core/src/bundle/tests.rs` regressions pin the core contract shared by CLI and FFI (both enter through `DoogatService`); `tests/e2e/bundle_import_ref_namespace.rs` pins the CLI leg end to end; the existing `tests/e2e/bundle_import_conflict_recovery.rs` and `tests/e2e/integration_bundle.rs` pin no-regression on the conflict and delta paths. Public shape change: a masterless bundle now errors instead of silently merging another bundle's `master` — recorded in CHANGELOG `Fixed`.
