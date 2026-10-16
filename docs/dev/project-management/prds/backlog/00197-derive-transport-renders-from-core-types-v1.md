---
catchup: force
default_model: opus
model_tier_rationale: cross-cutting shape change - one SqlResult wire shape across four surfaces plus an FFI semantic change; catchup forced because 00183 splits base_types.rs
---

# Derive transport renders from core result types

**Priority:** P2 (leverage; kills the field-drop drift class) · **Roadmap:** appended 2026-07-16 · **Depends on:** 00172 (closure actor), 00176 (one core result shape for search — this PRD generalizes the rule).

Source: `dev/local/audit-results/evolution-assessment-2026-07-16.md` F9 + lens-4 marginal-cost analysis.

## Overview

### Problem Statement

Transports hand-roll result rendering, so the same core type ships different field sets with contradictory semantics:

- `SqlResult` (`sql_engine/mod.rs:36-46`) has 4 renderers: PgWire alone uses `column_types` (`pgwire.rs:127-128`); GraphQL drops it (`base_types.rs:273`); FFI drops it AND fabricates `affected_rows = rows.len()` for SELECT (`ffi/records.rs:262-263`; field declared `:255`) where GraphQL returns `affected: null` (`base_types.rs:298`) — same result, contradictory semantics; CLI drops column headers entirely (`ddb-cli/src/commands/query.rs:29-32`).
- Within ONE transport: `SchemaApplyReport` is field-listed twice in GraphQL alone (type builder `schema/mutations/types.rs:157-170` + value serializer `schema/mutations/operations.rs:526-551`) even though the core type derives `serde::Serialize` (`schema_diff/plan.rs:237`) and REST serializes it directly (`rest.rs:89-93`).

Any new field on a core result type = 4-6 hand edits, and forgetting one is silent — the exact class coherence invariant I8 names.

### Target Users

Platform maintainers; typed clients (mobile FFI cannot type SQL rows today); downstreams porting a query between transports.

### Success Metrics

- A shared serde-to-GraphQL bridge removes duplicate value serialization while retaining dynamic field declarations and explicit schema-name/scalar adaptation.
- `SqlResult` has ONE canonical wire shape in core; all four surfaces derive from it (documented per-surface adaptations allowed — e.g. CLI plain-text — but declared, not accidental).
- FFI stops fabricating `affected_rows` for SELECT (semantic drift fixed, compatibility-reviewed).
- A cross-transport SQL conformance fixture pins the shape.

## Functional Decomposition

### Capability: serde→GraphQL render bridge

#### Feature: Generic value bridge
- **Description**: one helper converting any `serde::Serialize` core value into an `async_graphql::Value` for dynamic-schema output objects.
- **Inputs**: a `Serialize` core result (`SchemaApplyReport` derives it today, `plan.rs:237`; `SyncReport` and `CompactionReport` derive only `Debug, Clone` — `types/doogat.rs:216,:315`, grounded 2026-09-05 — so this PRD adds `serde::Serialize` to both before their renders adopt the bridge).
- **Outputs**: the `async_graphql::Value` tree the resolver returns.
- **Behavior**: serialize the canonical core value, then apply declared schema-aware field/scalar adaptation at the GraphQL boundary. Preserve dry_run → dryRun and existing decimal-string encoding for compaction u64 fields; do not globally rename core serde fields or change REST JSON. Serialization/adaptation failures return structured errors, never panic. Compare actual served responses with existing fixtures.

#### Feature: Report-render adoption
- **Description**: report-style GraphQL value serializers adopt the bridge, deleting the hand-listed twins.
- **Inputs**: the bridge; the existing type builders (field DECLARATIONS stay — the bridge kills value-serializer duplication, not the dynamic type registration).
- **Outputs**: byte-equivalent GraphQL responses with the hand-rolled serializers deleted.
- **Behavior**: adopt for `SchemaApplyReport` first (`operations.rs:526-551`), then `SyncReport`/`CompactionReport` renders; each adoption pinned by a response-equality test.

### Capability: Canonical SqlResult wire shape

#### Feature: One core SQL wire shape
- **Description**: core defines the serialized `SqlResult` shape once: `columns: Vec<String>`, `rows: Vec<Vec<String>>`, `column_types: Option<Vec<String>>`, `affected: Option<u64>`, `message: Option<String>`.
- **Inputs**: the existing `SqlResult` enum.
- **Outputs**: a core-owned serializable wire struct (or serde impl) all transports consume.
- **Behavior**: Rows preserves columns/rows/optional column_types with affected=None and message=None; Affected(n) has empty columns/rows, column_types=None, affected=Some(n), message=None; Ok(text) has empty columns/rows, column_types=None, affected=None, message=Some(text). Use checked count conversions; adapters retain every meaningful field.

