---
default_model: opus
model_tier_rationale: cross-cutting shape change - AppOutput read/delete across CLI, GraphQL, REST, and FFI
---

# Route read and delete through the app contract

**Priority:** P2 · **Roadmap:** Phase 2 (app contract) · **Depends on:** 00172 (closure-message actor — routing a verb through the server is one touch point after it lands)

**Maintainer decision (2026-09-05):** solo-developer beta with no production clients. Replace incompatible interfaces directly in the next release containing this PRD; no legacy entrypoints or deprecation window. Update in-repo callers, fixtures, generated bindings, developer migration guidance, CHANGELOG, and the compatibility checklist in the same change.

## Overview

### Problem Statement
The app contract already defines the read and delete commands and their result DTOs, but nothing consumes them — they are dead code:

- `ReadCommand { id }` (`ddb-core/src/app_contract/commands.rs:39`) and `DeleteCommand { id }` (`:64`) exist; `DeleteCommand`'s own doc (`:60-62`) states the service "returns a result that includes any broken backlinks created by the deletion, which transports should surface rather than discard."
- The result DTOs `ReadResult` (`results.rs:31`), `DeleteResult` (`:19`), and `BrokenBacklink` (`:12`) are defined but have "no in-tree consumer yet by design" (`results.rs:3-6`).
- Every transport instead binds a concrete `DoogatService` method directly: FFI `read_doogat` (`ffi/driver.rs:73-75`) returns a bare `String` and `delete_doogat` (`:89-94`) returns `()`; REST and GraphQL bind their own read/delete handlers. Because the contract is bypassed, the delete warning path (broken backlinks) is surfaced inconsistently — or dropped — and read has no uniform shape. Grounded 2026-09-05: GraphQL `deleteDoogat` returns `Boolean` and discards the backlinks (`schema/mutations/operations.rs:179-188`), REST returns 204 and discards (`rest.rs:441-447`), the actor drops them at `actor/handlers.rs:85-88`, FFI discards at `driver.rs:89-94`; only the CLI prints a warning line (`ddb-cli/src/commands/crud.rs:126-133`, pinned by `tests/e2e/delete_backlinks.rs`), and even that is rendered from the bare `Vec<(String, String)>`, not the contract shape.

This leaves the app-contract posture half-built: transports should decode into a command and render `AppOutput<T> { value, warnings }` (`output.rs:24-26`), surfacing warnings rather than each transport re-deriving read/delete semantics. The pattern already ships for schema-apply (`SchemaApplyReportRecord::from_output`, `ffi/records.rs:318-342`).

### Target Users
Downstream apps (jink) that read and delete doogats across transports and need one consistent result shape plus reliably surfaced delete warnings (broken backlinks) instead of per-transport drift.

