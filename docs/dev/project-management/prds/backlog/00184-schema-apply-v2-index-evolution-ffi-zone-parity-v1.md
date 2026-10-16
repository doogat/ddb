---
catchup: force
default_model: opus
model_tier_rationale: invented contract - new PlanOp variants and an FFI record field
---
catchup: force

# Declarative schema apply v2: index evolution and FFI zone parity

## Overview

### Problem Statement
Two schema/introspection completeness gaps were deferred from the 202606200035 batch:

- **Index evolution on existing types (00161 KNOWN + blind CRITICAL-2)**: declarative apply (PRD 00161) applies declared indexes at CREATE (inline `UNIQUE(...)`), but *evolving* an existing type's index/`unique_together` is unsupported — the differ records it as unsupported (`differ.rs:293-295` pushes into `plan.unsupported`; the `SCHEMA_UNSUPPORTED_CHANGE` code lives in `app_contract/error.rs` and is attached downstream in `service/schema_apply.rs` — never silently dropped) rather than migrating. So a downstream that changes a `unique_together` on an already-created type cannot converge via apply; it must hand-edit. The declarative-apply epic explicitly scoped full index diff/migrate to a follow-up (design §7).
- **FFI introspection omits column zone (00160 cross-interface parity)**: PRD 00160 added inline column `ZONE`, introspectable via GraphQL and the typedef YAML, but the FFI `list_type_schemas` schema-discovery records do NOT carry the column zone. This violates the 120% cross-interface parity posture: the zone capability is visible on some discovery surfaces but not FFI.

### Target Users
Downstream apps managing schemas declaratively (jink) that need to evolve indexes/uniqueness on existing types without hand-editing typedefs; FFI/mobile consumers (Swift/Kotlin) that introspect schema and need column zone parity with GraphQL/YAML.

### Success Metrics
- A desired-vs-live difference in a type's `unique_together` on an EXISTING type produces an applied migration (add/drop the unique index on the materialized table + update the typedef), not a `SCHEMA_UNSUPPORTED_CHANGE` warning; re-apply is an idempotent no-op. (`unique_together` is the only declarable uniqueness form; no other index concept exists in `TableSchema`/`PlanOp`.)
- The index migration is atomic and consistent with the 00161 apply contract (single git commit for the typedef change; the derived index rebuilt/altered on the materialized table; self-healing on reindex).
- FFI `list_type_schemas` records expose each column's `zone`, matching the GraphQL introspection + typedef YAML values; verified by an FFI test.
- No regression: existing 00161 apply tests + 00160 zone tests stay green; a NEW e2e asserts a successful `unique_together` migration on an existing type (grounded 2026-09-05: no such e2e exists today — `SCHEMA_UNSUPPORTED_CHANGE` has zero hits under `tests/`, and `tests/e2e/schema_apply.rs` never changes `unique_together` on an existing type).

## Functional Decomposition

### Capability: Index/uniqueness evolution
#### Feature: Diff unique_together on existing types
- **Description**: the differ computes add/drop operations for `unique_together` differences on an existing type instead of emitting `SCHEMA_UNSUPPORTED_CHANGE`. (Grounding 2026-08-14: `unique_together` is the ONLY declarable index/uniqueness form — `TableSchema` has no `indexes` field and `PlanOp` no index variant — so this feature is scoped to it exactly; no general index-declaration surface is invented.)
- **Inputs**: desired `SchemaDoc` `unique_together` declarations vs the live `TableSchema`.
- **Outputs**: ordered `PlanOp`s for unique-index add/drop (e.g. `AddUniqueConstraint`/`DropUniqueConstraint`).
- **Behavior**: extend `differ.rs` to detect `unique_together` deltas on existing types (today pushed into `plan.unsupported` at `differ.rs:293-295`) and emit the new op(s); reserve the `SCHEMA_UNSUPPORTED_CHANGE` warning only for genuinely-unrepresentable changes.

