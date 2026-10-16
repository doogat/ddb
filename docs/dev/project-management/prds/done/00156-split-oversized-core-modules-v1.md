# Split oversized core modules (autopilot enabler for PRD 00149)

## Overview

### Problem Statement

Four production modules on PRD 00149's edit path exceed the project's
800-line file cap (measured 2026-06-09):

- `ddb-core/src/service/crud.rs` — 1845 lines
- `ddb-core/src/indexer/mod.rs` — 1065 lines
- `ddb-core/src/ffi.rs` — 1060 lines
- `ddb-server/src/schema/mutations.rs` — 825 lines (flagged as
  pre-existing oversize during PRD 00154 review)

Beyond the style rule, this is a documented autopilot blocker: refactor
tasks that must read crud.rs or indexer/mod.rs cannot fit the 40K-token
replan floor, and PRD 00142 stalled on exactly this before being finished
manually at full context (memory `project_large_files_block_replan`). PRD
00149 (transport-adapter thinning — the architecture program's last PRD)
routes more workflows through `service/crud.rs`, thins resolvers in
`schema/mutations.rs`, and thins `ffi.rs`. Without this split it will hit
the same stall.

(`sql_engine/dml.rs` 1663, `sql_engine/ddl.rs` 1511, and
`indexer/materialize.rs` 1277 also exceed the cap but are not on 00149's
path; they are explicitly out of scope — see Risks.)

### Target Users

- The maintainer's autopilot loop: PRDs touching these files must plan
  within the replan budget instead of stalling as `oversized_task`.
- Future reviewers/agents: 200-400-line files matching the repo's own
  cohesion rule.

### Success Metrics

- No production file touched by this PRD exceeds 800 lines afterward;
  new files land in the 200-600 range.
- Zero behavior change: full Tier 1 gate plus `cargo test -p ddb-e2e`
  green at every commit; no public API signature changes.
- PRD 00149's planning can cite per-file reads under ~15K tokens for any
  single service/transport unit.

## Functional Decomposition

### Capability: Service write-path split

`service/crud.rs` becomes responsibility-sized files. The service/ folder
already uses one-impl-block-per-file (`ops.rs`, `utility.rs`,
`validation.rs`, `discovery.rs`, `search.rs`, `sql.rs`), so this extends
the existing pattern.

#### Feature: crud.rs decomposition

- **Description**: Move crud.rs's impl blocks into per-verb files.
- **Inputs**: `service/crud.rs` (create paths + app facade at ~150-220,
  upsert_singleton at ~340, update paths at ~610/698, batch create at
  ~833-878, raw paths at ~1316+).
- **Outputs**: `service/create.rs`, `service/update.rs`,
  `service/delete.rs` (or the seams Phase 0 confirms), each its own
  `impl DoogatService` block; `crud.rs` removed or reduced to a stub
  module comment.
- **Behavior**: pure moves — function bodies, doc comments, and
  `pub`/`pub(crate)` visibility move verbatim; only `use` lines and module
  wiring change.

### Capability: Indexer facade split

#### Feature: indexer/mod.rs decomposition

- **Description**: Move port impls and helper clusters out of the facade.
- **Inputs**: `indexer/mod.rs` (1065 lines: Index struct + connection
  management, typedef loading, port impls from PRD 00142
  (`IndexPort`/`TypedMaterializationPort`/`SqlBackend`/`DoogatIndex`
  pass-throughs), misc query helpers).
- **Outputs**: port impls and cohesive helper clusters moved to new
  submodules (e.g. `indexer/ports.rs`, `indexer/typedefs.rs`); `mod.rs`
  keeps the struct, constructor, and re-exports.
- **Behavior**: pure moves; trait impls stay in the same crate and module
  tree, so no coherence changes.

### Capability: Transport-file splits

#### Feature: mutations.rs decomposition

- **Description**: Move mutation field builders into smaller sibling files.
- **Inputs**: `ddb-server/src/schema/mutations.rs` (825 lines).
- **Outputs**: `schema/mutations/` directory (e.g. `crud.rs`, `sql.rs`,
  `batch.rs`) or two sibling files; field-builder functions move whole.
- **Behavior**: pure moves; GraphQL schema output is byte-identical
  (pinned by existing schema tests).

#### Feature: ffi.rs decomposition

- **Description**: Move the FFI facade into a module directory behind a
  re-exporting `ffi/mod.rs`.
- **Inputs**: `ddb-core/src/ffi.rs` (1060 lines, uniffi proc-macro
  exports).
- **Outputs**: `ffi/` module directory with `ffi/mod.rs` re-exporting the
  existing public surface so `uniffi` scaffolding and `ddb.udl`
  documentation stay aligned.
- **Behavior**: pure moves; binding regeneration must produce an
  equivalent Swift/Kotlin surface (see Test Strategy).

## Structural Decomposition

### Repository Structure

```
ddb-core/src/
├── service/
│   ├── create.rs        # Maps to: crud.rs decomposition (create + facade)
│   ├── update.rs        # Maps to: crud.rs decomposition (update/batch-update)
│   ├── delete.rs        # Maps to: crud.rs decomposition (delete/raw)
│   └── mod.rs           # wiring only
├── indexer/
│   ├── ports.rs         # Maps to: indexer/mod.rs decomposition (PRD 00142 port impls)
│   ├── typedefs.rs      # Maps to: indexer/mod.rs decomposition (schema loading)
│   └── mod.rs           # struct + constructor + re-exports
└── ffi/
    └── mod.rs (+ split files)   # Maps to: ffi.rs decomposition
ddb-server/src/schema/
└── mutations/ (or sibling files)  # Maps to: mutations.rs decomposition
```

Exact file names/seams are confirmed in Phase 0; the tree above is the
expected shape, not a mandate.

### Module: service write-path files

- **Maps to capability**: Service write-path split
- **Responsibility**: one file per CRUD verb family
- **Exports**: existing `DoogatService` methods, unchanged signatures

### Module: indexer submodules

- **Maps to capability**: Indexer facade split
- **Responsibility**: port impls and typedef loading out of the facade
- **Exports**: existing `Index` API, unchanged

### Module: transport splits (mutations, ffi)

- **Maps to capability**: Transport-file splits
- **Responsibility**: same public surface from smaller files
- **Exports**: `build_schema` inputs unchanged; FFI `DoogatDriver` surface
  unchanged

## Dependency Graph

### Foundation Layer (Phase 0)

No dependencies - built first.

- **Seam analysis**: per-file move plan (function → target file), no code
  changes.

### Core Layer (Phase 1)

- **service/crud split**: Depends on [seam analysis].
- **indexer/mod split**: Depends on [seam analysis]; independent of the
  crud split.

### Integration Layer (Phase 2)

- **mutations.rs split**: Depends on [nothing in core layer; ordered after
  to keep churn serialized].
- **ffi.rs split**: Depends on [service split] only insofar as imports
  move; binding check runs here.

## Implementation Phases

Execution note: this PRD itself touches the files that stall autopilot
replanning. Tasks are sliced one-target-file-per-task with pure-move
discipline so each task reads only its source and target. If autopilot
still stalls, run manually at full context (PRD 00142 precedent).

### Phase 0: Foundation

**Goal**: A move plan that makes every later task mechanical.

**Tasks**:

- [ ] Map every item in crud.rs / indexer/mod.rs / mutations.rs / ffi.rs
  to a target file (no deps) - Acceptance: table in `dev/local/plans/`
  listing item → destination, with no item unassigned and no destination
  projected above 800 lines.

**Exit Criteria**: Move plan reviewed against the 800-line projection.

### Phase 1: Core

**Goal**: ddb-core splits landed.

**Tasks**:

- [ ] Split service/crud.rs per plan (depends on: Phase 0) - Acceptance:
  Tier 1 + `cargo test -p ddb-e2e` green; `service/*.rs` all ≤ 800 lines;
  diff is move-only (verified by reviewing that no function body hunks
  changed).
- [ ] Split indexer/mod.rs per plan (depends on: Phase 0) - Acceptance:
  same gates; port impls relocated without trait-coherence changes.

**Exit Criteria**: ddb-core file-size rule satisfied on touched files.

### Phase 2: Integration

**Goal**: Server and FFI splits landed; bindings verified.

**Tasks**:

- [ ] Split schema/mutations.rs per plan (depends on: Phase 1) -
  Acceptance: Tier 1 green; schema tests unchanged and green.
- [ ] Split ffi.rs into ffi/ per plan (depends on: Phase 1) - Acceptance:
  Tier 1 + e2e green; uniffi binding generation (swift + kotlin) succeeds
  and the generated API surface matches pre-split output.
- [ ] Update `AGENTS.md` structure block and
  `docs/src/technical/walkthrough.md` for the new module boundaries
  (depends on: all splits) - Acceptance: docs name the new files.

**Exit Criteria**: All four target files retired or ≤ 800 lines; docs
match.

## Test Strategy

### Critical Scenarios

- **Happy path**: full Tier 1 (`cargo build`, `clippy --workspace`,
  `cargo test-ci`) after every split commit → Expected: green with zero
  test edits (tests import via `service::`/`indexer::` paths that
  re-exports preserve).
- **Edge case**: uniffi binding regeneration after the ffi split →
  Expected: Swift/Kotlin surface equivalent to pre-split (diff of
  generated files shows naming/ordering changes only, no API changes).
- **Error case**: a move that accidentally narrows visibility → Expected:
  caught at compile time by Tier 1; no `pub` widening allowed as a "fix"
  without a note.

## Risks

- **Hidden behavior change smuggled into a "move"**: Mitigation: per-task
  rule — no logic edits, no comment rewrites, no renames; anything found
  broken during the move is recorded as a follow-up, not fixed inline
  (surgical-changes discipline).
- **uniffi is sensitive to item paths**: proc-macro exports may encode
  module paths into scaffolding. Mitigation: keep a re-exporting
  `ffi/mod.rs` facade; binding-equivalence check is a hard acceptance
  gate; if equivalence cannot be held, drop the ffi split task and record
  why (the other three splits do not depend on it).
- **Scope creep into sql_engine**: dml.rs/ddl.rs/materialize.rs are also
  oversized but not on 00149's path. Mitigation: explicitly out of scope;
  revisit only when a PRD needs to touch them.
- **Merge friction with PRD 00149**: both touch the same files.
  Mitigation: land this PRD entirely before starting 00149 (00149's
  Status section carries the matching soft-dep note).
- **Churn overlap with PRD 00155**: 00155 edits the crud.rs create
  facade. Mitigation: run 00155 first (it is small and urgent — nightly
  is red); this PRD then moves the already-fixed code.

## Transport Impact

Pure structural refactor: no wire, CLI, or FFI behavior change on any of
CLI, GraphQL, REST, PgWire, FFI, NoSQL HTTP. Proof is the unchanged test
suite (Tier 1 + e2e + conformance harness) and the FFI
binding-equivalence check; no conformance fixtures change. Any observed
behavior delta is a defect of this PRD by definition.
