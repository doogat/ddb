---
catchup: force
default_model: opus
model_tier_rationale: invented contract - capability flag set consumed by jink
---
catchup: force

# Offline SDL export and capability/version handshake

**Priority:** P3 · **Roadmap:** Phase 3 (downstream enablement) · **Depends on:** none

## Overview

### Problem Statement
Two downstream-integration frictions share a root cause: there is no way to read ddb's schema or feature set WITHOUT a running, token-authed server.

- **Codegen needs a live server**: `jink: frontend/codegen.ts:6-11` introspects `ddb-server`'s typed schema, and the comment states plainly that "`npm run codegen` requires ddb-server running" while "`generated.ts` is committed so routine `npm run check` and `npm run build` work without ddb." So a build-time schema artifact depends on a runtime server + token — awkward for CI and fresh checkouts.
- **Feature probing is done live**: jink's 00026 adoption PRD requires checking the deployed binary rather than trusting a merged PRD. Its known probes are base-field orderBy, relation/negation filtering, and inline ZONE DDL. These requirements are embedded here; the cross-repo PRD may move among lifecycle directories and is optional corroboration, never a mandatory execution input.

Drift note (corrected against the code): `/health` (ddb-server/src/lib.rs:239-244) ALREADY returns `{ status, version, uptime_seconds, index_reachable }`, so a version signal exists. The real gap is a machine-readable **capability/feature-flag set** and a GraphQL `capabilities` field — not version.

### Target Users
Downstream apps (jink) that generate code from ddb's schema in CI/offline and that gate feature adoption on whether a capability is live in the deployed binary.

### Success Metrics
- `ddb schema sdl` prints the dynamic GraphQL SDL for the currently installed typedefs offline — no `ddb serve`, no token, no bound listener — and the exported SDL matches the schema the server serves for the same typedef set (conformance test).
- `/health` reports a `capabilities` feature-flag set alongside the existing `version`; a GraphQL `capabilities` field returns the same version + flags.
- ddb enables offline downstream codegen and one capability read in place of the three known live probes. Actual jink adoption is downstream-owned and is not an in-repo completion condition.

## Functional Decomposition

### Capability: Offline SDL export
#### Feature: Build the dynamic schema without listeners
- **Description**: construct the same dynamic schema the server builds, without binding HTTP/pg ports.
- **Inputs**: the repo's installed typedefs (`DoogatService::list_type_schemas()`, service/utility.rs:41).
- **Outputs**: an `async_graphql::dynamic::Schema`.
- **Behavior**: reuse `ddb_server::schema::build_schema(...)` (schema/mod.rs:27) with `reloader: None`. `.sdl()` walks the type system only (resolvers are never executed), so an in-process actor/read-pool over the local repo is sufficient and no server is "running."

#### Feature: Render + print SDL (`ddb schema sdl`)
- **Description**: a CLI subcommand that prints the SDL.
- **Inputs**: the repo path (existing global arg).
- **Outputs**: SDL text on stdout.
- **Behavior**: add `SchemaAction::Sdl` to the existing schema subcommand (ddb-cli/src/commands/schema.rs) and print `build_schema(...).sdl()`.

### Capability: Capability/version handshake
#### Feature: `/health` capabilities set
- **Description**: extend `/health` with a `capabilities` object (feature flags), keeping `version`.
- **Inputs**: the compiled binary's feature/version state.
- **Outputs**: `{ status, version, uptime_seconds, index_reachable, capabilities: {...} }`.
- **Behavior**: derive flags from a single-source `capabilities` module (flag set DECIDED 2026-09-05 backlog review — see the Phase 0 task); add the field additively in `health_ready` (lib.rs:224) as an object `{ "<flag>": true, ... }`.

#### Feature: GraphQL `capabilities` field
- **Description**: a query field returning version + the same flag set.
- **Inputs**: none.
- **Outputs**: `{ version, flags: [...] }` (the same flag names as the `/health.capabilities` keys).
- **Behavior**: add the field in the schema queries, backed by the same `capabilities` module (no second source of truth).

