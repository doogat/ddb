---
default_model: opus
model_tier_rationale: cross-cutting shape change - object-form fields input on GraphQL, REST, and FFI with shared core normalization
---

# Structured fields input: JSON object with native scalar types

**Priority:** P2 · **Roadmap:** Phase 2 (app contract) · **Depends on:** 00172 (contract input path), 00177 (binding-safe structured FFI inputs).

**Maintainer decision (2026-09-05):** solo-developer beta with no production clients. Replace incompatible interfaces directly in the next release containing this PRD; no legacy entrypoints or deprecation window. Update in-repo callers, fixtures, generated bindings, developer migration guidance, CHANGELOG, and the compatibility checklist in the same change.

## Overview

### Problem Statement
Generic create/update take `fields` as a JSON-ENCODED STRING, forcing every downstream to stringify a JSON object and to encode booleans and numbers as strings:

- REST `CreateBody.fields` is `Option<String>` (`ddb-server/src/rest.rs:25`) and `UpdateBody.fields` is `Option<String>` (`:35`); both are parsed by `crate::schema::parse_fields_json(&json_str)` (`rest.rs:392`, `:420`; defined at `ddb-server/src/schema/base_types.rs:22-31` — it deserializes into `BTreeMap<String, String>` and wraps every value in `Value::String`, so `{"done": true}` cannot even parse today — and re-exported at `schema/mod.rs:10`). GraphQL's generic create/update declare `fields` as a `String` input (`schema/type_defs.rs:437,:448,:466`) decoded only from `GqlValue::String` (`schema/input.rs:63-77`).
- The contract itself is already right: `CreateCommand.fields` and `UpdateCommand.fields` are `BTreeMap<String, Value>` (`ddb-core/src/app_contract/commands.rs:32`, `:54`) — native typed values. The input adapters are stringified; shared normalization must also carry update unsets because core Value has no Null variant.
- The downstream pain is concrete: jink maintains a `build_fields_json` helper purely to stringify its already-structured fields, plus a documented "boolean column needs 'true'/'false' string" probe/workaround (jink audit). Every downstream re-implements the same stringify-then-reparse dance.

Accepting `fields` as a JSON OBJECT with native scalars removes the round-trip and the boolean-string footgun, and lets the object decode straight into the contract's existing typed map.

### Target Users
Downstream apps (jink) building generic create/update payloads; they want to pass a JSON object with real numbers/booleans/null and have them land in the right column types, deleting their stringify + boolean-string glue.

### Success Metrics
- The generic create/update inputs on GraphQL/REST/FFI accept `fields` as a JSON OBJECT with native scalars (number/boolean/null), normalized through one core policy into typed fields plus update unset_fields.
- A boolean in the object lands in a boolean column; a number in a numeric column — no `'true'`/`'false'` string needed.
- JSON-string fields are rejected with InvalidInput; migrate GraphQL operations from String variables to the JSON scalar as part of this beta replacement. **Accepted exception**: a legacy `$f: String` variable passed into a `fields: JSON` position is not rejected by GraphQL schema validation (the dynamic schema accepts any scalar there) — it is rejected at decode time by the fields decoder, with the same migration-hint error. Net effect for callers is identical (operation fails, nothing is written); only the rejection layer differs from the originally stated schema-validation path.
- Object-form fields preserve native scalar types; tests cover invalid string inputs and the documented GraphQL variable migration.
- Core command maps remain BTreeMap<String, Value>; adapter-neutral normalization in app_contract returns typed fields plus update unset_fields. Boundary decoders remain in their transports.
- The public request-shape change passes the compatibility/deprecation review AGENTS.md requires.

## Functional Decomposition

### Capability: Native object fields input
#### Feature: Object-form fields decode
- **Description**: `fields` accepts a JSON object with native scalar values, decoded once into the contract's `BTreeMap<String, Value>`.
- **Inputs**: a JSON object, e.g. `{ "done": true, "priority": 3, "note": null }`.
- **Outputs**: a core normalized result containing BTreeMap<String, Value> plus unset_fields; booleans/numbers retain their Value variants, create-null keys are omitted, and update-null keys become unsets.
- **Behavior**: each adapter decodes its supported input shape, then core app_contract normalization applies the single null policy: create-null omits the key; update-null unsets it. Merge explicit unset_fields with null-derived unsets, deduplicate, and reject an explicit set/unset collision with InvalidInput. Core Value gains no Null variant and no literal null/NULL is written.

