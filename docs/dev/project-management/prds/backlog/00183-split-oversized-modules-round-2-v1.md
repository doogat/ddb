---
default_model: sonnet
model_tier_rationale: pure-move splits with the existing suites as the oracle; deliberate deviation from the equivalence row, the classifier still raises any contract_edit
---

# Split oversized server/core modules (round 2)

> **Provenance:** the extended 14-module split list and the `sql_conn`-exception decision were added by the 2026-07-16 assessment (F15 + lens-6 F4) and folded into the body's Features/tasks at the 2026-08-14 backlog review. Re-measured 2026-09-05 at `8fe51c9`: 13 modules remain >800 prod-only lines (`sync_manager/mod.rs` dropped to 774 and leaves the list; no unlisted production file exceeds 800); the `sql_conn` transitional exception sits at `ddb-core/src/traits.rs:68-77`. The sweep is grouped into three tasks so the plan stays under the 15-task loop ceiling (plan-tasks 5.5; 2026-09-05 backlog review).

## Overview

### Problem Statement
Thirteen production modules exceed the 800-line file guideline (rubric R13) and one function is past the 50-line guideline. Three were flagged repeatedly across the 202606200035 batch (00158/00159/00160/00161 reviews) and deferred each time as out-of-scope-for-a-feature-PRD per surgical-changes; the 2026-07-16 assessment extended the list to 14, re-measured to 13 on 2026-09-05 (see the Round-2 sweep capability). PRD 00156 established the pattern (pure-move splits of `crud.rs`, `indexer/mod.rs`, `ffi.rs`, `mutations.rs`). The three original flags:

- `ddb-server/src/filter.rs` — ~3397 lines (the where-compiler; large even excluding its colocated `#[cfg(test)]` module).
- `ddb-server/src/schema/queries.rs` — contains `build_query_fields`, an ~842-line function.
- `ddb-core/src/sql_engine/helpers.rs` — ~1544 lines (regex helpers, the `strip_inline_zones` family, plus a large colocated test module). (Its 3 deferred `unnecessary_get_then_check` clippy lints were cleared on 2026-07-16, commit `8cfe90a` — no lint debt remains here; the split is now purely a size concern.)

Oversized files raise the cost of every future read/edit and slow review; the historical helpers.rs lint debt is already cleared.

### Target Users
ddb maintainers and review agents: smaller, focused modules are faster to read, review, and modify.

### Success Metrics
- Every resulting production file is <= 800 lines; `build_query_fields` is decomposed so no single function exceeds the 50-line guideline (or is justified inline).
- Splits are pure moves / mechanical extractions: zero behavior change, no public-item additions or losses, byte-equivalent logic (verified the 00156 way — diff the moved bodies).
- `cargo clippy --workspace --all-targets` clean after every split (the 3 historical `helpers.rs` lints were already cleared in `8cfe90a`; the gate is that the splits introduce none).
- `cargo test-ci` + `cargo test -p ddb-e2e` stay green; no test asserts on a moved private path that breaks.
- The SqlBackend::sql_conn exception has an explicit disposition: remove only within the approved split scope, otherwise retain it with a caller inventory and an audit gap for later evolution assessment.

## Functional Decomposition

### Capability: Where-compiler module split (filter.rs)
#### Feature: Extract filter.rs into cohesive submodules
- **Description**: split `ddb-server/src/filter.rs` into a `filter/` module directory by concern (e.g. core where-clause building, tag/junction handling, reverse-relation discovery, the colocated tests).
- **Inputs**: the current `filter.rs` (production + test module).
- **Outputs**: a `filter/` module with each file <= 800 lines and `filter/mod.rs` re-exporting the existing public surface unchanged.
- **Behavior**: move code verbatim; preserve every `pub`/`pub(crate)` item and its path (re-export so external callers and tests are unaffected); move the test module alongside or into a `tests` submodule.

