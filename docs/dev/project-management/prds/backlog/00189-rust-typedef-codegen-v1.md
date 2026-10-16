---
catchup: force
default_model: opus
model_tier_rationale: invented algorithm - type mapping and deterministic rendering; catchup forced because 00183 splits base_types.rs
---

# Rust type codegen from typedefs

**Priority:** P3 · **Roadmap:** Phase 3 (downstream enablement) · **Depends on:** 00186 (offline SDL export / typedef introspection) — sequence AFTER it

## Overview

### Problem Statement
Downstream Rust apps hand-mirror every ddb typedef as structs plus selection-set strings, and they guess the generated GraphQL accessor names by probing. The mirror drifts from the served schema, and the guesswork is fragile.

Evidence (jink):

- `jink: src/graphql/types.rs:64-414` hand-mirrors the 7 ddb typedefs as ~15 `SimpleObject`/`InputObject` structs — `Link` (line 66), `Category` (160), `CategoryMembership` (286), `Quote` (318), `SavedSearch` (360), `PinnedResult` (378), `AppConfig`/jink-config (402), plus input structs.
- `LINK_FIELDS`-style selection-set consts live per mutation module: `LINK_FIELDS` (`jink: src/graphql/mutations/link.rs`), `SAVED_SEARCH_FIELDS` (`jink: src/graphql/mutations/saved_search.rs`), `QUOTE_FIELDS` (`jink: src/graphql/mutations/quote.rs`).
- Accessor-name guesswork: `jink: src/graphql/mutations/saved_search.rs:35-36` — "The typed nested accessor on createDoogat for `saved-search` is the camelCase form `savedSearch` (probed against ddb 0.2.5+dev)." A hand-probed name that codegen should emit deterministically.

### Target Users
Downstream Rust apps (jink) that model ddb typedefs in Rust and want generated, schema-accurate structs + selection sets + accessor names committed to their repo.

### Success Metrics
- `ddb schema codegen --lang rust` emits, from the installed typedefs: one Rust struct per typedef (typed fields with correct nullability), a `{TYPE}_FIELDS` selection-set const per type, and the correct camelCase nested-accessor name per type (removing the probing).
- The generated code compiles and matches the schema for a fixture typedef set.
- Regenerating on an unchanged schema is byte-stable (no diff) — deterministic ordering.
- Covers jink's `jink: src/graphql/types.rs` mirror structs, per-module `{TYPE}_FIELDS` consts, and accessor guesswork so jink can delete them (jink-side work, not verified in this repo).

## Functional Decomposition

