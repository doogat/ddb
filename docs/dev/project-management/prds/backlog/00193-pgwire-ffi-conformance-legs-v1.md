---
default_model: sonnet
model_tier_rationale: test-driver and docs slices; the classifier's test_port row lands sonnet anyway
---

# PgWire and FFI Conformance Legs

Depends on: 00192

Source: `dev/local/audit-results/2026-07-13-goal-gap-analysis.md` (gap G1).

> **Provenance:** the verb-disposition registry was added by the 2026-07-16 assessment (F7) and folded into the body's Features/tasks at the 2026-08-14 backlog review. Grounding note (`d77ad58`): the app contract has no `AppCommand` enum — commands are per-verb structs (`CreateCommand`, `ReadCommand`, `UpdateCommand`, `DeleteCommand`, `ApplySchemaCommand`, `SearchCommand` in `app_contract/commands.rs`), so the registry enumerates the command structs, and the completeness test keys on that list. This is the forcing function the 00161 probe proved missing: schema-apply's parity was bought with 5 review cycles, the same missing-reload bug shipped twice, and the disposition table in `schema-apply.md` emerged from review pressure rather than any mechanical check.

## Overview

### Problem Statement

The conformance harness (`tests/e2e/conformance/`) runs the golden CRUD and validation workflows through concrete drivers and compares outcomes — but only `CliDriver` and `GraphqlDriver` exist today. REST joins via 00175 and NoSQL HTTP joins with a blocking gate via 00192, which still leaves PgWire and FFI outside the compared workflow forever. PgWire is covered only by scattered hand-written checks (`tests/e2e/pgwire.rs`, `pgwire_singleton.rs`, and the §54.D singleton-parity port `integration_singleton_parity.rs`, which 00171 task 12 is still wiring into `tests/e2e/main.rs` at `8fe51c9`); FFI only by interface-local tests. AGENTS.md's own rule — "Do not call interfaces equivalent unless the same downstream workflow passes conformance tests on each" — means 120% parity stays unfalsifiable for 2 of 6 transports even after the backlog drains. Separately, the promise-level table in `docs/src/technical/transport-policy-inventory.md:22-29` (and its source anchor `docs/src/guide/building-apps.md#choosing-an-interface`) still carries pre-decision tier labels (REST `Specialized`, PgWire qualified "for SQL/reporting", FFI with no promise level) that contradict the settled 2026-07-13 no-specialization decision in `design-decisions.md`.

### Target Users

The maintainer (parity drift becomes a CI failure instead of an audit finding) and downstream app developers relying on "any transport, same behavior" when choosing an interface.

### Success Metrics

- All six transports (CLI, GraphQL, REST, PgWire, FFI, NoSQL HTTP) run the same golden CRUD and validation workflows through the comparator.
- A parity divergence on the PgWire or FFI leg fails `cargo test -p ddb-e2e` (rides the blocking gate 00192 establishes).
- The verb-disposition registry covers every app-contract command struct × six transports; a new command without disposition entries fails the e2e suite by construction.
- Zero remaining pre-decision promise-level labels in the docs; every deliberate divergence is an explicit comparator allowance citing a design decision.

## Functional Decomposition

### Capability: PgWire conformance leg
Drive the golden workflows over the PostgreSQL wire protocol and compare outcomes with the other transports.

#### Feature: PgwireDriver
- **Description**: A conformance driver that executes workflow steps as SQL over a simple-protocol PgWire connection.
- **Inputs**: Harness fixture (repo + `ServerGuard` from `tests/e2e/common.rs`), workflow steps.
- **Outputs**: Canonical step outcomes in the harness result type (`tests/e2e/conformance/result.rs`).
- **Behavior**: Connects with `tokio-postgres` using the existing pattern (`tests/e2e/pgwire.rs:11-19`: token as MD5 password, `NoTls`, `simple_query` only — the extended/prepared protocol is a placeholder in `ddb-server/src/pgwire.rs` and stays out of scope). Maps create/read/update/delete steps to `INSERT`/`SELECT`/`UPDATE`/`DELETE` statements.

