---
default_model: opus
model_tier_rationale: destructive eviction blast radius plus an equivalence obligation - both eviction legs must key off one captured identity without regressing rename-shaped change-sets
design: run
---

# Evict index rows by indexed identity, not by file stem

**Priority:** P2 (bounded index-corruption class; derived cache, git is source of truth) · **Roadmap:** pre-flight (assessment 2026-09-06) · **Depends on:** none

## Overview

### Problem Statement

Both incremental-reindex eviction legs decide *which row to delete* from the filename, not from what the index actually holds for that path.

- The poison-eviction filter maps each skipped file's path through `crate::parser::extract_id_from_path` (`ddb-core/src/indexer/rebuild.rs:112`), which is a bare `Path::file_stem` (`ddb-core/src/parser/mod.rs:972-977`).
- The pre-existing delete leg derives its IDs through the same call (`ddb-core/src/indexer/rebuild.rs:190`), so the two legs share one filename-is-identity model.
- `doogats` already records the true mapping: `path TEXT UNIQUE NOT NULL` beside the `id` primary key (`ddb-core/src/indexer/mod.rs:51-59`). Nothing consults it when evicting.

Consequences, both bounded but wrong: a file whose frontmatter `id` differs from its stem keeps its stale row when it is poisoned or deleted, and a newly added poison file whose stem equals another doogat's id evicts *that unrelated doogat* instead. Reachability is narrow - it needs a hand-authored `ddb/{stem}.md` whose frontmatter id differs from its stem, since every ddb write path names the file after the id - and a full rebuild self-heals both cases.

Provenance: the 00169 cycle-2 review's single HIGH (`dev/local/reviews/00169-poison-file-reindex-resilience-v1-review-02.md:72`, orchestrator verdict at `:103-124`), carried into the deferred ledger as `eviction-keys-off-file-stem-not-indexed-identity` (`dev/local/autopilot/deferred/202607161128-deferred.json:164-168`), and listed as an unowned HIGH by the 2026-09-06 assessment (`dev/local/audit-results/refactor-assessment-2026-09-06.md:66`, remediation at `:151`). Two medium items from the same ledger entry (`:178`) ride along by maintainer decision: `evict_doogats` swallows its type-lookup and unmaterialize failures (`rebuild.rs:151-159`, `:366`, `:373`), and the newly public `describe_consistency_warning` ships without rustdoc (`ddb-core/src/app_contract/mod.rs:21`).

Ruled out by the same review and NOT to be proposed here: folding both eviction calls after `batch_index_changes`. Eve proved it deletes a just-indexed row on rename-shaped change-sets (a deleted `ddb/_typedef/X.md` and an added `ddb/X.md` share a stem). Eviction ordering stays as it is - deletes before indexing (`rebuild.rs:97`), poison after (`rebuild.rs:117`).

### Target Users

Downstream apps (jink) whose repos take externally-authored markdown over sync, where a file's name and its frontmatter id can legitimately disagree. Also every operator who expects `ddb` reads to reflect git without a manual `ddb reindex`.

### Success Metrics

- Both legs evict the row the index holds for the changed path; neither derives a victim id from a file stem when the index knows the path.
- A poisoned or deleted file whose frontmatter id differs from its stem leaves no stale `doogats`/satellite/FTS/type-table row.
- A newly added poison file never evicts a row it does not own, whatever its stem.
- A rename-shaped change-set still indexes the new path and evicts the old row (no regression from the refuted fold).
- `evict_doogats` returns `Err` on a type-lookup or unmaterialize failure instead of advancing HEAD over incomplete cleanup; zero swallow sites remain in the eviction path.
- `describe_consistency_warning` carries rustdoc naming it the one core warning-rendering definition transports use.

## Functional Decomposition

### Capability: Identity-true eviction

#### Feature: Capture indexed identity before the change-set is applied
- **Description**: snapshot each changed path's currently indexed `(id, type)` before any eviction, parse, or index write runs.
- **Inputs**: the partitioned change-set paths (added, modified, deleted) from `partition_changes` (`rebuild.rs:172-198`).
- **Outputs**: a `path -> (id, type)` map holding one entry per path the index currently knows.
- **Behavior**: one query keyed on the `doogats.path` unique column; a path the index does not hold yields no entry (it is new, so there is nothing to evict).

