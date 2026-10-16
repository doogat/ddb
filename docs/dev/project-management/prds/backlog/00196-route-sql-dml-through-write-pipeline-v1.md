---
catchup: force
default_model: opus
model_tier_rationale: equivalence obligation - the write-plan extraction must be byte-identical for the service verbs
---

# Route SQL DML through the unified core write pipeline

**Priority:** P2 (leverage; ends a recurring bug class) · **Roadmap:** appended 2026-07-16 · **Depends on:** 00175 (read/delete routing), 00176 (search routing), 00177 (FFI CRUD), 00178 (structured fields) — the contract write plan must exist before DML compiles into it. Sequence WITH or AFTER 00183's dml.rs split (replan-floor lesson from 00156). (`catchup: force`: runs after the 00183 splits, so batch-cached catchup and this PRD's line refs will be stale — re-ground at execution.)

Source: `dev/local/audit-results/evolution-assessment-2026-07-16.md` F4 (lens 7, commit-history mining).

## Overview

### Problem Statement

Typed-write semantics (validation, FK/allowed_values, auto-junction sync, materialization, singleton enforcement) are re-implemented per entry path. `sql_engine/dml.rs` (INSERT/UPDATE/DELETE) is a parallel pipeline beside `DoogatService` create/update/delete. History shows the class: the same junction-sync bug was fixed separately on SQL INSERT (`be086e2`), SQL UPDATE (`f231e97`), service unset (`da9846e`), and FFI raw (`f26beae`, `143b58e`); "route typed X through unified helper" recurs per transport (`6e3f667`, `7b002e7`). Every new write-semantic feature ships the same bug N times. The contract block (00175-00178, which lands before this PRD) routes *transports* through the core; nothing routes the SQL-engine DML leg through the same write plan (grounded 2026-09-05: `dml.rs` runs its own `commit_files`/`commit_file`/`index_doogat`/`populate_junction_tables`/`materialize_single` legs and never calls `nosql_index_doogat`, while the service reaches junctions through `materialize_single` — the divergence is real). (`prepare_typed_insert_validate` unified validation only; the write/materialize/junction legs still fork.)

### Target Users

Platform maintainers (every write-semantic feature currently costs N implementations); downstream apps using SQL over GraphQL/PgWire.

### Success Metrics

- One core write plan (validate → git write → materialize → junctions) executed by BOTH `DoogatService` verbs and `sql_engine` DML handlers.
- A new write-semantic rule added in one place is enforced on service create, SQL INSERT, and SQL UPDATE by construction — pinned by one three-entry test.
- No public behavior change: existing sql_engine tests and e2e stay green; `dml.rs` sheds its duplicated write legs (measurable LOC drop).

## Functional Decomposition

### Capability: One core write plan

#### Feature: Shared write-plan extraction
- **Description**: the shared write sequence (validate → git write → materialize → junctions → singleton enforcement) extracted into one core function/struct consumed by the service verbs.
- **Inputs**: the validated column values / parsed content the service write helpers already produce.
- **Outputs**: the committed doogat + index/materialization side effects, identical to today's service-verb behavior.
- **Behavior**: pure refactor of `service/write_helpers.rs` (or a new `write_plan.rs`): the existing `service/{create,update,delete,batch}.rs` verbs consume the extracted plan; behavior byte-identical, existing tests are the oracle.

#### Feature: DML compiles into the plan
- **Description**: `sql_engine/dml.rs` INSERT/UPDATE/DELETE handlers execute their post-validation legs through the shared plan instead of their own copies.
- **Inputs**: parsed SQL statements; SQL-specific pieces (expression evaluation, WHERE resolution, `DEFAULT NEXT` counters) stay in sql_engine and feed plan inputs.
- **Outputs**: the same SQL results (`SqlResult`) as today; the duplicated write legs in `dml.rs` deleted.
- **Behavior**: `handle_insert`/`handle_update`/`handle_delete` build the same plan inputs the service verbs build, then execute the shared plan; transactional DDL buffering (00161) keeps its explicit boundary around the plan's commit step.

### Capability: Divergence inventory

#### Feature: Write-semantics rule × entry-path table
- **Description**: a Phase 0 inventory naming every write-semantic rule and where each entry path enforces it today.
- **Inputs**: `service/{create,update,delete,batch}.rs`, `sql_engine/dml.rs`, `sql_engine/typed_insert.rs`, the FFI paths.
- **Outputs**: a table (workdir note) of rule → enforcement point per path, divergences highlighted.
- **Behavior**: read-only analysis; the table drives which legs Phase 1/2 unify and proves nothing is silently dropped.

## Structural Decomposition

### Repository Structure

```text
ddb-core/src/service/
├── write_helpers.rs (or write_plan.rs)  # Maps to: Shared write-plan extraction
└── {create,update,delete,batch}.rs      # Maps to: Shared write-plan extraction (consumers)
ddb-core/src/sql_engine/
├── dml.rs (or dml/ post-00183 split)    # Maps to: DML compiles into the plan
└── typed_insert.rs                      # Maps to: Divergence inventory (validation chokepoint, unchanged)
```

