# Declarative typedef schema apply (diff + migrate)

## Overview

### Problem Statement
Schema evolution is imperative only. Downstreams hand-write ordered `CREATE`/`ALTER` sequences plus their own idempotency and detection logic; there is no diff/apply path (no `declarative`/`schema diff`/`apply` symbols in ddb-core or ddb-server — evolution is sqlparser DDL dispatch in `ddb-core/src/sql_engine/ddl.rs`). Every downstream re-invents "make the live schema match this desired shape." This is the single largest source of downstream boilerplate and the clearest lever for the "flexible backend that makes downstream trivial" goal. **Strategic epic — sequence after 00158/00159/00160.**

### Target Users
Every downstream that manages a schema. This is the headline capability for the program's investor narrative.

### Success Metrics
- A downstream declares its typedefs once — a schema document of columns, zones, indexes, search keys, and singleton flags — and calls a single apply.
- ddb diffs desired-vs-live and performs the minimal safe migration atomically (add / alter / rename / drop columns; zone / index / search-key / singleton settings), with a dry-run plan.
- jink's imperative `src/schema.rs` machinery (ordered `CREATE TABLE` + `MIGRATIONS` + `DDB_MIGRATIONS` + a 265-LoC hand-rolled data migration) collapses to one declared schema + apply call.

Motivating downstream evidence (jink): `src/schema.rs` is ~120 LoC of ordered DDL plus a 265-LoC non-atomic data migration — `install_types_via_api` / `run_migrations` / `run_data_migrations` — all encoding a desired schema by hand.

## Functional Decomposition

### Capability: Desired-schema representation
#### Feature: Schema document
A declarative description of typedefs (columns, zones, indexes, search keys, singleton flags), as an extension of the existing typedef YAML or a new apply payload.

### Capability: Diff + plan
#### Feature: Dry-run differ
Compute the ordered DDL plan from desired-vs-live without mutating; surface it for review.

### Capability: Apply
#### Feature: Transactional apply
Execute the plan atomically (add/alter/zone/index/search-key/singleton, then rename/drop), idempotent on re-apply.

## Structural Decomposition

### Repository Structure
New greenfield `schema-diff` module in ddb-core plus a thin transport surface (GraphQL mutation or CLI subcommand).

### Module: ddb-core/schema-diff (new)
Desired-schema model + differ; emits ordered DDL reusing the now-shipped `RENAME` (`ddl.rs:979`, `:1192`), `SET ZONE`, `SET SEARCH KEY`, `SET SINGLETON`, and index DDL.

### Module: transport (GraphQL or CLI)
`applySchema` / `ddb schema apply` with `--dry-run` plan output.

## Dependency Graph

### Foundation Layer (Phase 0)
Desired-schema representation + dry-run differ (plan only).

### Core Layer (Phase 1)
Apply for additive/alter operations.

### Integration Layer (Phase 2)
Rename/drop via `RENAME`; transactional apply; plan introspection.

## Implementation Phases

### Phase 0: Foundation
Define the desired-schema representation. Build a differ that maps desired -> current and emits an ordered DDL plan, surfaced as a dry-run (no mutation).

### Phase 1: Core
Apply additive and alter operations: add columns, zone changes, indexes, search keys, singleton settings.

### Phase 2: Integration
Column rename/drop via the shipped `RENAME` DDL with data-preserving moves; wrap apply in a transaction with idempotency; expose the plan for introspection.

## Test Strategy

### Critical Scenarios
- Apply to an empty repo (= CREATE all typedefs).
- Apply a no-op (fully idempotent).
- Added, removed, and renamed column.
- Zone / search-key / singleton changes.
- Failure mid-apply rolls back cleanly.

## Risks
- Large scope — explicitly an epic; sequence after the tactical query/DDL PRDs.
- Safe diffing of destructive ops (drop/rename) needs guardrails (confirm / plan-gate).
- Interaction with the existing imperative DDL path.
- Schema-document versioning and migration of the document format itself.

## Transport Impact
New apply surface (a GraphQL mutation or a `ddb schema apply` CLI subcommand) plus dry-run plan output. Existing imperative DDL paths remain unchanged and continue to work alongside.