#### Feature: Delete leg evicts the captured row
- **Description**: deletions evict the row captured for the deleted path, not the row named by the path's stem.
- **Inputs**: the deleted paths; the captured map.
- **Outputs**: `remove_doogat` plus type-table cleanup applied to exactly the captured id.
- **Behavior**: captured entry wins. With no captured entry, fall back to the stem-derived id only when `doogats` holds that id at a path that normalizes to the deleted path (a leading `./` trimmed) (guess: the normalization rule; exact-match is the observed shape today, the trim is defensive); otherwise evict nothing and log the mismatch.

#### Feature: Poison leg evicts the captured row
- **Description**: a file the change-set turned unreadable or malformed loses exactly its own previously indexed row.
- **Inputs**: the `UnreadableFile` / `MalformedYaml` warning paths from `batch_index_changes`; the captured map.
- **Outputs**: the same scoped eviction as the delete leg.
- **Behavior**: same rule, same fallback. A path with no captured entry (an added poison file) evicts nothing - the stem fallback cannot fire because no row for that id sits at that path.

### Capability: Loud eviction failures

#### Feature: Eviction errors propagate
- **Description**: `evict_doogats` stops converting SQLite failures into silence.
- **Inputs**: the type lookup (`rebuild.rs:151-159`), the type-table existence probe (`:360-366`), the type-table delete (`:373-376`).
- **Outputs**: `Err(DoogatError::Index(..))` naming the id and the failing stage; HEAD is not stored past a failed eviction.
- **Behavior**: `.ok().flatten()`, `unwrap_or(false)` and `let _ = execute(..)` become `?`. This is index-write failure, not per-file poison, so I5's degrade-per-item rule does not apply - a genuinely absent type table is still a skip, not an error.

