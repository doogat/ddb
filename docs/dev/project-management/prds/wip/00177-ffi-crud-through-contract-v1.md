---
default_model: opus
model_tier_rationale: invented contract - structured FFI records and policy consumed by 00187 and 00191
plan_expansion: allow
---

# Route FFI CRUD through the app contract

**Priority:** P2 · **Roadmap:** Phase 2 (app contract) · **Depends on:** 00173 (policy-derived `ffi_variant` for the FFI error rewiring), 00172 (closure actor), 00175 (shares the command wiring and the FFI-record-with-warnings pattern)

**Maintainer decision (2026-09-05):** solo-developer beta with no production clients. Replace incompatible interfaces directly in the next release containing this PRD; no legacy entrypoints or deprecation window. Update in-repo callers, fixtures, generated bindings, developer migration guidance, CHANGELOG, and the compatibility checklist in the same change.

## Overview

### Problem Statement
FFI create/update bypass the app contract entirely and cannot surface warnings:

- `create_doogat` (`ddb-core/src/ffi/driver.rs:66-71`) and `update_doogat` (`:77-87`) take RAW Markdown (`content: String`) and call `create_doogat_raw`/`update_doogat_raw`, bypassing `CreateCommand`/`UpdateCommand`. FFI callers get no structured `title`/`tags`/`doogat_type`/`fields` inputs, no `on_conflict` (`ConflictAction`) control, no `unregistered_type_policy`, and no warnings — `create_doogat` returns a bare `String` id.
- The raw path SILENTLY accepts unknown types: `create_doogat_raw` (`ddb-core/src/service/create.rs:428`) documents "Unknown types are accepted (legacy FFI contract)" (`:425-427`). Meanwhile CLI warns (`BaseOnly`, via `try_baseonly_unregistered`, `create.rs:100`) and GraphQL rejects (`Strict`, the `#[default]` on `UnregisteredTypePolicy`, `app_contract/commands.rs:10-20`). That is THREE divergent unregistered-type policies for the same operation, the FFI one buried in service-code prose rather than named in the contract.

The fix pattern already ships: `SchemaApplyReportRecord::from_output` (`ffi/records.rs:318-342`) consumes `AppOutput<T>` and embeds its warnings in an FFI record. FFI CRUD should do the same. With 00175 routing FFI read/delete through the contract with warning-bearing records, this PRD completes the FFI CRUD quartet: create and update join read and delete on the contract, so every FFI mutation surfaces warnings and honors a uniform, recorded policy.

### Target Users
FFI/mobile consumers (Swift/Kotlin), and a future mobile jink: they need structured create/update with conflict control and surfaced warnings — parity with GraphQL — instead of raw-string drift and a silent unknown-type policy.

### Success Metrics
- New FFI `create_structured`/`update_structured` build `CreateCommand`/`UpdateCommand` and return a record embedding `warnings` (mirroring `from_output`).
- FFI create/update honor `on_conflict` and expose the structured inputs (`title`/`tags`/`doogat_type`/`fields`/`unset_fields`).
- The FFI structured create uses `UnregisteredTypePolicy::BaseOnly` — DECIDED 2026-09-05 backlog review: the same policy as the CLI, the other in-process transport; unknown-but-safe types are accepted with a warning surfaced on the record — recorded as the documented FFI default in the app contract (`commands.rs`), NOT a constant buried in `create.rs`.
- Replace raw create_doogat/update_doogat FFI exports with create_structured/update_structured; migrate in-repo callers and delete helpers left with no consumers.
- Swift/Kotlin bindings regenerate clean; an FFI test pins warnings + policy parity with GraphQL.
- FFI error variants derive from 00173's policy `ffi_variant` — the lossy hand-rolled allowlist (`ddb-core/src/ffi/records.rs:72-129`, which collapses `SINGLETON_NOT_FOUND`→`Validation` via the Structured catch-all and `Conflict`→`Git`, `Sync`→`Git`, `Index`→`Sql` at `:98-100`) is retired — pinned by a test that the FFI variant matches the policy for each category.

## Functional Decomposition