### Capability: Query-field builder decomposition (queries.rs)
#### Feature: Decompose build_query_fields
- **Description**: break the ~842-line `build_query_fields` into focused helper functions and, if warranted, split `queries.rs`.
- **Inputs**: the current `build_query_fields` + `queries.rs`.
- **Outputs**: a top-level builder under 50 lines that composes named helpers; resulting files <= 800 lines.
- **Behavior**: extract per-field-group helpers with no logic change; the generated GraphQL schema is byte-identical.

### Capability: SQL-helpers module split (helpers.rs)
#### Feature: Extract helpers.rs into submodules
- **Description**: split `ddb-core/src/sql_engine/helpers.rs` (e.g. regex helpers, the `strip_inline_zones` family, the test module). Its 3 historical `unnecessary_get_then_check` lints were already cleared in `8cfe90a` — the split is purely a size concern.
- **Inputs**: the current `helpers.rs`.
- **Outputs**: submodules each <= 800 lines; clippy clean (no new lints introduced).
- **Behavior**: move verbatim; no behavior change.

### Capability: Round-2 sweep of the remaining oversized modules
#### Feature: Pure-move splits for the 10 additional >800-line modules
- **Description**: extend the split to every remaining production module over the 800-line cap (prod-only counts at `8fe51c9`, colocated tests excluded): `sql_engine/dml.rs` 1663, `sql_engine/ddl.rs` 1654, `indexer/materialize.rs` 1309, `schema/base_types.rs` 1113, `indexer/graph.rs` 1020, `parser/mod.rs` 978, `sql_engine/builders.rs` 916, `crdt_resolver.rs` 871, `git_ops/mod.rs` 851, `indexer/search.rs` 812 (`sync_manager/mod.rs` is 774 and no longer qualifies).
- **Inputs**: each listed module; current line counts re-measured at execution.
- **Outputs**: a module directory (or extracted siblings) per split with `mod.rs` re-exporting the prior public surface; every resulting production file <= 800 lines.
- **Behavior**: pure moves only, grouped into THREE tasks of 3-4 modules so the plan stays under the 15-task loop ceiling (plan-tasks 5.5): group A `dml.rs` + `ddl.rs` + `materialize.rs` (the contract-block collaterals, first — 00196 depends on the `dml.rs` split landing here), group B `base_types.rs` + `graph.rs` + `parser/mod.rs`, group C `builders.rs` + `crdt_resolver.rs` + `git_ops/mod.rs` + `search.rs`; split-before-refactor lesson from 00156 applies; each group's task is independently verifiable by its crate's tests.

#### Feature: sql_conn transitional-exception decision
- **Description**: inventory the SqlBackend::sql_conn exception and record whether removal fits the approved module-split scope.
- **Inputs**: the exception doc comment + current `sql_conn` callers.
- **Outputs**: a removal or explicit retained-exception rationale in traits.rs and design-decisions.md, with a remaining exit gap recorded in this backlog review for the next evolution assessment.
- **Behavior**: remove only if the caller inventory permits a scope-preserving change; otherwise retain the exception and record affected callers, boundary debt, and follow-up ownership by the next evolution assessment. Do not invent a successor PRD.

## Structural Decomposition

### Repository Structure

```
ddb-server/src/
├── filter/                 # Maps to: Where-compiler module split
│   ├── mod.rs              #   re-exports the prior filter.rs public surface
│   ├── where_build.rs      #   core where-clause building
│   ├── junction.rs         #   tag/junction/EXISTS handling
│   ├── reverse.rs          #   reverse-relation discovery
│   └── tests.rs            #   the colocated test module
├── schema/
│   └── queries.rs          # Maps to: Query-field builder decomposition (build_query_fields -> helpers)
ddb-core/src/sql_engine/
├── helpers/                # Maps to: SQL-helpers module split
│   ├── mod.rs              #   re-exports the prior helpers.rs surface
│   ├── regexes.rs          #   the cached re_* helpers
│   ├── inline_zone.rs      #   strip_inline_zones family
│   └── tests.rs            #   the colocated test module
(+ per-module split dirs)   # Maps to: Round-2 sweep — dml/, ddl/, materialize/, base_types/, graph/,
                            #   parser/, builders/, crdt_resolver/, git_ops/, search/
                            #   (same mod.rs re-export shape; exact submodule names decided per split)
ddb-core/src/
└── traits.rs               # Maps to: sql_conn transitional-exception decision (:68-77)
```