#### Feature: Per-transport adoption
- **Description**: GraphQL/CLI/FFI adopt the canonical shape; PgWire keeps `column_types` (already the richest consumer).
- **Inputs**: the wire shape.
- **Outputs**: GraphQL gains `column_types`; CLI gains column headers; FFI record gains `column_types` and its `affected_rows` becomes an honest Option (direct beta signature replacement — documented and compatibility-reviewed); bindings regenerated.
- **Behavior**: each adapter maps the wire shape; per-surface presentation adaptations (CLI text formatting) are declared in the docs, not accidental subsets.

## Structural Decomposition

### Repository Structure

```text
ddb-server/src/schema/
├── render.rs (new)          # Maps to: Generic value bridge
├── mutations/operations.rs  # Maps to: Report-render adoption (twin serializers deleted; :526-551 plus the SyncReport/CompactionReport renders at :349/:353/:436/:443)
└── base_types.rs            # Maps to: Per-transport adoption (SqlResult GraphQL render)
ddb-core/src/sql_engine/
└── mod.rs                   # Maps to: One core SQL wire shape
ddb-core/src/ffi/
└── records.rs               # Maps to: Per-transport adoption (FFI SqlResult record)
ddb-cli/src/commands/
└── query.rs                 # Maps to: Per-transport adoption (CLI headers)
ddb-server/src/
└── pgwire.rs                # Maps to: Per-transport adoption (verify no change needed)
```

### Module: render (ddb-server/src/schema/render.rs, new)
- **Maps to capability**: serde→GraphQL render bridge
- **Responsibility**: shared fallible serde-to-GqlValue conversion plus explicit schema-aware name/scalar adaptation; no duplicate business-policy or core result definitions.
- **Exports**: a fallible to_gql_value helper with declared GraphQL schema adaptation; its private signature is settled in design.

### Module: sql_engine (ddb-core/src/sql_engine/mod.rs)
- **Maps to capability**: Canonical SqlResult wire shape
- **Responsibility**: own the canonical serialized shape of `SqlResult`.
- **Exports**: the wire struct/serde impl (`columns: Vec<String>`, `rows: Vec<Vec<String>>`, `column_types: Option<Vec<String>>`, `affected: Option<u64>`, `message: Option<String>`).

### Module: adapters (ffi/records.rs, ddb-cli/commands/query.rs, schema/base_types.rs)
- **Maps to capability**: Canonical SqlResult wire shape
- **Responsibility**: mechanical mapping from the wire shape; declared presentation adaptations only.
- **Exports**: existing surfaces, field-complete.

### Module: core reports and GraphQL registration (ddb-core/src/types/doogat.rs; ddb-server/src/schema/mutations/operations.rs, mutations/types.rs, pgwire.rs)
- **Maps to capability**: serde→GraphQL render bridge; Canonical SqlResult wire shape
- **Responsibility**: derive core serialization, retain dynamic field declarations, declare existing GraphQL name/scalar adaptations, and preserve SQL protocol presentation.
- **Exports**: Serialize core reports and complete existing transport outputs.

### Module: capability publication (ddb-server/src/capabilities.rs)
- **Maps to capability**: Canonical SqlResult wire shape
- **Responsibility**: publish canonical_sql_results from the existing 00186 registry and verify its declared interface/build scope.
- **Exports**: the registry entry and matching handshake fixtures.

## Dependency Graph

### Foundation Layer (Phase 0)
Depends on 00172 (resolvers on the closure actor) + 00176 (the one-shape precedent for search).
- **render bridge**: the generic conversion.
- **SqlResult wire shape**: core-owned, independent of the bridge.

### Core Layer (Phase 1)
- **report-render adoption**: depends on [render bridge].
- **SqlResult transport adoption**: depends on [wire shape].

### Integration Layer (Phase 2)
- **conformance fixture + compat review + docs**: depends on [both adoptions].

## Implementation Phases

### Phase 0: Foundation
**Goal**: the two primitives exist.