### Capability: Structured FFI create/update
#### Feature: create_structured / update_structured
- **Description**: FFI methods that accept structured inputs, build the contract command, and return a record with warnings.
- **Inputs**: UniFFI-supported adapter records/enums (not directly exported core BTreeMap/Value) preserve native boolean/number fields and convert into the core map; design chooses the binding-safe private representation and validates it with local Swift/Kotlin generation. `title`, `tags`, `doogat_type`, `body`, `fields`, `on_conflict` (create); `id` + the optional-update fields + `unset_fields` (update).
- **Outputs**: CreateResultRecord/UpdateResultRecord losslessly mirror the core CreateResult/UpdateResult doogat payload plus AppOutput warnings; an id-only create or warning-only update would drop required result fields.
- **Behavior**: build `CreateCommand`/`UpdateCommand`, call the contract service method, map `AppOutput<CreateResult|UpdateResult>` into the record (mirror `from_output`), surfacing every `AppWarning`.

### Capability: Unified unregistered-type policy
#### Feature: One recorded FFI policy
- **Description**: FFI create's unregistered-type behavior is explicit and recorded in the contract, not a silent legacy default.
- **Inputs**: `CreateCommand.unregistered_type_policy`.
- **Outputs**: the same accept/warn/reject behavior as the chosen policy.
- **Behavior**: set the FFI command's policy to `BaseOnly` (decided 2026-09-05), named as the FFI default in the app contract with a doc comment; the structured path stops relying on the silent `create_doogat_raw` legacy acceptance and surfaces the `BaseOnly` warning on the record instead.

### Capability: FFI error-variant parity
#### Feature: Policy-derived FFI error variant
- **Description**: FFI error variants come from 00173's policy `ffi_variant`, not a hand-rolled allowlist.
- **Inputs**: an `AppError`/`ErrorRendering` carrying the `ffi_variant` hint (defined in ddb-core app_contract by 00173).
- **Outputs**: the FFI error variant for that error's category.
- **Behavior**: replace the lossy allowlist in `ddb-core/src/ffi/records.rs:72-129` — which collapses `SINGLETON_NOT_FOUND`→`Validation` (Structured catch-all), `Conflict`→`Git`, `Sync`→`Git`, `Index`→`Sql` (`:98-100`) — with the policy-derived `ffi_variant` that 00173 exposes, so FFI errors carry the same category the other transports do.

## Structural Decomposition

### Repository Structure

```
ddb-core/src/ffi/
├── driver.rs     # Maps to: create_structured/update_structured build the command, return records
├── records.rs    # Maps to: FFI CreateResult/UpdateResult records with full doogat payload and warnings (mirror from_output)
├── tests.rs      # Maps to: FFI tests: GraphQL warning parity, CLI/BaseOnly policy parity, binding-safe typed inputs
ddb-core/src/app_contract/
└── commands.rs   # Maps to: One recorded FFI unregistered-type policy (documented default)
```

### Module: ffi (ddb-core/src/ffi/driver.rs, records.rs, tests.rs)
- **Maps to capability**: Structured FFI create/update; Unified unregistered-type policy; FFI error-variant parity
- **Responsibility**: expose structured create/update; return records embedding warnings; pin parity.
- **Exports**:
  - `create_structured(...) -> CreateResultRecord`, `update_structured(...) -> UpdateResultRecord`
  - the records + their `from_output`-style constructors

### Module: app_contract commands (ddb-core/src/app_contract/commands.rs)
- **Maps to capability**: Unified unregistered-type policy
- **Responsibility**: name the FFI default policy in the contract (not in `create.rs`).
- **Exports**: the documented FFI policy default on `UnregisteredTypePolicy`/`CreateCommand`.

## Dependency Graph

### Foundation Layer (Phase 0)
Depends on 00172 + 00175 (command wiring + FFI-record-with-warnings pattern); the error-variant rewiring depends on 00173 (policy `ffi_variant`).

- **FFI records + policy decision**: the record shapes and the recorded policy.

### Core Layer (Phase 1)
- **create_structured/update_structured**: Depends on [FFI records + policy].
- **FFI error-variant rewiring**: Depends on [00173 policy `ffi_variant`] - replace the lossy `records.rs:111-126` allowlist.

### Integration Layer (Phase 2)
- **bindings + conformance test + docs**: Depends on [structured methods].

## Implementation Phases

### Phase 0: Foundation
**Goal**: record shapes + policy decided.

**Tasks**:
- [ ] Add UniFFI-safe structured input types and CreateResultRecord/UpdateResultRecord with all core result fields, warnings, and from_output constructors; document BaseOnly as the FFI policy default (depends on: 00172, 00175) - Acceptance: native boolean/number inputs preserve Value variants on conversion to the core BTreeMap, generated Swift/Kotlin bindings accept the input types, results preserve warnings, and commands.rs names BaseOnly. Do not export unsupported BTreeMap/core Value directly or add a core-to-server dependency.

**Exit Criteria**: record shapes + recorded policy exist.