#### Feature: Direct beta replacement of stringified fields
- **Description**: replace the stringified fields input with a native object during beta.
- **Inputs**: object-form fields; GraphQL variables declare the existing JSON scalar.
- **Outputs**: normalized typed fields/unsets, or a structured invalid-input error.
- **Behavior**: reject JSON-string fields; old GraphQL String-variable operations fail (rejected by the fields decoder at execution, not by schema validation — see the accepted exception in Success Metrics) and are explicitly migrated in examples, callers, fixtures, and release notes. No legacy decoder or compatibility window remains.

## Structural Decomposition

### Repository Structure

```
ddb-server/src/schema/
├── base_types.rs # Maps to: object-form decode (parse_fields_json lives here, :22-31; decode object values at the GraphQL boundary, then call core normalization)
├── type_defs.rs  # Maps to: GraphQL generic create/update `fields` input type (:437,:448,:466 → the existing JSON scalar)
├── input.rs      # Maps to: GraphQL decode (fields_map/opt_fields_map, :63-77) routes through the one decode path
└── mod.rs        # update the decoder re-export after removing the string-only helper
ddb-server/src/
└── rest.rs       # Maps to: CreateBody/UpdateBody fields accept a JSON object (serde_json::Value), rejecting encoded strings
ddb-server/src/schema/mutations/
└── operations.rs # Maps to: call sites of fields_map/opt_fields_map (:33-34,:67-68,:107,:149-150), pass normalized fields and update unsets
ddb-core/src/ffi/
└── driver.rs     # Maps to: FFI adapter inputs preserve native scalars and explicit null/unset intent (via 00177 create_structured)
ddb-core/src/app_contract/
└── commands.rs   # Maps to: core normalized fields/unsets policy over existing BTreeMap<String, Value>
```

### Module: fields decode (ddb-server/src/schema/base_types.rs)
- **Maps to capability**: Native object fields input
- **Responsibility**: decode GraphQL values into adapter-neutral input for core normalization; reject stringified fields.
- **Exports**: an object decoder replacing the string-only parse_fields_json helper.

### Module: transports (ddb-server/src/rest.rs, schema/type_defs.rs, schema/input.rs, schema/mod.rs, schema/mutations/operations.rs; ddb-core/src/ffi/driver.rs, records.rs)
- **Maps to capability**: Native object fields input
- **Responsibility**: accept the object form on their generic create/update inputs; route through the one decode path.
- **Exports**: `CreateBody`/`UpdateBody` with an object-capable `fields`; the GraphQL/FFI generic inputs.

### Module: shared normalization (ddb-core/src/app_contract/commands.rs)
- **Maps to capability**: Native object fields input
- **Responsibility**: normalize typed fields and explicit null/unset intent independently of GraphQL, REST, or UniFFI types.
- **Exports**: normalized typed fields plus update unset_fields; no server dependency from core.

## Dependency Graph

### Foundation Layer (Phase 0)
Depends on 00172 and 00177.

- **core normalization → typed fields and unset_fields**: the one decode path.

### Core Layer (Phase 1)
- **transport inputs accept object**: Depends on [decode path] - GraphQL/REST/FFI.

### Integration Layer (Phase 2)
- **conformance + tests + docs + walkthrough + compat review**: Depends on [transport inputs].

## Implementation Phases

### Phase 0: Foundation
**Goal**: one core normalization policy with transport-owned object decoders.

**Tasks**:
- [ ] Implement adapter-neutral fields normalization in app_contract and boundary object decoders, preserving core Value without Null (depends on: 00172, 00177 binding-safe FFI inputs) - Acceptance: tests pin native boolean/number variants, create-null omission, update-null unsets, deduplication with explicit unset_fields, and InvalidInput for a set/unset collision or encoded-string input; FFI communicates null intent through its binding-safe input representation without a server dependency.

**Exit Criteria**: core normalization preserves fields and unset intent; encoded-string inputs are rejected.

### Phase 1: Core
**Goal**: transports accept the object form.

**Tasks**:
- [ ] REST: accept object fields through the boundary decoder and shared normalization (depends on: Phase 0) - Acceptance: native booleans/numbers persist correctly, create-null omits, update-null unsets, and encoded-string fields return InvalidInput.
- [ ] Change GraphQL fields to the existing JSON scalar and FFI structured inputs to the binding-safe representation; decode through core normalization and migrate affected callers (depends on: Phase 0) - Acceptance: JSON-variable operations and FFI native scalars round-trip; null semantics agree; legacy String-variable GraphQL operations fail validation as documented; local Swift/Kotlin bindgen succeeds.