**Tasks**:
- [ ] Implement a fallible serde-to-GqlValue bridge with declared schema-aware name/scalar adaptation and report response fixtures (depends on: 00172) - Acceptance: actual GraphQL SchemaApplyReport responses preserve dryRun naming and compaction u64 values remain decimal strings where the served schema requires them; REST serde output stays unchanged, all fields survive, and conversion failures are structured.
- [ ] Define the canonical SQL result in core with columns, rows, optional column_types, affected: Option<u64>, and message: Option<String> (no deps) - Acceptance: tests pin Rows → original data/types with no affected/message, Affected(n) → Some(n) with empty data/no message, and Ok(text) → Some(text) with empty data/no affected; no DDL message is lost.

**Exit Criteria**: bridge + wire shape unit-tested; nothing adopted yet.

### Phase 1: Core
**Goal**: adoptions land.

**Tasks**:
- [ ] Adopt the bridge for `SchemaApplyReport` (delete `mutations/operations.rs:526-551`), then add `serde::Serialize` to core `SyncReport`/`CompactionReport` and adopt it for their renders (`operations.rs:349/:353/:436/:443`) (depends on: Phase 0) - Acceptance: GraphQL responses byte-equivalent (response-equality tests); twin serializers deleted.
- [ ] Adopt the canonical result on GraphQL, CLI, FFI, and PgWire; preserve transport names through declared mappings (core affected → FFI affected_rows), add missing column types/CLI headers, regenerate bindings, and migrate callers (depends on: Phase 0) - Acceptance: SELECT, DML, and DDL cases retain types/count/message on each interface; FFI affected_rows is None for SELECT and Some(n) for DML; generated bindings compile their Rust surface and regenerate cleanly.

**Exit Criteria**: no hand-rolled field subset remains for the covered types.

### Phase 2: Integration
**Goal**: pinned, reviewed, documented.

**Tasks**:
- [ ] Add a cross-transport SQL-result conformance fixture (same query → same field set on GraphQL/CLI/FFI, `column_types` present; feeds 00193's registry) and an e2e case in the Rust e2e harness (`tests/e2e/`; the shell harness was removed by 00171) (depends on: Phase 1) - Acceptance: fixture green on the available drivers.
- [ ] Run the compatibility/deprecation review AGENTS.md requires for the FFI `affected_rows` semantic change + CLI header addition; update `docs/src/technical/sql-engine.md` + `ffi.md` + `server.md`; CHANGELOG entries (depends on: Phase 1) - Acceptance: compat review recorded; docs match; CHANGELOG present. Include the `canonical_sql_results` registry addition and update exact-set plus health/GraphQL agreement fixtures; Acceptance also requires the flag to match compiled availability and the implemented workflow.

**Exit Criteria**: Tier 1 green; conformance fixture + compat review + docs landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: `applySchema` via GraphQL before/after the bridge → byte-equivalent response.
- **Edge case**: a SELECT via FFI → `affected_rows` is `None` (not fabricated row count); `column_types` present and matches PgWire's.
- **Error case**: a core report gains a field → shared value conversion carries it without a new handwritten serializer; dynamic GraphQL field registration and all public interface declarations still require updates and conformance coverage.

## Risks

- **FFI `affected_rows` change is public**: needs the AGENTS.md compatibility/deprecation review and binding regen in the same PRD. Mitigation: explicit Phase 2 task; additive `column_types`, semantic fix documented.
- **Dynamic GraphQL type builders still hand-declare fields**: the bridge kills value-serializer duplication, not type registration — stated honestly in docs so nobody expects full derivation.
- **Response-equality is the oracle**: any intentional difference (field ordering) must be normalized in the test, not waved through.

**Capability registry (depends on 00186):** add `canonical_sql_results` to the existing sorted registry only with this implemented surface; document the applicable interface/workflow and compile-time availability. Internal refactors and tests are not separate flags.

## Transport Impact

- **CLI**: Guaranteed — `ddb query` gains column headers (additive; presentation documented).
- **GraphQL**: Guaranteed — byte-equivalent for reports; SQL result gains `column_types` (additive).
- **REST**: Guaranteed — unchanged (already serde-derived); acts as the parity oracle.
- **PgWire**: Guaranteed — unchanged (already consumes `column_types`).
- **FFI**: Guaranteed — record gains `column_types`; `affected_rows` becomes honest Option (compat-reviewed, bindings regenerated).
- **NoSQL HTTP**: Intentionally absent — no SQL surface.
- **Cross-interface conformance plan**: the SQL-result fixture runs the same query across GraphQL/CLI/FFI asserting one field set (PgWire pinned by its existing e2e); report renders pinned by response-equality tests; the fixture feeds the 00193 verb-disposition registry.