### Capability: Export ergonomics
#### Feature: SDL to stdout or file
- **Description**: `ddb schema sdl` writes to stdout or a target file.
- **Inputs**: an optional `--out <path>`.
- **Outputs**: SDL on stdout (default) or written to the file.
- **Behavior**: default to stdout for piping; `--out` writes the committed artifact downstreams check in, replacing jink's live-server codegen prerequisite (`jink: frontend/codegen.ts:10`).

#### Feature: Introspection parity
- **Description**: exported SDL equals what GraphQL introspection returns.
- **Inputs**: the served dynamic schema.
- **Outputs**: matching SDL.
- **Behavior**: both derive from one `build_schema`, so `ddb schema sdl` and live introspection agree; the conformance test pins it.

#### Feature: Empty-schema safety
- **Description**: exporting a repo with no typedefs yields the base schema, not an error.
- **Inputs**: a repo with zero typedefs.
- **Outputs**: base SDL (root query/mutation only).
- **Behavior**: `list_type_schemas()` returns empty and `build_schema` still renders the base types.

## Structural Decomposition

### Repository Structure

```
ddb-server/src/
├── capabilities.rs         # NEW: single source for version + feature-flag set
├── schema/mod.rs           # reuse build_schema(...).sdl() offline; expose a render helper
├── schema/queries.rs       # Maps to: GraphQL `capabilities` field
├── lib.rs                  # health_ready(): add `capabilities` (version already present)
ddb-cli/src/
├── main.rs                 # Maps to: SchemaAction::Sdl variant
└── commands/schema.rs      # Maps to: schema() handles Sdl → print offline SDL
```

### Module: capabilities (ddb-server/src/capabilities.rs)
- **Maps to capability**: Capability/version handshake
- **Responsibility**: the authoritative version + feature-flag set for this binary.
- **Exports**: `capabilities()` (stable-ordered flag list), `version()`

### Module: schema (ddb-server/src/schema/mod.rs)
- **Maps to capability**: Offline SDL export; Capability/version handshake
- **Responsibility**: offline SDL rendering + the `capabilities` query field.
- **Exports**: an offline `render_sdl(...)` helper; the `capabilities` query field

### Module: schema command (ddb-cli/src/commands/schema.rs)
- **Maps to capability**: Offline SDL export
- **Responsibility**: `ddb schema sdl` prints the offline SDL (ddb-cli already depends on ddb-server, Cargo.toml:29).
- **Exports**: `SchemaAction::Sdl` handling

### Module: handshake endpoints and CLI dispatch (ddb-server/src/lib.rs, schema/queries.rs; ddb-cli/src/main.rs)
- **Maps to capability**: Capability/version handshake; Offline SDL export; Export ergonomics
- **Responsibility**: render one registry through health/GraphQL and dispatch the offline SDL command without listeners or credentials.
- **Exports**: health capabilities, GraphQL capabilities, and SchemaAction::Sdl.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **capabilities module**: the single flag/version source used by both surfaces.

### Core Layer (Phase 1)
- **offline SDL helper + `ddb schema sdl`**: Depends on [existing build_schema]; independent of capabilities.
- **health + GraphQL capabilities**: Depends on [capabilities].

### Integration Layer (Phase 2)
- **conformance + docs + scenarios**: Depends on [offline SDL, capabilities surfaces].

## Implementation Phases

### Phase 0: Foundation
**Goal**: one authoritative capability/version source.

**Tasks**:
- [ ] Add capabilities.rs with version and the sorted seed typed_orderby_base_fields, relation_where_filters, inline_zone_ddl, schema_apply, offline_sdl, capabilities_handshake (no deps) - Acceptance: a unit test pins this exact initial set and stable ordering; the first three flags cover the embedded orderBy, relation/negation, and inline-ZONE probes. No cross-repo input is required. Later producer PRDs add their explicitly assigned flags only when implemented; update the expected registry set and health/GraphQL agreement tests with each addition.

**Exit Criteria**: capability set is defined and unit-tested.

### Phase 1: Core
**Goal**: offline SDL + the handshake surfaces.