**Exit Criteria**: object-form fields accepted on GraphQL/REST/FFI.

### Phase 2: Integration
**Goal**: conformance, coverage, docs, compat review.

**Tasks**:
- [ ] Add a create-with-typed-fields step to the PRD 00148 conformance harness across GraphQL + REST asserting a boolean lands as a boolean column (the harness's REST driver lands in 00175) (depends on: Phase 1) - Acceptance: both transports store a native boolean, not a string.
- [ ] Author the scenarios in the Rust e2e harness (`tests/e2e/`; the shell harness was removed by 00171) — object-form round-trip, null/unset behavior, and rejected encoded strings — update `docs/src/technical/app-contract.md` + `rest-api.md` + `server.md` + `data-model.md` + `compatibility-checklist.md` + CHANGELOG, add a showboat walkthrough for object-form fields, and complete the compatibility/deprecation review AGENTS.md requires for public request-shape changes (depends on: Phase 1) - Acceptance: e2e scenarios green; docs describe the direct beta migration, including GraphQL variable declarations; walkthrough authored through showboat; CI verification uses the safe wrapper.

**Exit Criteria**: conformance + scenarios + docs + compat review landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: create with `fields: { "done": true, "priority": 3 }` → Expected: `done` stored as a boolean column value, `priority` as a number.
- **Edge case**: `fields: { "note": null }` → Expected: on create `note` is omitted from the document; on update `note` is unset; no literal null text is written — identical on GraphQL/REST/FFI.
- **Error case**: encoded-string fields are rejected with InvalidInput, and GraphQL String-variable operations fail (decoder-rejected, not schema-validation-rejected — accepted exception, see Success Metrics); neither silently reinterprets the old input.

## Risks

- **Public beta input replacement**: changing GraphQL fields from String to JSON breaks existing operation declarations. Mitigation: migrate examples/callers to JSON variables, update contract fixtures, and record exact old/new inputs, release boundary, and a single-commit revert path through the compatibility checklist; no deprecation window.
- **GraphQL scalar typing**: representing an arbitrary-scalar object in the GraphQL input schema (JSON scalar vs typed input). Mitigation: use the existing JSON-scalar approach; document the accepted scalar set (number/boolean/string/null).
- **Type coercion to column types**: an object number for a text column (or vice versa) must coerce/validate predictably. Mitigation: `resolve_column` (`base_types.rs:1003-1008`) resolves names only; coercion reuses the materializer's rules (e.g. `normalize_bool_str`, `indexer/materialize.rs:31`); test mismatches; fail clearly at the boundary.

## Transport Impact

- **GraphQL**: Guaranteed — generic create/update accept object-form `fields` (native scalars) via the one decode path.
- **REST**: Guaranteed — `CreateBody`/`UpdateBody` `fields` accept a JSON object, rejecting encoded strings — the primary drift fix.
- **FFI**: Guaranteed — `create_structured`/`update_structured` (00177) take UniFFI-safe typed adapter input and normalize into the core map; object-form is the FFI norm.
- **CLI**: Guaranteed — already accepts `--set key=value` (repeatable, `ddb-cli/src/main.rs:282-284`/`:303-305`; removal via `--unset`) and maps each pair into the typed fields map; native scalar typing is inferred at the column boundary.
- **PgWire**: Specialized — typed columns are set via SQL `INSERT`/`UPDATE` with native SQL literals already; the JSON-object input is not its surface.
- **NoSQL HTTP**: Intentionally absent — the NoSQL HTTP surface is read-only (`nosql_api.rs` exposes only GET: get/scan/backlinks), so it has no create/update input to carry `fields`. (Diverges from the drafting brief's "NoSQL Guaranteed": verified read-only, no write route exists.)
- **Cross-interface conformance plan**: the PRD 00148 conformance harness runs create-with-typed-fields across GraphQL + REST and asserts a boolean lands as a boolean column (not a string); smoke/integration + e2e cover the object-form round-trip, null/unset behavior, and rejected encoded strings; the compatibility/deprecation review gates the public request-shape change. jink impact: deletes jink's `build_fields_json` and the boolean-string workaround — a day-one win for every downstream.