#### Feature: Documented warning renderer
- **Description**: `describe_consistency_warning` gets rustdoc stating it is the single core definition transports render.
- **Inputs**: none (docs only).
- **Outputs**: rustdoc on `ddb-core/src/app_contract/mod.rs:21`.
- **Behavior**: names the callers (CLI `maintenance.rs:68`, FFI `ffi/driver.rs:130`, the envelope summary) so no transport re-invents the text.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/indexer/
├── eviction.rs          # NEW - Maps to: Capture indexed identity; Delete leg; Poison leg; Eviction errors propagate
├── rebuild.rs           # Maps to: incremental_reindex wiring (capture -> delete evict -> index -> poison evict)
└── mod.rs               # Maps to: `mod eviction;` declaration beside `mod rebuild;` (:1-8)
ddb-core/src/app_contract/
└── mod.rs               # Maps to: Documented warning renderer (:21)
ddb-core/tests/
└── index_eviction_identity.rs   # NEW - Maps to: identity + ordering regressions (indexer/tests/mod.rs is 6784 lines)
tests/e2e/
└── index_eviction_identity.rs   # NEW - Maps to: end-to-end mismatched-id eviction; registered in tests/e2e/main.rs
```

### Module: eviction (ddb-core/src/indexer/eviction.rs)
- **Maps to capability**: Identity-true eviction; Loud eviction failures
- **Responsibility**: own every "which row does this path mean" decision and the eviction it drives; `rebuild.rs` keeps only sequencing.
- **Exports**:
  - `capture_indexed_identities(paths) -> Result<HashMap<String, (String, String)>>` - snapshot before the change-set is applied
  - `evict_paths(captured, paths) -> Result<()>` - evict the captured rows for these paths, with the narrow stem fallback
  - `evict_doogats(ids)` - moved here from `rebuild.rs:148`, now error-propagating

### Module: rebuild (ddb-core/src/indexer/rebuild.rs)
- **Maps to capability**: Identity-true eviction
- **Responsibility**: sequence the incremental pass; hold no identity logic.
- **Exports**: `incremental_reindex(..)` - unchanged signature; body now captures identities first, then evicts deletes (`:97`), indexes (`:100`), evicts poison (`:117`)

### Module: app_contract (ddb-core/src/app_contract/mod.rs)
- **Maps to capability**: Loud eviction failures
- **Responsibility**: the one core rendering of a `ConsistencyWarning` for every transport.
- **Exports**: `describe_consistency_warning(..)` - behavior unchanged, rustdoc added

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **eviction**: path-keyed identity capture and error-propagating eviction primitives.

### Core Layer (Phase 1)
- **rebuild**: Depends on [eviction] - both legs routed through the captured identities.

### Integration Layer (Phase 2)
- **tests/e2e + docs + CHANGELOG**: Depends on [rebuild, eviction] - end-to-end proof and the written contract.
- **app_contract**: no dependency; rides Phase 2 as a docs-only change.

## Implementation Phases

### Phase 0: Foundation
**Goal**: an eviction module that resolves a path to the row the index actually holds and fails loudly.

**Tasks**:
- [ ] Create `ddb-core/src/indexer/eviction.rs` with `capture_indexed_identities` (query on the `doogats.path` unique column, `indexer/mod.rs:56`) and declare `mod eviction;` in `indexer/mod.rs` (no deps) - Acceptance: `cargo test -p ddb-core capture_resolves_indexed_id_when_frontmatter_id_differs_from_file_stem` passes.
- [ ] Move `evict_doogats` (`rebuild.rs:148-168`) and `unmaterialize_data_doogats` (`rebuild.rs:353-379`) into `eviction.rs` and replace `.ok().flatten()`, `unwrap_or(false)` and `let _ = self.conn.execute(..)` with `?` (depends on: eviction module) - Acceptance: `cargo test -p ddb-core evict_returns_err_when_the_type_table_delete_fails` passes (a `BEFORE DELETE ... RAISE(ABORT)` trigger forces the failure), and `rg -n "ok\(\)\.flatten\(\)|unwrap_or\(false\)|let _ = self\.conn\.execute" ddb-core/src/indexer/eviction.rs` returns no match. Premise: those three swallow sites are still present at the cited lines; if a prior PRD already removed them, skip the replacement, keep the move, and report the premise as failed.

**Exit Criteria**: the index can answer "which row is at this path" and no eviction failure is silent.

### Phase 1: Core
**Goal**: both legs evict the captured row; the refuted fold stays refuted.

**Tasks**:
- [ ] Capture identities for every changed path at the top of `incremental_reindex` and route the delete leg (`rebuild.rs:190`) through them, with the narrow stem fallback (depends on: Phase 0) - Acceptance: `cargo test -p ddb-core deleting_a_file_evicts_its_indexed_row_not_the_row_named_by_its_stem` passes.
- [ ] Route the poison leg (`rebuild.rs:107-117`) through the captured identities, dropping the `extract_id_from_path` call (depends on: delete leg) - Acceptance: `cargo test -p ddb-core poisoning_a_file_evicts_its_indexed_row_not_the_row_named_by_its_stem` and `cargo test -p ddb-core an_added_poison_file_does_not_evict_an_unrelated_row_sharing_its_stem` both pass. Premise: the poison filter still calls `crate::parser::extract_id_from_path` at `rebuild.rs:112`; if that leg is already identity-keyed, skip and report the premise as failed.
- [ ] Pin the eviction ordering against the refuted post-index fold with a rename-shaped regression (deleted `ddb/_typedef/X.md` + added `ddb/X.md`) (depends on: both legs) - Acceptance: `cargo test -p ddb-core rename_shaped_changeset_indexes_the_new_path_and_evicts_only_the_old_row` passes and fails if the two eviction calls are folded after `batch_index_changes`.

**Exit Criteria**: no eviction path derives a victim id from a file stem while the index knows the path; `cargo clippy --workspace --all-targets` is clean.

### Phase 2: Integration
**Goal**: the behavior is proven end to end and written down.

**Tasks**:
- [ ] Author `tests/e2e/index_eviction_identity.rs` (raw-git commit of `ddb/{stem}.md` whose frontmatter id differs, then modify it into poison and query the survivors) and register `mod index_eviction_identity;` in `tests/e2e/main.rs` beside `mod poison_file_reindex;` (`tests/e2e/main.rs:59`) (depends on: Phase 1) - Acceptance: the file defines `stale_row_is_evicted_when_frontmatter_id_differs_from_file_stem` and `cargo build -p ddb-cli` then `cargo test -p ddb-e2e index_eviction_identity` passes (this task replaces an existing code path, so CLAUDE.md's conditional e2e run applies).
- [ ] Rewrite the eviction paragraph in `docs/src/technical/indexer.md:134` and the I5 "Current state" text in `docs/src/technical/invariants.md:46` to state that eviction keys off the indexed `(id, type)` captured per path, and add a `### Fixed` CHANGELOG entry (depends on: Phase 1) - Acceptance: `rg -n "indexed identity" docs/src/technical/indexer.md docs/src/technical/invariants.md CHANGELOG.md` matches in all three files and `rg -n "00205" CHANGELOG.md` matches under `### Fixed`. Premise: `indexer.md:134` still describes poison eviction without naming the indexed identity and `invariants.md:46` still closes I5 on 00169 alone; if either already states identity-true eviction, edit only what remains and report.
- [ ] Add rustdoc to `describe_consistency_warning` (`ddb-core/src/app_contract/mod.rs:21`) naming it the single core warning rendering transports consume (no deps) - Acceptance: `rg -n -B3 "^pub fn describe_consistency_warning" ddb-core/src/app_contract/mod.rs` shows a `///` line. Premise: the function is still undocumented at that line; if rustdoc is already present, skip and report.