**Tasks**:
- [ ] Add an offline `render_sdl` helper reusing `build_schema(list_type_schemas(), reloader=None)` and wire `SchemaAction::Sdl` → `ddb schema sdl` (no listeners) (no deps) - Acceptance: `ddb schema sdl` prints SDL for a repo's typedefs with no server, no token, no bound port; --out writes identical content without duplicating it on stdout, and an empty repository yields valid base SDL.
- [ ] Add `capabilities` to `health_ready` (keep `version`) and a GraphQL `capabilities` field, both backed by the Phase 0 module (depends on: Phase 0) - Acceptance: `/health` and the GraphQL field both return version + the same flags.

**Exit Criteria**: offline SDL prints; both handshake surfaces report version + flags.

### Phase 2: Integration
**Goal**: conformance, scenarios, docs.

**Tasks**:
- [ ] Conformance test: offline `render_sdl` equals the served schema's `.sdl()` for a fixture typedef set (normalized equality) (depends on: Phase 1) - Acceptance: exported SDL == served SDL.
- [ ] Add e2e coverage in the Rust harness (`tests/e2e/`; the shell harness was removed by 00171): the `ddb schema sdl` CLI scenario, health `capabilities`, and GraphQL `capabilities`; + a walkthrough (new CLI command) + docs (`docs/src/technical/server.md` health/capabilities, `docs/src/guide/building-apps.md` offline codegen) + CHANGELOG (depends on: Phase 1) - Acceptance: e2e scenarios green; docs match shipped behavior.

**Exit Criteria**: SDL-equals-served conformance + scenarios + docs landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: `ddb schema sdl` on a repo with typedefs prints SDL containing the typed queries and objects.
- **Edge case**: exported SDL equals the served schema `.sdl()` for the same typedefs (the conformance pin).
- **Error case**: `ddb schema sdl` on a repo with no typedefs prints the base schema (no panic); `/health` still reports `capabilities` when the index is degraded.

## Risks

- **Offline build needs resolver handles**: `build_schema(actor: ActorHandle, read_pool: ReadPool, type_schemas: Vec<TableSchema>, reloader: Option<Arc<SchemaReloader>>)` takes resolver handles (signature verified at `schema/mod.rs:27-32`). Mitigation: spawn in-process handles over the repo purely to render SDL (no listeners bound = not "running a server"); if that is undesirable, decouple type-system construction from resolver wiring. Choose the smaller change and document it.
- **SDL drift (offline vs served)**: two builders would diverge. Mitigation: both paths call the SAME `build_schema`; the conformance test pins equality.
- **Capability-flag semantics**: a flag must mean "present in this binary" (00026's actual need), not a config toggle. Mitigation: derive flags from compiled features/version; document the contract.
- **Version already present**: do not rename/duplicate the `version` key. Mitigation: extend `/health` additively; keep `version`.

**Later flag ownership:** 00187 idempotent_create; 00188 ephemeral_serve; 00189 rust_typedef_codegen; 00190 read_freshness; 00191 ffi_git_sync; 00192 nosql_http_crud; 00197 canonical_sql_results. Each producer updates the registry, documents the applicable interface/workflow, and tests availability in the current build; feature-gated surfaces advertise only when compiled. Internal refactors, CI changes, and test-only PRDs add no flag.

## Transport Impact

Tooling PRD; Transport Impact is framed as which ddb interface the tool produces/consumes.

- **CLI**: Guaranteed — `ddb schema sdl` (new subcommand) produces offline SDL.
- **GraphQL**: Guaranteed — a `capabilities` query field is added; SDL introspection parity is pinned against the served schema.
- **Server `/health`**: Guaranteed — a `capabilities` object is added (version unchanged).
- **REST / PgWire / FFI / NoSQL HTTP**: Intentionally absent — SDL export is a GraphQL-schema concern, and the handshake is exposed via `/health` + GraphQL, which is sufficient for downstream probing (FFI already has its own `list_type_schemas` discovery).
- **Cross-interface conformance plan**: the golden conformance is offline-SDL-equals-served-schema (fixture typedef set); a second test asserts `/health` and the GraphQL `capabilities` field agree on version + flags. Smoke/integration/e2e scenarios cover both surfaces.