#### Feature: Apply index/uniqueness migration
- **Description**: the apply path executes the new index ops against the materialized table + updates the typedef.
- **Inputs**: the new `PlanOp`s.
- **Outputs**: the materialized table's unique index added/dropped; the typedef `unique_together` updated; an apply report entry.
- **Behavior**: render the op to the appropriate DDL/index operation (reuse the existing unique-index materialization), route the typedef write through the 00161 transactional apply so the plan stays single-commit atomic; idempotent on re-apply.

### Capability: Cross-interface introspection parity (zone)
#### Feature: Column zone in FFI schema discovery
- **Description**: FFI `list_type_schemas` records carry each column's `zone`.
- **Inputs**: the live `TableSchema` (which already holds `ColumnDef.zone`).
- **Outputs**: the FFI schema/column record with a `zone` field populated.
- **Behavior**: add `zone` to the FFI column record (`ddb-core/src/ffi/records.rs`) and populate it in the `list_type_schemas` mapping, matching GraphQL/YAML values; regenerate bindings.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/schema_diff/
├── differ.rs           # Maps to: Diff unique_together on existing types
├── plan.rs             # Maps to: Apply index/uniqueness migration (new PlanOp + render)
ddb-core/src/service/
└── schema_apply.rs     # Maps to: Apply index/uniqueness migration (verb wiring + report)
ddb-core/src/sql_engine/
└── ddl.rs              # Maps to: Apply index/uniqueness migration (unique-index add/drop on materialized table)
ddb-core/src/ffi/
└── records.rs          # Maps to: Column zone in FFI schema discovery
ddb-core/src/ffi/
└── driver.rs           # Maps to: Column zone in FFI schema discovery (list_type_schemas mapping)
```

### Module: schema_diff (ddb-core/src/schema_diff/)
- **Maps to capability**: Index/uniqueness evolution
- **Responsibility**: detect index/uniqueness deltas on existing types and render the migration DDL/index ops.
- **Exports**:
  - `differ::diff_type(...)` - now emits index add/drop ops
  - `PlanOp::{AddUniqueConstraint, DropUniqueConstraint}` (or equivalent) + `render_sql`

### Module: schema_apply (ddb-core/src/service/schema_apply.rs)
- **Maps to capability**: Index/uniqueness evolution
- **Responsibility**: execute the index migration ops under the 00161 transactional apply; report them.
- **Exports**: `apply_schema(...)` - handles the new ops

### Module: ffi (ddb-core/src/ffi/records.rs, driver.rs)
- **Maps to capability**: Cross-interface introspection parity (zone)
- **Responsibility**: expose `zone` on the FFI schema-discovery record.
- **Exports**: the column record type (now with `zone`); `list_type_schemas` populates it.

### Module: DDL execution and golden workflow (ddb-core/src/sql_engine/ddl.rs; tests/e2e/conformance/)
- **Maps to capability**: Index/uniqueness evolution; Cross-interface introspection parity (zone)
- **Responsibility**: execute constraint operations atomically without the ignored raw-index path and pin apply/discovery behavior.
- **Exports**: internal constraint operation execution and conformance fixtures.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **schema_diff (new index ops + differ delta)**: provides the plan-level representation of index evolution.

### Core Layer (Phase 1)
- **schema_apply + ddl**: Depends on [schema_diff] - execute the index migration under the transactional apply.

### Integration Layer (Phase 2)
- **ffi zone parity**: independent of the index work (separate capability) - may proceed in parallel; depends only on the existing `ColumnDef.zone`.
- **tests + docs + bindings**: Depends on [schema_apply, ffi].

## Implementation Phases

### Phase 0: Foundation
**Goal**: represent index evolution in the differ + plan.

**Tasks**:
- [ ] Add `unique_together` delta detection in `differ.rs` + the new `PlanOp`(s) with `render_sql` (scoped to `unique_together` — the only declarable uniqueness form; no new index-declaration surface) (no deps) - Acceptance: a `unique_together` change on an existing type produces an add/drop op (not `SCHEMA_UNSUPPORTED_CHANGE`); pure-differ unit tests cover add, drop, and no-op.

**Exit Criteria**: the differ plans index evolution; rendering unit-tested.

### Phase 1: Core
**Goal**: apply the migration atomically.

**Tasks**:
- [ ] Execute the new index ops in `schema_apply`/`ddl`, bypassing the ignored CREATE INDEX IF NOT EXISTS path (drop+create the unique index on the materialized table; update the typedef) under the 00161 transactional apply (depends on: Phase 0) - Acceptance: applying a changed `unique_together` migrates the live type; re-apply is an idempotent no-op; the apply stays single-commit atomic; a new e2e (authored in Phase 2) asserts success where today's code emits `SCHEMA_UNSUPPORTED_CHANGE`.

**Exit Criteria**: index evolution applies and converges.

### Phase 2: Integration
**Goal**: FFI zone parity + docs + bindings + conformance.

**Tasks**:
- [ ] Add `zone` to the FFI column record + populate in `list_type_schemas`; regenerate Swift/Kotlin bindings (no deps on index work) - Acceptance: an FFI test asserts the column zone matches GraphQL/YAML; bindings regenerate clean.
- [ ] Add the index-evolution golden workflow on CLI, GraphQL, and REST (00175 driver), plus FFI unit coverage; update `docs/src/technical/schema-apply.md` (index evolution now supported) + `ffi.md` (zone in discovery) + CHANGELOG; add e2e scenarios for index evolution to the Rust harness (`tests/e2e/`; the shell harness was removed by 00171) (depends on: Phase 1) - Acceptance: conformance asserts add/drop/reapply outcomes on CLI/GraphQL/REST and FFI, docs match, and e2e scenarios are authored for CI.

**Exit Criteria**: FFI parity + docs + scenarios landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: apply a desired schema that adds a `unique_together` to an existing type → Expected: the unique index is created on the materialized table; duplicate rows now rejected; report lists the op.
- **Edge case**: re-apply the same desired schema → Expected: empty plan, `applied=false` (idempotent).
- **Error case**: drop a `unique_together` whose removal would orphan nothing → Expected: index dropped; an FFI `list_type_schemas` call returns the column zone matching GraphQL.

## Risks

- **Index migration on data with existing duplicates**: adding a `unique_together` to a type that already holds duplicate rows must fail clearly (not silently). Mitigation: surface a structured error (reuse the unique-violation path); document; test the duplicate-data case.
- **Atomicity with the derived index**: the typedef write is git-atomic but the materialized index is derived. Mitigation: follow the 00161 contract (single commit for the typedef; index rebuilt on apply; self-heals on reindex); test the post-commit consistency.
- **FFI record shape change**: adding `zone` changes a uniffi record (regenerates bindings). Mitigation: additive optional field; regenerate + smoke the Swift/Kotlin surface; CHANGELOG note.
- **Scope**: keep to `unique_together` (the only declarable uniqueness form) — general secondary-index DDL remains out of scope (Doogat DB has none); document the boundary.

## Transport Impact

- **CLI**: Guaranteed — `ddb schema apply`/`diff` now plans + applies index evolution on existing types.
- **GraphQL**: Guaranteed — `applySchema` applies index evolution; report includes the new ops.
- **REST**: Guaranteed — `POST /rest/schema/apply` applies index evolution.
- **FFI**: Guaranteed — `apply_schema` applies index evolution AND `list_type_schemas` now exposes column `zone` (the parity fix).
- **PgWire**: Specialized — declarative schema apply has no SQL surface. Ordinary CREATE/DROP INDEX are unsupported and CREATE INDEX IF NOT EXISTS currently does nothing; do not claim raw DDL already provides index evolution or route the new PlanOps into that no-op path.
- **NoSQL HTTP**: Intentionally absent — read-only document surface; schema management out of its workflow (consistent with 00161).
- **Cross-interface conformance plan**: the conformance harness has no schema-apply workflow today (3 fixtures, CLI + GraphQL drivers — grounded 2026-09-05), so this PRD adds an index-evolution apply workflow run on CLI + GraphQL (+ REST via the driver 00175 lands); the FFI leg is pinned by an FFI unit test (the harness FFI driver arrives later, in 00193), exactly as the zone parity is; e2e scenarios cover the apply path.
