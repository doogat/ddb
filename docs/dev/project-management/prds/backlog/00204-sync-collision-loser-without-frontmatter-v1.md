---
default_model: opus
model_tier_rationale: changes the write-locked merge commit path (P0 data-safety blast radius), invents a new core result type plus a public GraphQL field, and breaks the GitMerge trait signature across core, CLI and server.
design: run
---

# Unwedge sync when a collision loser has no frontmatter

**Priority:** P1 · **Roadmap:** deferred-ledger tail (assessment 2026-09-06) · **Depends on:** none

## Overview

### Problem Statement

An add-add collision whose LOSING content carries no YAML frontmatter wedges sync permanently. `fold_one_loser` calls `parser::rewrite_id_field` (`ddb-core/src/git_ops/merge.rs:192`), which parses the loser through `find_frontmatter` and fails with `Parse("no frontmatter opening ---")` (`ddb-core/src/parser/mod.rs:180-184`, `:728`). The error propagates with `?` and aborts `commit_merge` before its single `create_commit`, so nothing lands — not the winner, not any earlier loser. The next sync recomputes the identical merge and fails identically: no `--skip`, no `--force`, no documented recovery. Before PRD 00167 the winner commit landed and sync converged (lossily); today it never converges. Provenance: `dev/local/audit-results/refactor-assessment-2026-09-06.md` (unowned deferred tail, "00167 frontmatter-less collision-loser sync wedge (no recovery path)") and `dev/local/autopilot/deferred/202607161128-deferred.json` (00167 cycle 2, topic `malformed-loser-wedges-sync`, HIGH, deferred at the rework cap with no PRD home).

The abort itself was the correct 00167 ruling for the general case: skipping a loser DELETES its content, because a loser only survives by being reassigned. But this specific case has a third option that is neither abort nor data loss, and it is coherent precisely because the failing content has no frontmatter `id` to leave stale: fold the loser at its derived content-id path with its bytes unchanged.

**Decision (maintainer, 2026-09-06, settled — do not re-open):** implement the byte-unchanged fold. It applies when, and only when, the losing content has no parseable frontmatter block; a parse failure for any other reason (well-formed delimiters, unparseable YAML inside) still aborts the whole merge exactly as it does today.

### Target Users