#### Feature: PgWire comparison wiring
- **Description**: Include the PgWire leg in `cross_driver_crud` and `cross_driver_validation` comparisons.
- **Inputs**: PgwireDriver outcomes plus the existing legs' outcomes.
- **Outputs**: Pass/fail per workflow step; divergence reports naming the legs.
- **Behavior**: Deliberate divergences (e.g. PgWire's absent warning channel, per the policy map in `transport-policy-inventory.md:42`) are explicit, documented comparator allowances — never silent skips. Removing an allowance must make the test fail, proving it is load-bearing.

### Capability: FFI conformance leg
Drive the same workflows through the embedded `DoogatDriver` and compare with the server-backed transports.

#### Feature: FfiDriver
- **Description**: A conformance driver that executes workflow steps through `ddb-core`'s `DoogatDriver` (`ddb-core/src/ffi/driver.rs`) in-process.
- **Inputs**: Harness fixture repo path (FFI is embedded: it opens the repo directly instead of talking to `ServerGuard`), workflow steps.
- **Outputs**: Canonical step outcomes in the harness result type.
- **Behavior**: ddb-core already belongs to the e2e package. Use create_structured/update_structured from 00177, warning-bearing read_doogat/delete_doogat records from 00175, filtered contract search from 00176, and execute_sql; never use removed raw CRUD exports or drop warnings while normalizing outcomes.

#### Feature: FFI comparison wiring
- **Description**: Include the FFI leg in the cross-driver comparisons.
- **Inputs**: FfiDriver outcomes plus the existing legs' outcomes.
- **Outputs**: Pass/fail per workflow step.
- **Behavior**: Embedded-vs-server differences (no HTTP status, no auth step) are encoded as explicit allowances tied to the transport-owned rows of the policy map, same rule as the PgWire leg.

### Capability: Verb-disposition registry
The mechanical check that makes parity skips explicit rows instead of review archaeology.

#### Feature: Command × transport disposition table
- **Description**: one registry maps every app-contract command struct (create/read/update/delete/apply-schema/search — the current `app_contract/commands.rs` set) to its disposition on each of the six transports: Guaranteed / Specialized / Intentionally absent, each non-Guaranteed row citing its policy-map row or design decision.
- **Inputs**: the command-struct list; the transport-policy inventory; the comparator's allowance list.
- **Outputs**: a registry (a table in the conformance harness, mirrored into `transport-policy-inventory.md`) plus a completeness test.
- **Behavior**: the test enumerates the command structs and fails when any command lacks a disposition entry for any of the six transports — adding a new contract command without deciding its per-transport disposition breaks `cargo test -p ddb-e2e`; comparator allowances and registry rows reference each other so neither can drift silently.

### Capability: Promise-level re-sync
Make the public docs state what the no-specialization decision settled.

#### Feature: Docs re-sync
- **Description**: Update the promise-level table and its guide anchor so all six interfaces read CRUD `Guaranteed`, keeping PgWire's non-transactional-semantics note.
- **Inputs**: `docs/src/technical/transport-policy-inventory.md`, `docs/src/guide/building-apps.md#choosing-an-interface`, the design-decisions no-specialization entry.
- **Outputs**: Docs where the only per-interface differences are recorded, cited allowances.
- **Behavior**: Premise: some rows (REST `Specialized`, NoSQL `Intentionally absent`) may already be updated by 00175/00192 by the time this runs — re-check at execution and touch only rows still stale; a failed premise on a row means skip that row and report, never force.

## Structural Decomposition

### Repository Structure

```
tests/
└── e2e/conformance/                    # Maps to: all three capabilities (existing at HEAD: mod.rs, args.rs, comparator.rs, fixture.rs, result.rs, setup.rs, step_refs.rs, workflows.rs, driver_cli.rs + driver_cli_test.rs, driver_graphql.rs + driver_graphql_test.rs; tests/Cargo.toml already depends on ddb-core)
    ├── mod.rs                          # + driver_pgwire, driver_ffi wiring
    ├── driver_pgwire.rs                # Maps to: PgwireDriver
    ├── driver_pgwire_test.rs           # driver-level tests (pattern: driver_cli_test.rs)
    ├── driver_ffi.rs                   # Maps to: FfiDriver
    ├── driver_ffi_test.rs              # driver-level tests
    ├── cross_driver_crud.rs            # + PgWire/FFI legs
    ├── cross_driver_validation.rs      # + PgWire/FFI legs
    └── disposition_registry.rs         # Maps to: Verb-disposition registry (table + completeness test)
docs/src/
├── technical/transport-policy-inventory.md   # Maps to: Docs re-sync; mirrors the disposition registry
└── guide/building-apps.md                    # Maps to: Docs re-sync
```

### Module: driver_pgwire
- **Maps to capability**: PgWire conformance leg
- **Responsibility**: Translate workflow steps to simple-protocol SQL and canonicalize outcomes.
- **Exports**:
  - `PgwireDriver` - concrete driver struct following the `CliDriver`/`GraphqlDriver` pattern (the harness uses concrete structs, no shared trait)

### Module: driver_ffi
- **Maps to capability**: FFI conformance leg
- **Responsibility**: Execute workflow steps on an in-process `DoogatDriver` and canonicalize outcomes.
- **Exports**:
  - `FfiDriver` - concrete driver struct

### Module: cross_driver extensions
- **Maps to capability**: PgWire + FFI conformance leg, Promise-level re-sync (gate)
- **Responsibility**: Run all legs against the shared fixture and compare via `comparator.rs`; hold the explicit allowance list.
- **Exports**: extended `cross_driver_crud` / `cross_driver_validation` test fns

### Module: disposition registry and policy documentation (tests/e2e/conformance/; docs/src/technical/transport-policy-inventory.md, conformance-harness.md; docs/src/guide/building-apps.md)
- **Maps to capability**: Verb-disposition registry; Promise-level re-sync
- **Responsibility**: enumerate all command structs across six interfaces, require rationale for specialized rows, and keep published promises aligned.
- **Exports**: the complete disposition table, omission-failure test, and matching documentation.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **driver_pgwire**: PgWire leg driver (reuses existing `tokio-postgres` dep and `ServerGuard`)
- **driver_ffi**: FFI leg driver (`ddb-core` is already a dep of `tests/Cargo.toml`)

### Core Layer (Phase 1)
- **cross_driver extensions**: Depends on [driver_pgwire, driver_ffi]

### Integration Layer (Phase 2)
- **docs re-sync + gate confirmation**: Depends on [cross_driver extensions]

## Implementation Phases

### Phase 0: Foundation
**Goal**: Both new drivers execute the golden workflows standalone.

**Tasks**:
- [ ] Implement `PgwireDriver` + driver-level tests (no deps) - Acceptance: driver runs the CRUD workflow steps against `ServerGuard` via `simple_query` and produces canonical outcomes.
- [ ] Implement `FfiDriver` + driver-level tests (no deps) - Acceptance: driver runs the same steps in-process through structured create/update and record-returning read/delete; tests pin BaseOnly warnings, broken-backlink warnings, filters, and every canonical result field.

**Exit Criteria**: Both drivers pass their driver-level tests standalone.

### Phase 1: Core
**Goal**: Six-legged comparison with explicit allowances.

**Tasks**:
- [ ] Wire both drivers into `cross_driver_crud` and `cross_driver_validation` (depends on: Phase 0) - Acceptance: comparisons run all available legs; every deliberate divergence is an explicit allowance entry citing a policy-map row or design decision; no silent skips.
- [ ] Allowance load-bearing test (depends on: Phase 0) - Acceptance: a test proves that removing an allowance turns the comparison red.
- [ ] Build the verb-disposition registry: one table mapping each app-contract command struct → disposition per six transports, with a completeness test that fails when any command lacks entries; non-Guaranteed rows cite their policy-map row or design decision (depends on: Phase 0) - Acceptance: the registry covers every command struct in app_contract/commands.rs, explicitly including IndexHealthCommand (00181) and SetRemoteCommand/GetRemoteCommand/SyncCommand/FetchCommand/PushCommand (00191); deleting a row or adding a new command struct without entries fails `cargo test -p ddb-e2e`; the table is mirrored into `transport-policy-inventory.md`.

**Exit Criteria**: `cross_driver_crud` and `cross_driver_validation` run six-legged and green.

### Phase 2: Integration
**Goal**: Docs match the decision; drift blocks.

**Tasks**:
- [ ] Re-sync promise-level docs, including `docs/src/technical/conformance-harness.md` (module-structure table gains the two driver rows; the "FFI driver — deferred" / "PgWire driver — deferred" bullets at `:130` are resolved) (depends on: Phase 1) - Acceptance: all six interfaces read CRUD `Guaranteed`; PgWire states FT-6's Shared cross-request transaction rejection and autocommit behavior, while embedded FFI retains Exclusive transactions. Re-check at execution whether 00206's isolated sessions have shipped before describing session support; neither this PRD nor 00180 lifts FT-6's guard. `conformance-harness.md` lists both drivers and no longer defers them; premise re-checked per row (already-correct rows are skipped and reported).
- [ ] Confirm blocking behavior (depends on: Phase 1) - Acceptance: an injected PgWire/FFI divergence fails `cargo test -p ddb-e2e` under the 00192 gate; revert the injection.

**Exit Criteria**: Same downstream workflow passes conformance on each of the six transports, per the AGENTS.md equivalence rule.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: golden CRUD workflow across six legs → Expected: identical canonical outcomes (modulo explicit allowances).
- **Edge case**: PgWire warning-channel divergence → Expected: covered by a named allowance; deleting the allowance fails the run.
- **Error case**: validation workflow (bad input) → Expected: same canonical error code on every leg (post-00173 single error table; PgWire surfaces it as the PG error string derived from that table).

## Risks

- **`ddb-core` is already linked into `ddb-e2e`** (since 00169, `tests/Cargo.toml:15`), so the FFI driver adds no new build cost; the e2e suite stays Tier 2 (CI-executed, not per-task).
- **PgWire can only speak simple protocol today**: the driver intentionally uses `simple_query`; the extended/prepared protocol is a separately-deferred roadmap item and NOT re-scoped here.
- **Allowance list balloons into de-facto specialization**: every allowance must cite a policy-map row or design decision; an uncited allowance is a review-blocking finding.

## Transport Impact

Test-and-docs only; no runtime behavior changes on any transport. CLI/GraphQL/REST/NoSQL legs already exist or arrive via 00175/00192; this PRD adds the PgWire and FFI legs, completing the cross-interface conformance plan that AGENTS.md requires. Docs: promise-level table re-sync as scoped above.