### Module: filter (ddb-server/src/filter/)
- **Maps to capability**: Where-compiler module split (filter.rs)
- **Responsibility**: compile GraphQL where-input into parameterized SQL; preserve the exact public surface via `mod.rs` re-exports.
- **Exports**: the same items `filter.rs` exported today (e.g. `pub fn try_build_where_sql` at `:625`, `pub(crate) struct WhereCtx` at `:579`, ...), re-exported unchanged at their current visibility.

### Module: queries (ddb-server/src/schema/queries.rs)
- **Maps to capability**: Query-field builder decomposition (queries.rs)
- **Responsibility**: build the dynamic GraphQL query fields; `build_query_fields` becomes a thin composer over named helpers.
- **Exports**: `build_query_fields(...)` - unchanged signature, decomposed body.

### Module: helpers (ddb-core/src/sql_engine/helpers/)
- **Maps to capability**: SQL-helpers module split (helpers.rs)
- **Responsibility**: SQL pre-parse helpers (regexes, inline-zone stripping); re-export the prior surface via `mod.rs`.
- **Exports**: the same `pub(super)`/`pub(crate)` helpers `helpers.rs` exposed, re-exported unchanged.

### Module: grouped sweep and exception (ddb-core/src/sql_engine/dml.rs, ddl.rs, builders.rs; indexer/materialize.rs, graph.rs, search.rs; parser/mod.rs, crdt_resolver.rs, git_ops/mod.rs, traits.rs; ddb-server/src/schema/base_types.rs)
- **Maps to capability**: Round-2 sweep of the remaining oversized modules
- **Responsibility**: perform the three bounded groups of pure moves, re-export existing APIs, and record the sql_conn exception disposition.
- **Exports**: existing module APIs only; no new product capability.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first. Each split is independent of the others (different crates/files).

- **filter split**: provides the `filter/` module.
- **helpers split**: provides the `helpers/` module.
- **queries decomposition**: provides the decomposed builder.
- **round-2 sweep**: 10 pure-move splits in three grouped tasks (A: dml/ddl/materialize; B: base_types/graph/parser; C: builders/crdt_resolver/git_ops/search).

### Core Layer (Phase 1)
- No cross-dependencies between the three splits; they may proceed in parallel. Each depends only on the workspace compiling.

### Integration Layer (Phase 2)
- **verification**: Depends on [all splits] - full Tier-1 gate + e2e confirm zero behavior change.
- **sql_conn decision**: Depends on [the sql_engine splits] - decided from the post-split caller inventory.

## Implementation Phases

### Phase 0: Foundation
**Goal**: split each oversized module as a pure move, independently.