**Exit Criteria**: docs, CHANGELOG and an e2e scenario describe and pin identity-true eviction.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root.

Phase 0's two regressions are colocated unit tests inside `ddb-core/src/indexer/eviction.rs`, because `capture_indexed_identities` and `evict_doogats` are crate-private; `cargo test-ci` (`--lib --bins`, `.cargo/config.toml:4`) runs them. The Phase 1 identity and ordering regressions live in a new `ddb-core/tests/index_eviction_identity.rs` (`ddb-core/src/indexer/tests/mod.rs` is 6784 lines, far past the 800-line ceiling), driving the public `Index::open` (`indexer/mod.rs:140`) / `rebuild` (`rebuild.rs:443`) / `incremental_reindex` (`rebuild.rs:53`) surface over a `GitRepo` fixture the same way `ddb-core/tests/query_value_indexer_t141.rs` does; `test-ci` skips integration targets, so each task's named `cargo test -p ddb-core` acceptance is the gate that runs them. Every test asserts the victim row *and* a survivor row, so a total-wipe implementation cannot pass. No showboat walkthrough: this PRD adds no CLI command, endpoint, or user-visible output.

### Critical Scenarios
- **Happy path**: a file with stem `A` and frontmatter id `B` is indexed, then modified into malformed YAML → Expected: row `B` and its satellite/FTS/type-table rows are gone; any row `A` is untouched.
- **Edge case**: a new poison file `ddb/{id-of-another-doogat}.md` is committed → Expected: nothing is evicted; the other doogat keeps every row; the skip warning still names the poison path.
- **Edge case**: a rename-shaped change-set deletes `ddb/_typedef/X.md` and adds `ddb/X.md` → Expected: the new path is indexed and survives; the old typedef row is gone.
- **Error case**: the type-table delete fails during eviction → Expected: `incremental_reindex` returns `Err` naming the id and stage; the stored HEAD does not advance.

## Risks

- **The fallback re-opens the bug**: a permissive stem fallback would evict an unrelated row exactly as today. Mitigation: the fallback fires only when `doogats` holds that id at a path normalizing to the changed path, and the added-poison-file test fails if it widens.
- **Capture snapshot goes stale mid-pass**: identities captured before indexing could disagree with rows written during it. Mitigation: capture covers only pre-existing rows, the delete leg still runs before indexing, and the rename-shaped test pins that ordering.
- **Propagating eviction errors turns a warn into a hard failure**: a repo with a broken type table would start erroring where it used to continue. Mitigation: a genuinely absent type table stays a skip (the existence probe's `false` branch), and only real SQLite failures propagate.
- **Moving `evict_doogats` churns the 00169 regressions**: those tests live at `ddb-core/src/indexer/tests/mod.rs:6247+` and exercise the private path. Mitigation: the move keeps the method on `Index`, so call sites and existing tests compile unchanged.

## Transport Impact

- **CLI**: no change - `ddb reindex` output, exit codes and warning lines are untouched; eviction sits below the verb.
- **GraphQL**: no change - no schema, response or warning-extension change.
- **REST**: no change - no endpoint or `warnings` field change.
- **PgWire**: no change - SQL results simply stop reflecting stale or wrongly-evicted rows.
- **FFI**: no change - `RebuildReport`/`RebuildWarningRecord` shapes are untouched; `describe_consistency_warning`'s text is unchanged, only documented.
- **NoSQL HTTP**: no change - `get`/`scan` read the same index through the same path.
- **Cross-interface conformance plan**: the defect and fix live below `DoogatService`, so all six interfaces inherit the correction through one code path and no interface can drift. The CLI-driven e2e (`tests/e2e/index_eviction_identity.rs`) is the shared conformance vehicle: it commits a mismatched-id file by raw git, poisons it, and asserts the survivors through `ddb query` - the same index every transport reads. No new capability is introduced, so no `Guaranteed` capability matrix row changes.