### Success Metrics
- `ReadCommand`/`DeleteCommand` flow `transport → DoogatService → AppOutput<ReadResult|DeleteResult>`; the direct method bindings are retired or thinned to decode-and-render shims.
- A delete that breaks a backlink surfaces a `BrokenBacklink` warning on EVERY delete-capable transport (CLI, GraphQL, REST, FFI) — none silently discards it.
- Read returns the same `ReadResult` shape across CLI/GraphQL/REST/FFI.
- The conformance harness (`tests/e2e/conformance/`, built by PRD 00148 — link-ok: that PRD file was GC'd, the harness is live) asserts read + delete parity (shape + warnings) across CLI, GraphQL, and REST.
- Existing behavioral regressions stay covered; response assertions migrate to the new beta contract. GraphQL delete returns a warning-bearing result, REST DELETE returns 200 with its result envelope, and FFI read/delete return records.

## Functional Decomposition

### Capability: Contract-routed read
#### Feature: Read via ReadCommand → AppOutput<ReadResult>
- **Description**: every transport decodes its read request into `ReadCommand` and renders `AppOutput<ReadResult>`.
- **Inputs**: a doogat `id` from the transport request.
- **Outputs**: `AppOutput<ReadResult> { value, warnings }`.
- **Behavior**: the service builds `ReadResult` from the parsed doogat; transports serialize `value` and surface any `warnings` (e.g. a stale-index or parse warning) rather than dropping them.

### Capability: Contract-routed delete
#### Feature: Delete via DeleteCommand → AppOutput<DeleteResult>
- **Description**: every delete-capable transport decodes into `DeleteCommand` and renders `AppOutput<DeleteResult>`, surfacing broken-backlink warnings.
- **Inputs**: a doogat `id` (+ commit message where the transport supplies one).
- **Outputs**: `AppOutput<DeleteResult> { value, warnings }` where `warnings` carries a `BrokenBacklink` entry per dangling reference.
- **Behavior**: the service deletes, computes broken backlinks, and returns them as `AppWarning`s; transports render them. No transport returns a bare `()`/id that discards the warning.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/service/
├── read.rs        # Maps to: Read via ReadCommand (build ReadResult, attach warnings)
├── delete.rs      # Maps to: Delete via DeleteCommand (build DeleteResult + BrokenBacklink warnings)
ddb-core/src/ffi/
├── driver.rs      # Maps to: read_doogat/delete_doogat decode into the command, return records
├── records.rs     # Maps to: FFI ReadResult/DeleteResult records embedding warnings (mirror from_output)
ddb-server/src/
├── schema/queries.rs               # Maps to: GraphQL read resolver renders AppOutput
├── schema/mutations/operations.rs  # Maps to: GraphQL delete resolver surfaces warnings
└── rest.rs        # Maps to: REST read/delete handlers render AppOutput
ddb-cli/src/commands/
└── crud.rs        # Maps to: CLI read/delete render value + warnings
```

### Module: service read/delete (ddb-core/src/service/)
- **Maps to capability**: Contract-routed read; Contract-routed delete
- **Responsibility**: accept the command, return `AppOutput<ReadResult|DeleteResult>` with warnings.
- **Exports**:
  - `read(ReadCommand) -> AppOutput<ReadResult>`
  - `delete(DeleteCommand) -> AppOutput<DeleteResult>` (broken backlinks as warnings)

### Module: ffi (ddb-core/src/ffi/driver.rs, records.rs)
- **Maps to capability**: Contract-routed read/delete
- **Responsibility**: decode into the command; return a record that embeds warnings (mirror `SchemaApplyReportRecord::from_output`, `records.rs:318-342`).
- **Exports**: `read_doogat`/`delete_doogat` returning records with a `warnings` field.

### Module: application adapters and conformance (ddb-server/src/schema/queries.rs, schema/mutations/operations.rs, rest.rs; ddb-cli/src/commands/crud.rs; tests/e2e/conformance/)
- **Maps to capability**: Contract-routed read; Contract-routed delete
- **Responsibility**: decode commands, render complete values/warnings, and add the REST conformance driver with read/delete parity fixtures.
- **Exports**: replacement beta read/delete responses, CLI rendering, and driver_rest.

## Dependency Graph

### Foundation Layer (Phase 0)
Depends on 00172 (closure actor).

- **service read/delete → AppOutput**: the contract-routed service methods; the shared foundation for every transport.

### Core Layer (Phase 1)
- **transport decode/render**: Depends on [service read/delete] - CLI, GraphQL, REST, FFI decode into the command and render `AppOutput`.

### Integration Layer (Phase 2)
- **conformance + tests + docs + walkthrough**: Depends on [transport decode/render].

## Implementation Phases

### Phase 0: Foundation
**Goal**: make the service speak the contract for read/delete.

**Tasks**:
- [ ] Implement `read(ReadCommand) -> AppOutput<ReadResult>` and `delete(DeleteCommand) -> AppOutput<DeleteResult>` (broken backlinks as `AppWarning`s) (depends on: 00172) - Acceptance: unit tests assert `ReadResult` content and that a delete breaking a backlink yields a `BrokenBacklink` warning while a clean delete yields none.

**Exit Criteria**: the service returns contract shapes with warnings for read + delete.

### Phase 1: Core
**Goal**: route every transport through the command.

**Tasks**:
- [ ] Route CLI + GraphQL + REST read/delete through the command and render all AppOutput warnings; replace GraphQL's Boolean delete with a result object and REST DELETE 204 with 200 and the canonical REST data/warnings envelope; update existing callers and response fixtures (depends on: Phase 0) - Acceptance: read values and broken-backlink warnings survive on each transport; clean deletes have an empty warnings array; CLI retains its existing content/stdout and warning/stderr rendering; no warning-dropping legacy handler remains.
- [ ] Add FFI `read_doogat`/`delete_doogat` records embedding warnings (mirror `from_output`); regenerate Swift/Kotlin bindings (depends on: Phase 0) - Acceptance: an FFI test asserts a delete surfaces a `BrokenBacklink` warning; bindings regenerate clean.

**Exit Criteria**: all read/delete-capable transports render the contract shape with warnings.

### Phase 2: Integration
**Goal**: pin parity, cover, document.

**Tasks**:
- [ ] Extend the conformance harness's existing `StepOp::ReadDoogat`/`StepOp::DeleteDoogat` (`fixture.rs:54,:56`; a CLI↔GraphQL not-found read is already asserted at `cross_driver_crud.rs:102-127`) into a read + delete parity step (shape + warnings) across CLI+GraphQL+REST. NOTE: the harness (`tests/e2e/conformance/`, PRD 00148 — link-ok) currently ships CLI + GraphQL drivers only — building its REST driver (`driver_rest`, mirroring `driver_graphql` over the REST routes) is IN SCOPE for this task; 00176/00178 reuse it (depends on: Phase 1) - Acceptance: `driver_rest` exists and the harness asserts identical `ReadResult` fields and the same broken-backlink warning across the three.
- [ ] Author the scenarios in the Rust e2e harness (`tests/e2e/`; the shell harness was removed by 00171) — read shape, delete-with-broken-backlink warning — update `docs/src/technical/app-contract.md` + `server.md` + `rest-api.md` + `ffi.md` + `conformance-harness.md` + CHANGELOG, and add a showboat walkthrough for delete-surfaces-warnings (depends on: Phase 1) - Acceptance: e2e scenarios green; docs match; walkthrough authored through showboat; CI verification uses `dev/bin/safe-showboat-verify`.

**Exit Criteria**: conformance + scenarios + docs + walkthrough landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: read an existing doogat via each transport → Expected: identical `ReadResult`; no warnings.
- **Edge case**: delete a doogat with no inbound references → Expected: `DeleteResult` with an empty `warnings` list on every transport.
- **Error case**: delete a doogat that is the target of a reference → Expected: a `BrokenBacklink` warning is surfaced (not discarded) on CLI, GraphQL, REST, and FFI.

## Risks

- **Public beta response replacement**: old Boolean/204/string/unit results cannot carry the contract. Mitigation: replace them directly under the maintainer decision, document exact old/new requests and responses in developer guidance and CHANGELOG, update conformance fixtures, and name the single-commit revert path.
- **FFI signature replacement**: read/delete change from String/unit to records. Regenerate bindings and migrate in-repo callers in this PRD; local bindgen validates signatures and 00194 owns platform compilation.
- **Direct-binding retirement could drop a caller**: some path may still call the old method. Mitigation: grep call sites; thin the old method to a decode-and-delegate shim rather than deleting blindly; apply CLAUDE.md's conditional e2e deletion safety net in Claude-routed sessions (this replaces existing code paths).

## Transport Impact

- **CLI**: Guaranteed — `ddb read`/`ddb delete` decode into the command and print `value` + any broken-backlink warning.
- **GraphQL**: Guaranteed — the read query and delete mutation render `AppOutput`; warnings in the response envelope.
- **REST**: Guaranteed — `GET`/`DELETE` document routes render `AppOutput` with warnings.
- **FFI**: Guaranteed — `read_doogat`/`delete_doogat` return records embedding warnings (mirroring `from_output`).
- **PgWire**: Specialized — read/delete are expressed as SQL `SELECT`/`DELETE`; the typed command is not its surface.
- **NoSQL HTTP**: read Guaranteed — `nosql_get` is a read and renders the contract shape; delete Intentionally absent — the NoSQL HTTP surface is read-only (`nosql_api.rs` exposes only GET: get/scan/backlinks), so it has no delete route.
- **Cross-interface conformance plan**: the PRD 00148 conformance harness runs read + delete across CLI, GraphQL, and REST and asserts identical `ReadResult`/`DeleteResult` shape and the same `BrokenBacklink` warning; an FFI test pins the FFI record's warning; smoke/integration + e2e cover the delete-warning path. jink impact: one consistent read/delete result shape and reliably surfaced delete warnings, deleting jink's per-transport read/delete glue.