### Phase 1: Core
**Goal**: structured FFI CRUD.

**Tasks**:
- [ ] Implement create_structured/update_structured through CreateCommand/UpdateCommand with on_conflict and BaseOnly, replacing raw create_doogat/update_doogat FFI exports; migrate in-repo callers and remove raw helpers only after verifying no remaining consumer (depends on: Phase 0) - Acceptance: typed inputs, conflict control, warnings, and unknown-type behavior follow the contract; no public raw-only FFI CRUD path remains.
- [ ] Replace the lossy FFI error allowlist (`ffi/records.rs:72-129`) with the policy-derived `ffi_variant` from 00173 (depends on: 00173) - Acceptance: `SINGLETON_NOT_FOUND` no longer collapses to `Validation`, and `Conflict`/`Sync`/`Index` no longer collapse to `Git`/`Sql`; a test asserts the FFI error variant equals the policy's `ffi_variant` for each category.

**Exit Criteria**: FFI CRUD flows through the contract with warnings + conflict control; FFI error variants match the 00173 policy.

### Phase 2: Integration
**Goal**: bindings, parity test, docs.

**Tasks**:
- [ ] Regenerate Swift/Kotlin bindings; add an FFI test asserting warnings parity with GraphQL and unregistered-type policy parity with the CLI (`BaseOnly`: same warning, same accept) (depends on: Phase 1) - Acceptance: bindings regenerate clean; the test pins both parities.
- [ ] Update `docs/src/technical/app-contract.md` + `ffi.md` (structured CRUD and removal of raw FFI exports) + `conformance-harness.md` + CHANGELOG; apply CLAUDE.md's conditional e2e deletion safety net in Claude-routed sessions (this replaces an existing FFI code path) (depends on: Phase 1) - Acceptance: docs match; e2e green. (No showboat walkthrough: FFI has no CLI/HTTP surface for `showboat exec` to drive; parity is pinned by the FFI test + `ffi.md` examples.)

**Exit Criteria**: bindings + parity test + docs landed.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: `create_structured` with a registered type + fields → Expected: a record with the new id and an empty warnings list; the doogat is created with the structured fields.
- **Edge case**: `create_structured` with `on_conflict` set and a colliding singleton → Expected: the conflict policy is honored (same outcome as GraphQL).
- **Edge case**: `update_structured` with `unset_fields` removing a frontmatter key → Expected: the key is removed; the record carries any resulting warning.
- **Error case**: `create_structured` with an unregistered `type:` → Expected: the recorded FFI policy (`BaseOnly`) applies — accepted with a `BaseOnly` warning on the record, matching the CLI — never a silent accept via the legacy raw path.
- **Regression**: migrated FFI callers use structured methods and round-trip equivalent content while receiving warnings; removed raw exports have no in-repo consumers.

## Risks

- **Public FFI beta replacement**: remove raw-only CRUD exports, migrate callers, regenerate bindings, and document exact old/new method signatures with the compatibility checklist and CHANGELOG.
- **Policy choice is a behavior decision**: `BaseOnly` was chosen (2026-09-05) so unknown-but-safe types remain accepted with structured warnings; the difference from GraphQL/REST (`Strict`) is documented in the app contract with rationale, not accidental. Mitigation: pin it with the CLI-parity test.
- **Raw helper ownership**: remove obsolete raw helpers only after checking their actual callers; any retained internal helper remains behind contract-driven behavior and is not a public FFI bypass.

## Transport Impact

- **FFI**: Guaranteed — structured create/update reach parity (structured inputs, `on_conflict`, warnings, explicit `BaseOnly` unregistered-type policy); error variants derive from the 00173 policy `ffi_variant` (lossy allowlist retired); raw-only exports removed.
- **CLI**: Unchanged — already routes create/update through the contract with `BaseOnly`; no code touched; it is the parity oracle for the FFI policy test.
- **GraphQL**: Unchanged — already routes through the contract with `Strict`; it is the parity oracle for the FFI warnings test.
- **REST**: Unchanged — already contract-routed for create/update.
- **PgWire**: Specialized — create/update via SQL `INSERT`/`UPDATE`; the typed command is not its surface.
- **NoSQL HTTP**: Intentionally absent — read-only document surface (`nosql_api.rs`: get/scan/backlinks); no create/update.
- **Cross-interface conformance plan**: FFI structured CRUD matches GraphQL warning content and CLI/BaseOnly unknown-type acceptance; binding generation and migrated callers pin the replacement API. 00193 uses these structured exports for FFI conformance, including read/delete records from 00175.
