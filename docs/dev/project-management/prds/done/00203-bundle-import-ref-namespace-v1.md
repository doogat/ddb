---
default_model: opus
model_tier_rationale: destructive ref deletion and import concurrency require careful reachability proofs.
design: run
---

# Per-import bundle ref namespace

**Priority:** P0 · **Roadmap:** pre-flight · **Depends on:** none
**Scope approved:** 2026-09-27, maintainer's "ok to proceed" after the recommendation to defer the new abort API.

## Overview

Importing a masterless bundle can merge another failed bundle's stale master,
report success, then delete refs that the previous failure kept reachable.
The current path fetches into shared refs/remotes/bundle/*, merges
bundle/master and cleans the entire parent namespace.

Provenance: dev/local/audit-results/refactor-assessment-2026-09-06.md and
dev/local/autopilot/deferred/202607161128-deferred.json (00168 CRITICAL).
Current source: ddb-core/src/bundle/mod.rs, unbundle_git_objects,
merge_bundle_and_resolve, delete_all_bundle_refs and import_bundle.
Forcing fetch is not a fix: it overwrites failed imports' retained refs.

The target users are existing air-gapped/bundle users through CLI and FFI.
The bounded contract is: import only the current Git payload's advertised
master; retain other imports' refs; clean the current namespace only after
successful post-merge work. A masterless bundle fails loudly rather than
implicitly selecting stale master, main or another branch.

## Design and compatibility

Use refs/remotes/bundle/<payload-id>/*, where payload-id is the full 64-character
lowercase SHA-256 of objects.bundle bytes, streamed after existing v1 checksum
verification. Same payload retries intentionally share an id. Metadata-only
tar changes do not skip node registration. Keep archive format/export/verify
compatibility; the existing unframed archive checksum is not a payload identity.

Take a separate repo-scoped cross-process import lease before namespace
inspection/unbundle/fetch and hold it through final cleanup/error. Reuse
git_ops::write_lock::acquire with ddb-bundle-import.lock in Git's common
metadata directory and a 30-second acquisition timeout. The file persists.
Resolve/check relative common-directory paths. No new public trait/dependency.

This outer import lease differs from the existing Git write lease. Do not
hold the Git write lease around subprocesses, SQLite work or index.rebuild.
Preserve existing Git/index ordering. The new lease serializes cooperating
importers only; it does not close 00168's separate three-write-lease HIGH,
unlocked preparation, or general rebuild races with other writers.

Read delivered refs/heads and OIDs from this extracted Git bundle. Reject a
missing payload before repo mutation. Reject unexpected names/OIDs already
in the candidate namespace; fetch with --no-prune and no forced refspec, then
verify fetched OIDs. Merge advertised master through the existing
merge_remote_allowing_unrelated("bundle", "<payload-id>/master") path.

Masterless/merge errors retain Sync + "bundle merge failed:" and identify the
payload and delivered branches. The new pre-merge lease timeout uses existing
Conflict with an import-specific message. FFI mapping stays DdbError::Git.
There are no signature, result-shape or new public capability changes.

After merge/apply, node registration and final index rebuild succeed, delete
only the exact trailing-slash namespace through delete_remote_ref. Earlier
failure retains fetched refs; cleanup errors may leave partial cleanup and
must be loud. Do not promise universal HEAD rollback or atomic multi-ref
deletion. Legacy flat refs, sibling namespaces and unrelated remotes survive.
Old binaries/manual Git commands bypass the new import lease.

## Repository structure

- ddb-core/src/bundle/mod.rs: complete core path.
- ddb-core/src/bundle/tests.rs: colocated semantic, custody and lease tests.
- tests/e2e/integration_bundle_import_ref_namespace.rs and main.rs: CLI workflow.
- ddb-core/src/ffi/tests.rs: same workflow through public DoogatDriver.
- docs/src/technical/{bundle,invariants,sync,walkthrough,ffi}.md: precise contract.
- CHANGELOG.md: Fixed entry and corrected still-unreleased shared-ref claims.
- Local walkthrough/evidence and held 00168 note: closure evidence only.

Exact per-item allowlists, test names and constraints:
dev/local/tmp/FT-7a-card.md and dev/local/tmp/FT-7b-card.md.
Source-grounded decisions: dev/local/designs/00203-bundle-import-ref-namespace-preflight.md.
Historical requirements: dev/local/designs/00203-pre-FT-7-scope.md.

## Implementation phases

### FT-7a — complete core correction

Fetch, target selection and cleanup must land together, not in broken
intermediate commits.

- [x] Author baseline-compatible stale-A/masterless-B regression; make stale A
  mergeable first so the old path demonstrably reports false success.
- [x] Implement full payload identity, import lease, advertised-head/OID checks,
  namespaced fetch, current-master merge and own-namespace cleanup.
- [x] Prove exact ref/OID custody under fetch.prune true/false, missing payload,
  unexpected candidate refs, success, failure, retry, concurrent processes and
  final rebuild failure. Retain existing conflict/delta/error tests.
- [x] Update core behavior/locking docs and changelog; run every card gate.
- [x] END build session; complete full independent review, verified findings,
  at most one rework and per-finding closure before committing.
  (FT-7a: commit 3b27002, dev/local/reviews/FT-7a-fast-track-review.md.)

### FT-7b — public workflow and closure

Depends on reviewed/committed FT-7a.

- [x] Author CLI and public FFI golden workflow: failed A -> locally repaired
  fixture -> masterless B refusal -> successful C preserving A/B -> retry A ->
  repeated success. Assert results, Git HEAD/tree, readable data and exact refs.
- [x] Record fail-first public regression replay against pre-FT-7 runtime.
- [x] Create showboat walkthrough through its CLI; verify only through
  dev/bin/safe-showboat-verify using temporary repositories.
- [x] Finish bundle/FFI docs and all card gates; END build session for fresh
  full review and closure before the second commit.
- [x] Run explicit batch gates, push reviewed master and verify CI for its exact
  HEAD. Record evidence; annotate held 00168's CRITICAL only, preserving its HIGH.
- [x] Move this PRD to done only after every retained deliverable is complete;
  retain Deferred work below. Stop after FT-7.
  (FT-7b: commit ab14498, dev/local/reviews/FT-7b-fast-track-review.md.)

## Test strategy

Driver owns sequential foreground mise-managed Rust 1.89.0 commands. Each
item runs build, all-target Clippy with warnings denied and test-ci, plus the
card's explicit bundle/sync/e2e gates. A Claude-routed replacement also requires
full e2e per CLAUDE.md; FT-7b has the plan's explicit batch-end full e2e gate.
No full-workspace heavy battery. Preserve fail-first and failure logs separately
from later passing runs. Never run ddb or unsafe showboat verify in the checkout.

Each item uses a fresh implementor and then a separate fresh-session full
review roster, not an autopilot loop or same-session review. All consensus,
blind and doubt/de-slop R1-R5 lenses, Victor verification and finding closure
remain mandatory. See the cards and fast-track plan for the complete roster.

## Transport impact

| Interface | Scope and evidence |
| --- | --- |
| CLI | Existing bundle import syntax/output; actual golden workflow through ddb. |
| FFI | Existing import_bundle signature/void result and DdbError mapping; same golden workflow through DoogatDriver. |
| GraphQL | No existing bundle-import endpoint; no new endpoint or parity claim. |
| REST | No existing bundle-import endpoint; no new endpoint or parity claim. |
| PgWire | No existing bundle-import endpoint; no new endpoint or parity claim. |
| NoSQL HTTP | No existing bundle-import endpoint; no new endpoint or parity claim. |

Shared core tests alone are not proof of CLI/FFI equivalence. Do not claim
Swift/Kotlin runtime coverage or SyncReport parity. This fixes an existing
capability; no new CLI-only recovery capability is introduced.

## Risks and deferred work

- **Abort API — explicitly deferred by maintainer, not implemented.** Preserve
  the recovery need: inspect retained imports and deliberately discard selected
  refs without racing active imports or silently orphaning needed data. A future
  design must settle ownership/reachability, target selection, any bulk-delete
  semantics, auth/errors, app-contract routing and coherent public interface
  coverage/conformance. Do not silently revive the historical bare --abort
  clears-all guess or treat a CLI-only surface as approved.
- Failed/legacy refs can accumulate until successful retry or operator recovery;
  do not add broad auto-GC. Errors identify the relevant payload/namespace.
- Separate three-write-lease HIGH, unlocked preparation and general rebuild
  races remain out of scope. Import serialization is not a sync-wide repair.
- SHA-256 collision resistance is an identity assumption, not absolute uniqueness.
  Unexpected candidate refs cause refusal rather than force/delete.
- Mixed-version concurrent importers and manual ref edits do not obey the new
  advisory lease. Preserve that compatibility limit in the docs.
- FT-7a landed 2026-09-28 as 3b27002 (review: dev/local/reviews/FT-7a-fast-track-review.md).
  Deferred review follow-ups, owned here:
  - `ddb-core/src/bundle/tests.rs` is 1,847 lines (limit 800; 831 at base). Split
    the import regressions into their own test module (needs an allowlist entry).
  - `docs/src/technical/errors.md:20`, `docs/src/architecture/overview.md:154`,
    `docs/src/technical/phase-2-exit.md:45` still cite `ddb-core/src/bundle.rs`.
  - DONE 2026-09-28: both lease tests and the tag test passed on windows-full
    and macos-full in Full Validation run 36443671857 (headSha 3b27002).
- FT-7b landed 2026-09-29 as ab14498 (build: dev/local/reviews/FT-7b-build.md;
  review: dev/local/reviews/FT-7b-fast-track-review.md). The full roster found
  0 CRITICAL/HIGH. One rework followed, and 8/8 closure checks came back CLOSED.
  The fresh a739c29 replay is red at prune=false too (6/6). Exact-HEAD CI:
  Test 36590016013 and Docs 36590016016 both succeeded. The held 00168 note was
  annotated (CRITICAL closed, write-lease HIGH open). Deferred review follow-ups,
  owned here:
  - `ddb-core/src/ffi/tests.rs` is 1,667 lines (1,085 at base). Split the bundle
    workflow into its own test module; this needs an allowlist entry.
  - The golden-workflow fixture is duplicated between ddb-core FFI tests and the
    e2e crate. Sharing it needs a ddb-core test-support surface.
  - The CLI test takes B's payload id from the new namespace. Hashing
    `objects.bundle` there needs sha2/tar dev-deps in tests/Cargo.toml. The FFI
    and core tests already pin the SHA-256 link.
  - The fetch.prune axis cannot be observed on the new runtime: removing
    `--no-prune` keeps the tests green, because the refspec is per-payload. The
    loop stays as a regression guard.
  The abort API stays deferred.

## Unresolved questions

None for the approved bounded scope. The future abort API needs its own design.