**Tasks**:
- [ ] Split `filter.rs` into `filter/` submodules with `mod.rs` re-exports (no deps) - Acceptance: every `filter/*.rs` <= 800 lines; public surface unchanged; `cargo build` + the 00159 where-filter tests green.
- [ ] Split `helpers.rs` into `helpers/` submodules (no deps) - Acceptance: every `helpers/*.rs` <= 800 lines; `cargo clippy --workspace --all-targets` clean; sql_engine tests green.
- [ ] Decompose `build_query_fields` into named helpers (no deps) - Acceptance: top-level builder <= 50 lines; generated schema unchanged (schema-snapshot/integration tests green).
- [ ] Sweep group A — pure-move splits of `sql_engine/dml.rs`, `sql_engine/ddl.rs`, `indexer/materialize.rs` (the contract-block collaterals; 00196 needs `dml/`) (no deps) - Acceptance: per module, every resulting production file <= 800 lines, the prior public surface re-exported unchanged, and `cargo test-ci` green; counts re-measured at execution before splitting.
- [ ] Sweep group B — pure-move splits of `ddb-server/src/schema/base_types.rs`, `indexer/graph.rs`, `parser/mod.rs` (no deps) - Acceptance: same per-module bar; the applicable fast tests green through cargo test-ci.
- [ ] Sweep group C — pure-move splits of `sql_engine/builders.rs`, `crdt_resolver.rs`, `git_ops/mod.rs`, `indexer/search.rs` (no deps) - Acceptance: same per-module bar; `cargo test-ci` green. (`sync_manager/mod.rs` is 774 prod lines at `8fe51c9` — re-measure; split only if it crossed 800 again.)

**Exit Criteria**: all targeted files within guidelines; workspace builds.

### Phase 1: Core
**Goal**: confirm behavior preservation per split.

**Tasks**:
- [ ] Per-split behavior check: diff moved bodies; run the applicable fast tests through cargo test-ci (depends on: Phase 0) - Acceptance: moved logic byte-equivalent; no public item added/lost.

**Exit Criteria**: each split verified behavior-preserving.

### Phase 2: Integration
**Goal**: full-suite confirmation + docs.

**Tasks**:
- [ ] Full Tier-1 gate + e2e (deletion/replace safety net) (depends on: Phase 1) - Acceptance: `cargo test-ci` + `cargo test -p ddb-e2e` green; clippy clean.
- [ ] Inventory SqlBackend::sql_conn callers after the splits; remove the exception only within the approved scope, otherwise retain it with explicit rationale and the deferred exit in the backlog-review gap ledger (depends on: Phase 1) - Acceptance: traits.rs and design-decisions.md state the actual disposition and remaining callers; no unrelated refactor or unnamed successor is smuggled into the split.
- [ ] Update `docs/src/technical/walkthrough.md` / module docs if module boundaries are referenced (depends on: Phase 1) - Acceptance: docs reflect the new module layout.

**Exit Criteria**: green suite; docs aligned.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: existing where-filter / query / sql-engine tests pass unchanged against the split modules → Expected: green, no edits beyond import paths.
- **Edge case**: a test that referenced a moved item compiles via the `mod.rs` re-export → Expected: no broken path.
- **Error case**: clippy (`--all-targets`) run after the helpers split → Expected: clean, no new warnings.

## Risks

- **Pure-move regressions hidden in path changes**: re-exports must preserve every public/`pub(crate)` path. Mitigation: `mod.rs` re-exports the prior surface; verify with `cargo build` across the workspace + the 00156-style moved-body diff.
- **build_query_fields decomposition altering schema**: extracting helpers could reorder field registration. Mitigation: preserve registration order; rely on schema-snapshot/integration tests; keep the change mechanical.
- **Scope creep into behavior changes**: refactor only. Mitigation: no logic edits anywhere — the historical helpers.rs lints were already cleared in `8cfe90a`; anything else stays deferred.
- **Sweep size**: 13 modules is a large PRD. Mitigation: every split is a mechanical pure move; the sweep is grouped into three tasks (10 tasks total, under the 15-task loop ceiling); each task is independently verifiable by its crate's tests, so a partial batch still leaves the tree healthy.

## Transport Impact

- This is an internal, behavior-preserving refactor. No public application capability changes on any interface (CLI, GraphQL, REST, PgWire, FFI, NoSQL HTTP); the GraphQL schema output and all error/response shapes are byte-identical.
- **Cross-interface conformance plan**: the existing per-interface suites (`cargo test-ci`, `tests/e2e`, the GraphQL integration tests) are the conformance gate — they must stay green with no assertion changes beyond import paths, proving the refactor is transport-neutral.