Operators syncing two or more ddb devices where a `ddb/{id}.md` was hand-authored (outside ddb's own write paths, which always emit frontmatter) on two devices under the same id. Their sync is currently dead with no operator recourse. Also downstream apps (jink) whose sync call never succeeds again once this state is reached.

### Success Metrics

- A two-device add-add collision with a frontmatter-less loser converges in ONE sync round (today: never).
- Zero bytes lost: the loser's file content is byte-identical before and after the merge.
- The event is machine-readable, not log-only: it appears on `SyncReport` and on every transport that renders `SyncReport`.
- No behavior change for losers WITH frontmatter (id-rewritten as today) or for other parse failures (still abort atomically).

## Functional Decomposition

### Capability: Frontmatter-less loser fold

Decide per loser whether its content can carry a rewritten id, and fold it into the merge tree either way.

#### Feature: Classify loser content by frontmatter presence
- **Description**: One pure predicate decides whether a loser's bytes contain a usable frontmatter block.
- **Inputs**: `&str` loser content.
- **Outputs**: `bool`.
- **Behavior**: `parser::has_frontmatter(content)` is true iff `find_frontmatter` locates BOTH the opening and the closing `---` line (`ddb-core/src/parser/mod.rs:180-192`). Missing opening, missing closing, or empty content are all false. Nothing else is inspected — a well-formed block whose YAML is invalid is TRUE here, so it still reaches `rewrite_id_field` and still aborts.

#### Feature: Fold a frontmatter-less loser byte-unchanged
- **Description**: When the predicate is false, the loser lands at its derived path with its original bytes instead of aborting the merge.
- **Inputs**: `CollisionLoser`, the in-memory merge index, the winner commit, the taken-id set.
- **Outputs**: an index entry at the derived path; one kept-verbatim record.
- **Behavior**: `id_minting::derive_content_id(old_id, losing_blob_oid, exists)` (`ddb-core/src/id_minting.rs:71`) still supplies the new id and `doogat_path` the new path, so both devices derive the same destination. `rewrite_id_field` is skipped, the original bytes are blobbed, and inbound-link rewriting still runs unchanged. Everything stays inside the existing `with_write_lock` and the single `create_commit`.

### Capability: Report the verbatim fold

Make the event visible to operators and downstream apps, not only to `tracing`.

#### Feature: Merge outcome carries kept-verbatim records
- **Description**: `commit_merge` returns the commit hash plus what it kept verbatim.
- **Inputs**: the losers batch.
- **Outputs**: `MergeCommitOutcome { commit: CommitHash, losers_kept_verbatim: Vec<CollisionLoserKeptVerbatim> }`.
- **Behavior**: replaces the bare `Result<CommitHash>` on the inherent method, the `GitMerge` trait (`ddb-core/src/traits.rs:408-415`) and the delegating impl (`ddb-core/src/git_ops/mod.rs:714-723`). Records are emitted only after `create_commit` succeeds, so an aborted merge reports nothing.

#### Feature: SyncReport field and transport renders
- **Description**: the records reach the CLI and GraphQL sync surfaces.
- **Inputs**: `MergeCommitOutcome` at `ddb-core/src/sync_manager/mod.rs:372`.
- **Outputs**: `SyncReport.collision_losers_kept_verbatim: Vec<CollisionLoserKeptVerbatim>`.
- **Behavior**: the new field lands in the core type first (`ddb-core/src/types/doogat.rs:218-232`), initialized empty at the report's single construction site (`ddb-core/src/sync_manager/mod.rs:283-291`). These losers are still counted by `collisions_reassigned` — they ARE reassigned — and additionally listed here. CLI and GraphQL each render it; no other transport exposes sync (see Transport Impact).

## Structural Decomposition

### Repository Structure

```
ddb-core/src/
├── parser/mod.rs              # Maps to: Classify loser content by frontmatter presence
├── git_ops/merge.rs           # Maps to: Fold a frontmatter-less loser byte-unchanged
├── traits.rs, git_ops/mod.rs  # Maps to: Merge outcome carries kept-verbatim records
├── types/doogat.rs            # Maps to: SyncReport field and transport renders
└── sync_manager/mod.rs        # Maps to: SyncReport field and transport renders
ddb-cli/src/commands/sync.rs   # Maps to: SyncReport field and transport renders
ddb-server/src/schema/mutations/operations.rs  # Maps to: SyncReport field and transport renders
```

### Module: parser
- **Maps to capability**: Frontmatter-less loser fold
- **Responsibility**: pure content classification; no git, no id knowledge.
- **Exports**:
  - `has_frontmatter(&str) -> bool` - true iff both frontmatter delimiters are present

### Module: git_ops::merge
- **Maps to capability**: Frontmatter-less loser fold; Report the verbatim fold
- **Responsibility**: build the merge tree, fold every loser, commit once under the write lock; owns the `GitMerge::commit_merge` declaration in `traits.rs` and its delegating impl in `git_ops/mod.rs`.
- **Exports**:
  - `commit_merge(...) -> Result<MergeCommitOutcome>` - the single merge-commit entry point, inherent and via `GitMerge`

### Module: types
- **Maps to capability**: Report the verbatim fold
- **Responsibility**: adapter-neutral shapes shared by every transport.
- **Exports**:
  - `CollisionLoserKeptVerbatim { old_id, old_path, new_path }` - one verbatim fold
  - `MergeCommitOutcome` - commit hash plus the verbatim folds
  - `SyncReport.collision_losers_kept_verbatim` - per-sync list

### Module: sync_manager
- **Maps to capability**: Report the verbatim fold
- **Responsibility**: map merge outcomes into the sync report; owns no fold policy.
- **Exports**:
  - `SyncManager::sync` - unchanged signature, richer report

### Module: ddb-cli::commands::sync
- **Maps to capability**: Report the verbatim fold
- **Responsibility**: render one `SyncReport` to stdout; holds no policy and re-derives nothing.
- **Exports**:
  - `sync()` - prints the verbatim-fold lines when the vector is non-empty

### Module: ddb-server::schema::mutations::operations
- **Maps to capability**: Report the verbatim fold
- **Responsibility**: serialize the same `SyncReport` into the GraphQL sync mutation result.
- **Exports**:
  - `collisionLosersKeptVerbatim` - GraphQL list mirroring the core vector field-for-field

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **parser**: the `has_frontmatter` predicate
- **types**: `CollisionLoserKeptVerbatim`, `MergeCommitOutcome`, the `SyncReport` field

### Core Layer (Phase 1)
- **git_ops::merge**: Depends on [parser, types]
- **sync_manager**: Depends on [git_ops::merge, types]

### Integration Layer (Phase 2)
- **ddb-cli / ddb-server / docs**: Depends on [sync_manager, types]

## Implementation Phases

### Phase 0: Foundation
**Goal**: the predicate and the shapes exist, with nothing depending on them yet.

**Tasks**:
- [ ] Add `parser::has_frontmatter(&str) -> bool` beside `find_frontmatter`, true iff both delimiters are found (no deps) - Acceptance: `rg -q "fn has_frontmatter_rejects_content_with_no_opening_delimiter" ddb-core/src/parser` exits 0 (a cargo filter matching zero tests exits 0, so presence is asserted separately) and `cargo test -p ddb-core has_frontmatter` passes it plus `has_frontmatter_rejects_unterminated_block` and `has_frontmatter_accepts_well_formed_block`.
- [ ] Add `CollisionLoserKeptVerbatim` and `MergeCommitOutcome` to `types/doogat.rs`; add `collision_losers_kept_verbatim` to `SyncReport`, initialized empty at `sync_manager/mod.rs:283-291` (no deps) - Acceptance: `cargo build --workspace` succeeds and `rg -q "collision_losers_kept_verbatim" ddb-core/src/types/doogat.rs` exits 0.

**Exit Criteria**: workspace builds; the predicate is unit-tested; no merge behavior has changed yet.

### Phase 1: Core
**Goal**: a frontmatter-less loser converges instead of wedging, and every other loser behaves exactly as before.

**Tasks**:
- [ ] In `fold_one_loser`, skip `rewrite_id_field` when `!parser::has_frontmatter(&loser.content)`, blob the original bytes at the derived path, and return a kept-verbatim record; thread `MergeCommitOutcome` through `commit_merge`, `traits.rs:408-415`, `git_ops/mod.rs:714-723` and `sync_manager/mod.rs:372`; replace the atomic-abort doc comment at `git_ops/merge.rs:127-134` with the two-branch rule. Premise: that comment still states the unconditional abort; re-check it at execution, rewrite it wherever it now lives, and report the drift if it already reads otherwise (the fold itself is unconditional and always ships) (depends on: Phase 0) - Acceptance: `rg -q "MergeCommitOutcome" ddb-core/src/traits.rs` exits 0, `cargo clippy --workspace --all-targets` is clean, and `cargo test-ci` passes.
- [ ] Add `git_ops/tests.rs` test `frontmatterless_loser_lands_byte_identical_at_its_derived_path`: the merge commits, HEAD moves, the winner lands, and the loser's bytes at `doogat_path(derive_content_id(old_id, losing_blob_oid))` equal its input bytes exactly. Premise: `git_ops/tests.rs:2672` `commit_merge_commits_nothing_when_one_loser_in_the_batch_cannot_be_rewritten` currently asserts the opposite for this input and must be re-pointed at a YAML-invalid loser; re-check that test name and its frontmatter-less fixture at execution and skip-and-report if it has moved (depends on: Phase 1 fold) - Acceptance: `rg -q "fn frontmatterless_loser_lands_byte_identical_at_its_derived_path" ddb-core/src/git_ops/tests.rs` exits 0 and `cargo test -p ddb-core frontmatterless_loser_lands_byte_identical_at_its_derived_path` passes.
- [ ] Add the two control tests: `loser_with_frontmatter_is_still_id_rewritten` (bytes equal `rewrite_id_field(content, derived_id)`, not the input) and `loser_with_unparseable_yaml_still_aborts_the_whole_batch` (well-formed `---` delimiters, invalid YAML body: `commit_merge` errs, HEAD does not move, nothing lands) (depends on: Phase 1 fold) - Acceptance: `rg -c "fn loser_with_" ddb-core/src/git_ops/tests.rs` reports 2 and `cargo test -p ddb-core loser_with_` runs both and passes.
- [ ] Map `MergeCommitOutcome.losers_kept_verbatim` onto `SyncReport` in `create_conflict_commit`/`resolve_merge_conflicts`, keeping these losers counted in `collisions_reassigned`; add `sync_manager/tests.rs` test `sync_report_lists_every_verbatim_folded_collision_loser` (depends on: Phase 1 fold) - Acceptance: `rg -q "fn sync_report_lists_every_verbatim_folded_collision_loser" ddb-core/src/sync_manager/tests.rs` exits 0 and `cargo test -p ddb-core sync_report_lists_every_verbatim_folded_collision_loser` passes.
- [ ] Re-point the bundle-import expectations that assert this input FAILS the import. Premise: `ddb-core/src/bundle/tests.rs:299` and `:335` build `import_with_unresolvable_collision` from a frontmatter-less loser and expect `Err`; that fixture now converges. Re-check both at execution; if the fixture no longer has that shape, skip and report (depends on: Phase 1 fold) - Acceptance: `cargo test -p ddb-core bundle::tests` passes with the frontmatter-less import asserted to SUCCEED and report one verbatim fold.

**Exit Criteria**: `cargo test-ci` green; a frontmatter-less loser commits, a YAML-invalid loser still aborts, and the report names the fold.

### Phase 2: Integration
**Goal**: operators and downstream apps can see the fold, and the behavior is documented and demonstrated.

**Tasks**:
- [ ] Render the field on both sync transports: a CLI line `  collision losers kept verbatim: N` (guess) printed only when non-empty, then one `    {old_path} -> {new_path}` line each (`ddb-cli/src/commands/sync.rs:18-20` pattern); a GraphQL `collisionLosersKeptVerbatim` (guess) list of `{ oldId, oldPath, newPath }` objects on the sync mutation result (`ddb-server/src/schema/mutations/operations.rs:360-363` pattern) (depends on: Phase 1) - Acceptance: `cargo test-ci` passes and `rg -q "collisionLosersKeptVerbatim" ddb-server/src/schema/mutations/operations.rs` exits 0.
- [ ] Author e2e scenario `add_add_collision_with_frontmatterless_loser_converges_in_one_sync` in `tests/e2e/collision.rs` (module already registered at `tests/e2e/main.rs:11`): two nodes commit the same `ddb/{id}.md`, one side without any frontmatter; `ddb sync` on the resolving node exits 0, prints the verbatim line, both files survive, and the frontmatter-less bytes are unchanged on disk (depends on: Phase 2 renders) - Acceptance: `rg -q "fn add_add_collision_with_frontmatterless_loser_converges_in_one_sync" tests/e2e/collision.rs` exits 0 and `cargo build --workspace --tests` succeeds; execution is Tier 2 CI.
- [ ] Update `docs/src/technical/sync.md` (replace the atomic-abort paragraph at `:121` with the two-branch rule plus recovery semantics: what the operator sees, where the file lands, that it stays unindexable until frontmatter is added), note the branch under I1 in `docs/src/technical/invariants.md`, add CHANGELOG `Fixed` (the wedge) and `Changed` (`GitMerge::commit_merge` return type) entries, and record the walkthrough in `dev/local/walkthroughs/` via the showboat CLI (depends on: Phase 2 renders) - Acceptance: `rg -q "kept verbatim" docs/src/technical/sync.md CHANGELOG.md` exits 0 and the walkthrough file exists.

**Exit Criteria**: `cargo build`, `cargo clippy --workspace --all-targets`, `cargo test-ci` all clean; docs, CHANGELOG and walkthrough describe the converging behavior.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root.

### Critical Scenarios
- **Happy path**: add-add collision, loser has no frontmatter → merge commits once; winner keeps the contested id; loser's bytes land byte-identical at `derive_content_id(old_id, losing_blob_oid)`; `SyncReport` lists one verbatim fold; next sync is a no-op.
- **Happy path (unchanged)**: add-add collision, loser HAS frontmatter → id-rewritten exactly as today, absent from the verbatim list.
- **Edge case**: two losers in one batch, one frontmatter-less and one normal → both land in the SAME commit; the verbatim list holds only the first.
- **Edge case**: two devices resolve the same frontmatter-less collision in opposite merge directions → identical destination path on both, because the derivation is unchanged.
- **Error case**: loser with well-formed `---` delimiters but unparseable YAML → `commit_merge` still errs, HEAD does not move, nothing lands (the 00167 atomicity guarantee survives for every non-frontmatter-less failure).

## Risks

- **Weakening the 00167 atomic-abort ruling**: the fold is gated on one narrow, structurally-checked predicate, and the YAML-invalid control test pins that every other failure still aborts the whole batch. The predicate is evaluated in exactly one place.
- **Breaking `GitMerge::commit_merge`**: a public library signature change with one production caller (`sync_manager/mod.rs:372`) plus tests. Mitigated by a CHANGELOG `Changed` entry, matching 00167's own precedent for this method.
- **A frontmatter-less doogat is unindexable**: it lands in git but the indexer will skip it as malformed and report it through the I5 poison-file warning path. That is strictly better than the wedge, and the docs task states it as the operator's follow-up (add frontmatter, commit).
- **Not covered here**: the same ledger's `independent-resolution-test-can-false-green` HIGH (`ddb-core/src/sync_manager/tests.rs:1888`) is deliberately out of scope — the ledger calls that fix cheap, but `resolve_collision_with_push_order` returns `Vec<String>` filenames (`ddb-core/src/sync_manager/tests.rs:1800`) and must first surface the losing side's blob OID before the derived path can be pinned, so it is more than a one-assertion change and stays out of this PRD. It remains unowned.

## Transport Impact

- **CLI**: CHANGED. `ddb sync` gains the verbatim-fold lines (printed only when non-empty). Covered by the e2e scenario in `tests/e2e/collision.rs`.
- **GraphQL**: CHANGED. The sync mutation result gains `collisionLosersKeptVerbatim`; purely additive, no existing field changes shape.
- **REST**: no change. `ddb-server/src/rest.rs` registers only `/doogats`, `/doogats/{id}` and `/schema/apply` — there is no sync endpoint to render.
- **PgWire**: no change. The wire is SQL-only; sync is not a SQL verb.
- **FFI**: no change. `DoogatDriver` exposes no `sync` method and `import_bundle` already discards its `SyncReport` (a pre-existing gap tracked in the 00168 deferred ledger, not widened here).
- **NoSQL HTTP**: no change. `ddb-server/src/nosql_api.rs` serves read-only `GET /{id}`, `GET /` and `GET /{id}/backlinks`.
- **Cross-interface conformance plan**: sync is a two-transport capability (CLI + GraphQL). Conformance is the shared core assertion — both transports read the SAME `SyncReport.collision_losers_kept_verbatim` vector with no per-transport field subset — plus the e2e CLI scenario above and a GraphQL assertion in the same run that the mutation returns one entry with the identical `oldPath`/`newPath` the CLI printed. No transport re-derives the list.

## Deferred work (FT-8a review, 2026-10-02)

Phases 0-1 landed as d7867d2 (fast-track FT-8a; report
`dev/local/reviews/FT-8a-fast-track-review.md`). Left open by maintainer
decision or scope:

- **Horizontal-rule residual wedge (maintainer: pin and defer).**
  `has_frontmatter` follows `find_frontmatter`, which accepts the first `---`
  line anywhere. A frontmatter-less body with two `---` lines takes the
  rewrite branch: it still wedges when the text between them is not a YAML
  map, and (pre-existing) loses every line before the first `---` when it
  is. Pinned by `has_frontmatter_counts_paired_horizontal_rules_as_a_block`.
  A fix must anchor the opening delimiter to the first non-blank line in
  `split_zones` AND the predicate together, or the parser and the predicate
  disagree. Needs its own PRD (it changes how every doogat parses).
- **Oversized test files.** `git_ops/tests.rs` (~4000 lines),
  `sync_manager/tests.rs`, `bundle/tests.rs`, `ffi/tests.rs` and
  `parser/tests.rs` are over 800 lines (all were before FT-8a). Split
  together with the FT-7 deferral.
- **Phase 2 (FT-8b)** also owns: interface-level success tests (CLI e2e and
  GraphQL), the walkthrough 00203 re-record, sync.md:121 and invariants I1,
  and a recovery note that a verbatim-kept file is unindexable (and
  `reindex --strict` fails) until frontmatter is added.