### Capability: Rust typedef codegen
#### Feature: Struct generation
- **Description**: a Rust struct per typedef.
- **Inputs**: the installed typedefs (`DoogatService::list_type_schemas()`, service/utility.rs:41 — the same `TableSchema` source 00186's SDL uses).
- **Outputs**: a struct with one field per column, typed and nullability-correct.
- **Behavior**: generate read types from the served GraphQL field shape/nullability, using TableSchema plus the shared schema builder/naming rules. ColumnDef.required governs write requirements, not read nullability: currently even required typed scalar columns may be nullable in GraphQL. Expand REFERENCES object/list/raw-ID fields exactly as served.

#### Feature: Selection-set const generation
- **Description**: a `{TYPE}_FIELDS` const per type.
- **Inputs**: the type's fields.
- **Outputs**: a space-separated GraphQL selection string (as jink hand-writes today).
- **Behavior**: emit a valid served GraphQL selection alongside the matching deserializable read struct; REFERENCES selects the documented object/list subfields or raw-ID fields with the required nested selections, using shared schema names.

#### Feature: Nested-accessor name emission
- **Description**: the camelCase accessor name for each type.
- **Inputs**: the typedef name (including hyphenated names like `saved-search`, `category-membership`).
- **Outputs**: the exact accessor identifier the server exposes (e.g. `savedSearch`).
- **Behavior**: REUSE the server's naming (`sanitize_type_name`/`sanitize_field_name`/`pluralize_preserving_case` — defined in `ddb-server/src/schema/base_types.rs:951/:980/:933` at `d77ad58`, re-exported via `schema/mod.rs`; all three are `pub(crate)` AND the `schema/mod.rs:9-12` re-export is itself `pub(crate)`, so this PRD widens visibility at both sites — a `pub` re-export or small naming facade) so a generated name equals the served name — no reimplementation, no probing.

#### Feature: Deterministic output
- **Description**: stable, diff-free regeneration.
- **Inputs**: the typedef set.
- **Outputs**: identical bytes across runs on an unchanged schema.
- **Behavior**: sort by typedef id and declared column order; no `HashMap`-iteration-order leakage.

### Capability: Input and shape coverage
#### Feature: Input-struct generation
- **Description**: emit create/update input structs, not just read structs.
- **Inputs**: the typedef's writable columns.
- **Outputs**: a `Create{Type}Input` struct (mirroring jink's hand-written `CreateLinkInput`, `jink: src/graphql/types.rs:147`).
- **Behavior**: generate separate create/update input mappings from the actual served input types, preserving required-create versus omitted-update versus explicit-null/unset behavior. Do not reuse read-wire Option mapping as the write contract.

#### Feature: Zone-aware field typing
- **Description**: a column's zone informs its Rust type.
- **Inputs**: `ColumnDef::effective_zone()` + `data_type` + `required`.
- **Outputs**: the Rust type matching the served field, including Option for nullable read scalars even when the write column is required; zone and input requirements are mapped separately.
- **Behavior**: map zone + type + nullability consistently so the struct matches the served object shape.

#### Feature: Provenance header
- **Description**: generated files declare they are generated.
- **Inputs**: the codegen run.
- **Outputs**: a `// @generated by ddb schema codegen` header + the source version.
- **Behavior**: stamp a stable header (no timestamp, to preserve byte-stability) so reviewers and tools recognize the file.

## Structural Decomposition

### Repository Structure

```
ddb-cli/src/
├── main.rs                 # Maps to: SchemaAction::Codegen { lang, out }
├── commands/schema.rs      # Maps to: schema() handles Codegen → emit Rust
└── codegen/rust.rs         # NEW: TableSchema → Rust rendering
ddb-server/src/schema/mod.rs # reuse sanitize_*/pluralize naming (single source) + 00186 offline path
ddb-core/                   # TableSchema (list_type_schemas) is the codegen input
```

### Module: codegen::rust (ddb-cli/src/codegen/rust.rs)
- **Maps to capability**: Rust typedef codegen
- **Responsibility**: render `TableSchema` → Rust structs + `{TYPE}_FIELDS` consts + accessor names.
- **Exports**: `render_rust(schemas: &[TableSchema]) -> String`

### Module: schema command (ddb-cli/src/commands/schema.rs)
- **Maps to capability**: Rust typedef codegen
- **Responsibility**: `ddb schema codegen --lang rust [--out]` wires `list_type_schemas()` into the renderer.
- **Exports**: `SchemaAction::Codegen` handling

### Module: server naming (ddb-server/src/schema/base_types.rs, re-exported via schema/mod.rs)
- **Maps to capability**: Rust typedef codegen
- **Responsibility**: the single naming source; widen `sanitize_*`/`pluralize_preserving_case` from `pub(crate)` to a `pub` surface so codegen and the served schema cannot diverge.
- **Exports**: shared naming helpers (made `pub` for the ddb-cli consumer)

### Module: CLI dispatch and schema input (ddb-cli/src/main.rs; ddb-core/src/types/schema.rs)
- **Maps to capability**: Rust typedef codegen; Input and shape coverage
- **Responsibility**: declare the codegen CLI action and consume existing TableSchema metadata alongside served schema shapes.
- **Exports**: SchemaAction::Codegen; existing TableSchema remains the source metadata.

### Module: capability publication (ddb-server/src/capabilities.rs)
- **Maps to capability**: Rust typedef codegen
- **Responsibility**: publish rust_typedef_codegen from the existing 00186 registry and verify its declared interface/build scope.
- **Exports**: the registry entry and matching handshake fixtures.

## Dependency Graph

### Foundation Layer (Phase 0)
Depends on 00186 (offline typedef/SDL introspection).

- **naming reuse + TableSchema→Rust mapping**: the render logic + shared naming.

### Core Layer (Phase 1)
- **`ddb schema codegen --lang rust`**: Depends on [Phase 0 renderer + 00186 offline path].

### Integration Layer (Phase 2)
- **compile/match/stability tests + docs**: Depends on [the codegen command].

## Implementation Phases

### Phase 0: Foundation
**Goal**: schema-accurate rendering + shared naming.

**Tasks**:
- [ ] Implement `render_rust(&[TableSchema])` (structs + `{TYPE}_FIELDS` + accessor names) reusing the server's `sanitize_*`/`pluralize_preserving_case` so names match the served schema; deterministic ordering (depends on: 00186) - Acceptance: unit tests — a fixture `TableSchema` renders the expected struct + FIELDS const + accessor, and the names equal the server's for the same typedef.

**Exit Criteria**: the renderer is correct + name-accurate, unit-tested.

### Phase 1: Core
**Goal**: the CLI command.

**Tasks**:
- [ ] Add `SchemaAction::Codegen { lang, out }` → `ddb schema codegen --lang rust`, feeding `list_type_schemas()` into `render_rust`; write to `--out` or stdout (depends on: Phase 0) - Acceptance: the command emits Rust for a fixture typedef set; re-running on an unchanged schema is byte-identical.

**Exit Criteria**: codegen runs from installed typedefs, deterministically.

### Phase 2: Integration
**Goal**: prove it compiles + matches, document it.

**Tasks**:
- [ ] Compile generated output, execute its generated selection against the real schema, and deserialize the actual response into generated read types (depends on: Phase 1) - Acceptance: fixtures include a required scalar with served nullable shape and REFERENCES object/list/raw-ID expansion; the selection validates and its response deserializes, create/update input requirements are checked independently, shared names match, and repeated generation is byte-stable.
- [ ] Author an e2e test under `tests/e2e/` that runs `ddb schema codegen --lang rust` against a fixture typedef repo and asserts the emitted Rust matches the served schema (new user-facing CLI behavior requires an e2e per the DoD; compile/stability tests alone do not exercise the CLI binary) (depends on: Phase 1) - Acceptance: e2e authored and green.
- [ ] Add a walkthrough (new CLI command) + docs (`docs/src/guide/building-apps.md` codegen section, schema docs) + CHANGELOG; the CLI scenario obligation is covered by the Phase-2 e2e task above (the shell harness was removed by 00171 — no smoke leg exists) (depends on: Phase 1) - Acceptance: walkthrough authored for safe CI verification; docs describe the command + type-mapping table. Include the `rust_typedef_codegen` registry addition and update exact-set plus health/GraphQL agreement fixtures; Acceptance also requires the flag to match compiled availability and the implemented workflow.

**Exit Criteria**: compile + match + stability tests + e2e + scenarios + docs landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: codegen on a fixture typedef set emits structs + `{TYPE}_FIELDS` + accessor names; the generated code compiles.
- **Edge case**: regenerating an unchanged schema produces zero diff (deterministic ordering).
- **Error case**: a hyphenated type (`category-membership`) yields a valid Rust identifier + the correct camelCase accessor; an `ENUM`/`SET`/`REFERENCES` column maps to a documented Rust type (never a panic).

## Risks

- **Naming drift vs the served schema**: the #1 risk — this is exactly the bug jink's "probed" comment reflects. Mitigation: reuse the server's `sanitize_*`/`pluralize_preserving_case` (schema/mod.rs:9-12) as the single naming source; a test asserts generated names equal the served SDL names (00186).
- **Type-mapping completeness**: publish distinct read-wire and write-input tables for ENUM/SET/REFERENCES/body-zone columns. Match actual served JSON and selection structure; do not replace a structured response with String merely to make generated code compile.
- **Language scope**: `--lang rust` only. Mitigation: scope to Rust; the SDL export (00186) already unblocks TypeScript via graphql-codegen, so TS is out of scope here.
- **Determinism**: `HashMap` iteration or column reordering would churn diffs. Mitigation: sort by typedef id + declared column order; a stability test pins it.
- **Sequencing on 00186**: codegen reads the same offline typedef introspection SDL export uses. Mitigation: land after 00186 and reuse its schema path.

**Capability registry (depends on 00186):** add `rust_typedef_codegen` to the existing sorted registry only with this implemented surface; document the applicable interface/workflow and compile-time availability. Internal refactors and tests are not separate flags.

## Transport Impact

Tooling PRD; framed as which ddb interface the tool consumes/produces.

- **CLI**: Guaranteed — `ddb schema codegen --lang rust` (new subcommand) produces Rust source from typedefs.
- **GraphQL / REST / PgWire / FFI / NoSQL HTTP**: Intentionally absent — codegen is a build-time tool that consumes the offline typedef introspection (00186) and emits source; it is not a runtime transport.
- **Cross-interface conformance plan**: the conformance surface is generated-types-compile + names-match-served-SDL (tied to 00186) + regeneration-stability (zero diff). No runtime interface surface is added.