### Module: write plan (ddb-core/src/service/write_helpers.rs or write_plan.rs)
- **Maps to capability**: One core write plan
- **Responsibility**: own the validate→commit→materialize→junction sequence once.
- **Exports**: the plan entry point the service verbs and DML handlers share (exact signature decided in design; no adapter types).

### Module: dml (ddb-core/src/sql_engine/dml.rs)
- **Maps to capability**: One core write plan
- **Responsibility**: SQL parsing/evaluation only; write execution delegated to the plan.
- **Exports**: `handle_insert`/`handle_update`/`handle_delete` — unchanged signatures, thinner bodies.

### Module: service consumers and validation inventory (ddb-core/src/service/create.rs, update.rs, delete.rs, batch.rs; sql_engine/typed_insert.rs)
- **Maps to capability**: One core write plan; Divergence inventory
- **Responsibility**: consume the shared plan and preserve each verb's required semantics, including delete backlinks and transaction boundaries.
- **Exports**: existing service verbs and validation helpers; no concrete adapter construction.

## Dependency Graph

### Foundation Layer (Phase 0)
Depends on 00175-00178 (contract block landed).
- **divergence inventory**: names the legs to unify before code moves.

### Core Layer (Phase 1)
- **write-plan extraction**: depends on [inventory] - pure refactor, service verbs consume it.
- **INSERT leg**: depends on [write-plan extraction].

### Integration Layer (Phase 2)
- **UPDATE/DELETE legs + docs**: depends on [INSERT leg].

## Implementation Phases

### Phase 0: Foundation
**Goal**: the divergences are named before code moves.

**Tasks**:
- [ ] Build the write-semantics rule × entry-path inventory table (service create/update/delete/batch, SQL INSERT/UPDATE/DELETE, FFI execute_sql and structured CRUD) with today's enforcement point for each (no deps) - Acceptance: every rule the tests pin has a row; divergences are explicitly listed; the table lives in the PRD workdir notes.

**Exit Criteria**: inventory complete; Phase 1 scope is a checklist, not archaeology.

### Phase 1: Core
**Goal**: one plan; INSERT rides it.

**Tasks**:
- [ ] Extract the shared write plan from the service write helpers (pure refactor); route `service/{create,update,delete,batch}` through it (depends on: Phase 0) - Acceptance: existing service tests green, including delete-specific backlink/report behavior and transaction boundaries; moved logic diffed the 00156 way (byte-equivalent bodies).
- [ ] Route `handle_insert`'s post-validation legs (git write, materialize, junctions, singleton) through the plan; delete the superseded `dml.rs` copies (depends on: extraction) - Acceptance: existing INSERT tests green; a new three-entry test proves one added rule fires on service create AND SQL INSERT; `dml.rs` LOC drops.

**Exit Criteria**: INSERT and service writes share one plan; three-entry test green for create+INSERT.

### Phase 2: Integration
**Goal**: UPDATE/DELETE ride it; documented.

**Tasks**:
- [ ] Route `handle_update`/`handle_delete` legs through the plan; delete their superseded copies; extend the three-entry test to SQL UPDATE (depends on: Phase 1) - Acceptance: existing UPDATE/DELETE + transactional-DDL tests green; the three-entry test covers create/INSERT/UPDATE.
- [ ] Update `docs/src/technical/sql-engine.md` + `architecture/modules.md`; apply CLAUDE.md's conditional e2e deletion safety net in Claude-routed sessions (deletes/replaces code paths per CLAUDE.md); CHANGELOG only if any behavior changed (target: none) (depends on: Phase 1) - Acceptance: docs match; e2e green.

**Exit Criteria**: Tier 1 + e2e green; duplicated write legs gone.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: existing sql_engine INSERT/UPDATE/DELETE suites pass unchanged against the plan-routed handlers.
- **Edge case**: the three-entry rule test — one write-semantic rule added once fires on service create, SQL INSERT, and SQL UPDATE.
- **Error case**: transactional DDL (00161) COMMIT failure inside a plan-routed write → same rollback/cleanup behavior as today (pinned by the existing transaction tests).

## Risks

- **dml.rs size**: 1663 prod lines pre-split. Mitigation: sequence WITH or AFTER 00183's dml split so the diff stays reviewable; the dependency header says so.
- **Transactional DDL interaction**: the buffer boundary around the plan's commit step must stay explicit. Mitigation: the existing 00161 transaction tests are the oracle; the boundary is named in the plan's design.
- **Hidden semantic differences between the legs**: the Phase 0 inventory exists precisely to surface them before unification; any genuine behavioral divergence found becomes a named decision, not a silent pick.

## Transport Impact

SQL behavior is preserved on GraphQL executeSql, PgWire, CLI ddb query, and FFI execute_sql. REST/NoSQL CRUD consumes the same service plan but has no separate SQL endpoint. Cross-interface conformance plan: existing SQL regressions cover all four SQL-bearing interfaces; the three-entry rule test pins shared enforcement, and delete-specific behavior remains covered.
