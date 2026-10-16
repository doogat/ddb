---
catchup: force
design: run
default_model: opus
model_tier_rationale: concurrency and cross-interface contract changes; snapshot ownership, commit races, expiry, and six-interface equivalence determine correctness
---

# Per-connection transaction sessions

## Overview

### Problem Statement
FT-6 deliberately rejects cross-request SQL transaction verbs on the Shared server actor. Removing that guard alone would let one client's transaction capture another client's writes. `executeBatch` already buffers transaction-aware operations within one request; this PRD adds ownership across round trips, not a second batch API.

Sources: `dev/local/plans/fast-track-2026-09-06.md` S3 Job C and D1a; `docs/src/architecture/design-decisions.md` (PgWire planned evolution); `dev/local/reviews/stash-recovery-independent-review-2026-09-13.md`. Grounded at `7cea5e3`, 2026-09-13:
- `ddb-core/src/sql_engine/transaction.rs`: `SAVEPOINT ddb_txn` is connection-local; commit flushes buffered files through `commit_batch` without validating their read baseline.
- `ddb-core/src/service/sql.rs`: Shared guards and detach backstop; Exclusive retains the caller's transaction. `sql_engine/mod.rs`: implicit batch transaction and buffered read-your-writes.
- `ddb-server/src/pgwire.rs` and `actor/mod.rs`: connections dispatch to one service. `service/{create,update,delete,batch}.rs`: structured CRUD still has direct commit paths; a SQL-only session would miss the platform workflow.
- `docs/src/technical/sql-engine.md#transactions`: CREATE and supported column/typedef ALTER buffer; DROP TABLE and table rename commit immediately. SQLite rollback cannot undo a committed Git write.

### Target Users
Downstream applications using any public interface that must read, make a decision, and commit several CRUD changes across calls while other clients continue working.

### Success Metrics
- `transaction_sessions_golden_crud` passes the same isolation/commit/rollback workflow on CLI, GraphQL, REST, PgWire, FFI, and NoSQL HTTP; no absent-driver skips.
- `transaction_sessions_conflicting_commit_preserves_winner` and `transaction_sessions_cross_process_commit_race` prove that a stale transaction cannot overwrite a committed edit or partially commit.
- `transaction_sessions_expiry_releases_resources` and `transaction_sessions_disconnect_isolated` prove deterministic cleanup with no Git write from abandoned work.
- Existing Exclusive transaction, request-local batch, cascade, constraint, warning, and Shared no-owner guard regressions retain their original assertions.

## Functional Decomposition

### Capability: Owned transaction workflow
One adapter-neutral contract owns lifecycle and CRUD semantics before transports are changed.

#### Feature: Session commands and results
- **Description**: Define begin, session-bound command execution, commit, and rollback through app-contract command structs, `DoogatService`, and `AppOutput`.
- **Inputs**: Repository identity, authenticated server context or embedded owner, session handle, and existing CRUD/SQL command inputs.
- **Outputs**: Core-defined session identity/lifecycle results, existing command results and warnings, and a commit outcome that distinguishes committed work from discarded work and an unavailable outcome after a lost connection.
- **Behavior**: A handle selects exactly one owner/repository/session; possession does not replace server authentication. Missing, foreign, expired, or completed handles never fall back to autocommit. Serialize commands for the same session. Reject nested BEGIN and unsupported transaction controls without disturbing other sessions. Define wire encodings, error codes, warning fields, and lifecycle transitions in the reviewed design and executable contract fixtures before adapter implementation; use the existing command-struct convention, not an invented AppCommand enum.

#### Feature: Transaction-aware CRUD and golden workflow
- **Description**: Stage structured CRUD and SQL DML through the same core write plan, including validation, references, cascades, and warnings.
- **Inputs**: Existing create/read/list/update/delete commands or corresponding SQL inside an owned session; a second independent client.
- **Outputs**: Read-your-writes results for the owner; committed-only results for outsiders; one Git commit for a nonempty successful transaction and none for rollback or an empty transaction.
- **Behavior**: Golden workflow: A begins, creates P/C with CASCADE and reads them, updates P, deletes C, and reads/lists its staged result across separate calls; B sees none of A's uncommitted changes. A commits; B sees the final state and Git advances once. Repeat with rollback; B and Git stay unchanged. Run a mirrored workflow with B writing while A is open to prove ownership does not absorb B's work. Preserve UNIQUE/FK/singleton rules and structured warnings; statement failures restore that statement's staged/index effects and leave explicit rollback available.

### Capability: Isolated state and safe commit
Git remains authoritative; session-local SQLite state is disposable.

#### Feature: Private session snapshot
- **Description**: Give each session its own SQLite connection, transaction buffer, and coherent read baseline.
- **Inputs**: A captured Git HEAD/tree and an injected factory for a private derived index and snapshot reader.
- **Outputs**: An isolated view of that baseline plus the session's staged changes.
- **Behavior**: Capture file bytes, schemas, index contents, and conflict baseline from the same committed tree. Reads, predicate evaluation, and buffer fallback use that tree, not moving HEAD. A separate connection to the shared writable index is insufficient: open session writes must neither block unrelated clients for its lifetime nor expose staged data. Use a private derived index; rollback/release may affect only that session. No repo write lock, shared SQLite write transaction, or actor exclusivity is held between requests. Concrete adapters are constructed at composition roots and injected; shared types contain no adapter dependencies.

#### Feature: Conflict-checked all-or-nothing commit
- **Description**: Validate the read baseline at the commit boundary before publishing any staged files.
- **Inputs**: Session baseline, staged writes/deletes, and fresh repository state under the existing repo-scoped write lock.
- **Outputs**: A durable commit result or a structured conflict with no transaction writes applied.
- **Behavior**: V1 rejects COMMIT if fresh HEAD differs from its BEGIN baseline, including an unrelated commit; discard the stale buffer and require a fresh transaction for retry. Maintainer confirmed this policy on 2026-09-13. This covers changed rows, deleted dependencies, new predicate matches, schema changes, and uniqueness races without inventing incomplete read-set validation. Compare and publish within one write-lock acquisition, build from fresh state, and retain existing lock order. Read-only/empty transactions also validate the baseline; if unchanged, release the session without a Git write. Never use 00195's partial-unit skip semantics for a transaction; either every operation commits or none does. Never auto-replay business commands after conflict or an uncertain network outcome.

### Capability: Bounded lifecycle and transport integration
Connection ownership and token ownership adapt the same core session policy.

#### Feature: Expiry, disconnection, and resource bounds
- **Description**: Release abandoned session state without blocking unrelated clients or publishing buffered data.
- **Inputs**: Owner close/disconnect, shutdown/restart, idle clock, explicit rollback, and configured limits for lifetime, active sessions, private-index/buffer resources, and operation/lock wait.
- **Outputs**: Discarded uncommitted state, released resources, stable lifecycle errors, and redacted diagnostics.
- **Behavior**: PgWire disconnect and embedded owner drop end that owner's session. HTTP request completion/socket reuse does not end token sessions; explicit rollback and idle expiry do. Sessions are ephemeral across process restart. Same-session execution, expiry, and commit are ordered so cleanup cannot race a successful publication. Set finite defaults and maxima in the design, expose validated configuration and expiry information, and pin exact-limit/over-limit behavior with injected time rather than sleeps. Resource exhaustion refuses new work without evicting an unrelated active session. No raw session/auth tokens, SQL values, or private paths in public errors or logs.

#### Feature: Commit visibility, recovery, and compatibility
- **Description**: Publish committed changes and honest outcomes while preserving existing non-session APIs.
- **Inputs**: Git commit result, index refresh/event result, session close/cancellation, and existing clients.
- **Outputs**: Core commit result/warnings, committed schema/data visibility, and migration/support guidance.
- **Behavior**: After Git publication, index failure is a committed outcome needing index repair, never a claim that rollback removed the write; carry the durable commit identity and warning through the core result. Refresh/invalidate shared derived reads and emit mutation/schema events only for committed changes. A response lost during process failure can remain unknown: document reconciliation against committed repository state and forbid automatic replay; no durable session journal or new outcome-query API is required. Preserve legacy Exclusive transactions and tokenless autocommit/batches. In the new session workflow, refuse operations that bypass transactional staging, including DROP TABLE/table rename, before their first side effect; preserve legacy batch limits. Existing buffered CREATE/supported ALTER must stage privately if exposed; no uncommitted schema reload. Maintenance/sync/bundle operations do not become transaction verbs.

## Structural Decomposition

### Repository Structure
```text
ddb-core/src/
├── app_contract/{commands,results,output,error}.rs  # Owned transaction workflow
├── service/{mod,sql,write_helpers}.rs              # Workflow integration; post-00196 write-plan home
├── service/transaction_sessions.rs                # Lifecycle/use cases (new, private module)
├── sql_engine/{mod,transaction,delete}.rs          # Snapshot/buffer consumers
├── indexer/                                      # Private derived-index adapter/factory
├── git_ops/                                      # Snapshot reads; guarded commit boundary
├── traits.rs                                     # Injected session storage/clock ports
└── ffi/driver.rs                                 # Embedded transport
ddb-server/src/{actor/,pgwire.rs,rest.rs,nosql_api.rs,schema/,auth.rs,config.rs}
ddb-cli/src/{main.rs,commands/}                    # Persistent CLI workflow
tests/e2e/{conformance/,integration_transaction_sessions.rs,smoke_transaction_sessions.rs,main.rs}
docs/src/{guide/,technical/,architecture/}         # Contract, limits, migration and module guidance
```

### Module: app_contract
- **Maps to capability**: Owned transaction workflow; Bounded lifecycle and transport integration.
- **Responsibility**: Own command/result/error/warning definitions and the lifecycle contract once.
- **Exports**: Session command structs and result types through the existing app-contract module; extend its one error policy.

### Module: session use cases and write-plan consumers
- **Maps to capability**: Owned transaction workflow; Isolated state and safe commit; Bounded lifecycle and transport integration.
- **Responsibility**: Select owned state, stage both CRUD and DML, serialize lifecycle operations, and publish only through guarded commit.
- **Exports**: Service session operations and injected storage/clock interfaces; no transport policy or concrete adapter construction.

### Module: git_ops and indexer session adapters
- **Maps to capability**: Isolated state and safe commit.
- **Responsibility**: Read a pinned Git tree, own a private derived index per session, and compare/publish under the repo lock.
- **Exports**: Snapshot/private-index factory and all-or-nothing guarded commit implementation behind core ports; no second lock protocol.

### Module: transport adapters and conformance
- **Maps to capability**: Bounded lifecycle and transport integration; Owned transaction workflow.
- **Responsibility**: Translate native ownership and commands into the core contract, compare the same workflow, and document operation/setup/diagnostics.
- **Exports**: Six public bindings and conformance drivers using core results, error classification and warnings without field subsets.

## Dependency Graph

### Foundation Layer (Phase 0)
Execution prerequisites: 00172 closure actor; 00173 shared error policy; 00175-00178/00197 contract routing/results; 00180 server hardening; 00192 NoSQL CRUD; 00193 all-six conformance; 00196 common write pipeline. These remain separate PRDs; do not absorb them into 00206. Re-check capabilities at execution rather than trusting moved file paths. FT-6/FT-3 and the write lock are already shipped. 00195 is related, not a prerequisite: its partial-skip contract is different.
- **contract fixtures/design**: Depends on [prerequisite capabilities]; fixes the workflow, lifecycle, encodings and bounded configuration before code moves.

### Core Layer (Phase 1)
- **snapshot/private-index adapters**: Depends on [contract fixtures/design].
- **guarded commit**: Depends on [snapshot/private-index adapters, existing repo write lock].
- **session use cases and shared write-plan consumers**: Depends on [guarded commit, contract fixtures/design].

### Integration Layer (Phase 2)
- **six transport adapters**: Depends on [session use cases, core results/errors, closure actor].
- **conformance/release guidance**: Depends on [all six transport adapters, existing conformance drivers].

## Implementation Phases

### Phase 0: Foundation
**Goal**: Freeze the executable downstream contract before refactoring.
**Tasks**:
- [ ] Re-ground prerequisites and the Shared guard, private-index needs, current CRUD commit paths and DDL bypasses (no in-PRD deps) - Acceptance: an execution-time inventory references symbols/tests and records satisfied prerequisites; if a required capability is absent, stop this PRD and report that dependency, never force or silently absorb another PRD.
- [ ] Define lifecycle/results, native bindings, auth/setup, finite configuration defaults/maxima, commit uncertainty, and golden fixtures in the reviewed design and `guide/app-contract.md` (depends on: inventory) - Acceptance: `transaction_sessions_contract_state_matrix`, `transaction_sessions_config_bounds`, and wire-schema fixtures cover every lifecycle transition and six mappings; all choices are encoded before runtime implementation with no mid-implementation maintainer decisions.
**Exit Criteria**: Contract fixtures expose required behavior and unsupported operations explicitly; shared guard remains active.

### Phase 1: Core
**Goal**: Owned snapshots and safe publication work through one core path.
**Tasks**:
- [ ] Implement private snapshot/index adapters and ownership/lifecycle use cases (depends on: Phase 0) - Acceptance: colocated `transaction_sessions_private_snapshot`, `transaction_sessions_expiry_releases_resources`, and `transaction_sessions_resource_limits` pin stable read-your-writes, outsider progress, no shared savepoint damage, boundary expiry and resource release.
- [ ] Add guarded publication and integrate structured CRUD/SQL staging (depends on: private snapshots) - Acceptance: `transaction_sessions_conflicting_commit_preserves_winner`, `transaction_sessions_unrelated_commit_conflicts`, `transaction_sessions_predicate_and_schema_conflicts`, `transaction_sessions_readonly_commit_no_write`, and `transaction_sessions_crud_staging` check Git OIDs, contents, indexes and warnings; conflicts publish no partial state. Preserve the existing cascade planner and constraint regressions.
- [ ] Wire commit outcome, cancellation, DDL refusal and shared read/event refresh (depends on: guarded publication) - Acceptance: fault-injected `transaction_sessions_commit_index_failure_reports_commit`, `transaction_sessions_expiry_commit_race`, `transaction_sessions_ddl_guard`, and `transaction_sessions_committed_visibility` prove truthful durable outcomes and no staged events/schema leakage.
**Exit Criteria**: Tier 1 passes with same-process deterministic race tests; no interface bypasses the contract.

### Phase 2: Integration
**Goal**: Ship the same supported workflow on all six interfaces together.
**Tasks**:
- [ ] Adapt CLI/FFI and GraphQL/REST/PgWire/NoSQL to the core operations (depends on: Phase 1) - Acceptance: each transport's colocated tests exercise ownership, auth, state/result serialization, errors/warnings, and teardown; `transaction_sessions_unowned_guard_preserved` keeps tokenless Shared calls protected. New verbs use the closure actor, not per-verb actor enums.
- [ ] Extend all-six drivers and author registered `smoke_transaction_sessions` and `integration_transaction_sessions` scenarios plus a showboat walkthrough (depends on: adapters) - Acceptance: `transaction_sessions_golden_crud`, `transaction_sessions_cross_transport_isolation`, `transaction_sessions_cross_process_commit_race`, and `transaction_sessions_disconnect_isolated` run in Tier 2 with every driver required; no GraphQL detour for PgWire or CLI subprocess scaffolding hidden in tests.
- [ ] Update transaction guidance, server/FFI/setup docs, module table, `technical/walkthrough.md`, transport policy/registry, CHANGELOG and migration examples (depends on: conformance authoring) - Acceptance: documentation shows each native begin/CRUD/commit/rollback workflow, retry/unknown-outcome handling, DDL restrictions, config/defaults and support diagnostics; `mdbook build docs` passes. Premise: old Shared rejection/PgWire policy may still be documented; re-check each passage and change only where session conformance now proves support, skip/report already-correct text. Include the stale broad DDL claim in `technical/server.md`'s batch example. Commit and push master after required gates; claim Guaranteed session support only when all-six CI passes.
**Exit Criteria**: Six-interface workflow and regressions pass CI; published guidance matches the implemented contract and rollback boundary.

## Test Strategy
Per implementation task: sequential foreground `cargo +1.89.0 build`, `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings`, and `cargo +1.89.0 test-ci`, including colocated unit regressions named above. The names are required new tests, not claims that tests already exist. Use deterministic barriers and injected clocks/failures, not timing sleeps. Git contents/OIDs are the durability oracle; compare the derived index too. Author cross-process/e2e/conformance/property scenarios for Tier 2 CI; do not execute the heavy battery per task. Retain CLAUDE.md's conditional deletion-path e2e rule only in Claude-routed sessions. All probe repos live in temporary directories. Walkthrough content goes through showboat CLI; verification uses `dev/bin/safe-showboat-verify`.

### Critical Scenarios
- **Happy path**: The golden workflow on six transports produces identical committed content, normalized result/warning fields, and commit counts.
- **Edge case**: Two private sessions coexist while tokenless CRUD commits; neither session captures it, and stale commits fail under the baseline rule, including read-only/empty transactions. Rollback/expiry of one cannot unwind the other.
- **Error case**: Forged/foreign/expired handle, closed connection, restarted process, failed Git write, post-commit index failure, unsupported DDL and commit-time HEAD change all preserve truthful outcomes and unrelated data.

## Risks
- **Snapshot cost and lock starvation**: merely opening two connections to the same writable SQLite file does not isolate writers. Measure private-index setup for the documented <5K-doogat envelope; pin admission, memory and wait limits in configuration tests and publish measurements. No unmeasured latency guarantee or lock held over client think time.
- **Conflict policy**: the conservative HEAD rule deliberately trades extra retries for complete dependency protection; disjoint-write acceptance/read-set tracking is future scope, not an ad hoc optimization.
- **Commit uncertainty**: Git success followed by index/network failure cannot be undone by SQLite rollback. Preserve durable commit identity when available; an unavailable post-restart outcome must be explained and reconciled without blind replay.
- **Scope drift**: no crash-persistent sessions, nested transactions/savepoint API, new isolation-level menu, PgWire extended/prepared protocol, sync-in-transaction, or cross-repository transactions. Runtime PRD execution is separate from this planning task.

## Transport Impact
Target promise: the same CRUD transaction workflow is Guaranteed on every interface once this PRD ships; present-day support is not relabeled in advance.

| Interface | Native ownership and required implementation/conformance |
| --- | --- |
| CLI | One running `ddb` process owns the embedded session across incremental interactive or scripted input/output; extend the existing SQL/command runner with documented begin/CRUD/commit/rollback use. EOF/process exit discards uncommitted state; transactions do not persist across separate invocations. Driver must read intermediate output before sending the next command. Maintainer confirmed this policy on 2026-09-13. |
| GraphQL | Explicit authenticated opaque token across requests; begin/execute/commit/rollback operations adapt core commands, including existing CRUD resolvers. Request/socket completion is not session disconnect. |
| REST | Native lifecycle routes and session context on existing CRUD; same auth, expiry, core results/warnings/errors. No SQL or GraphQL escape hatch required. |
| PgWire | Session owned by the authenticated connection; SQL BEGIN/CRUD/COMMIT/ROLLBACK and protocol transaction status reflect the core lifecycle. Disconnect cleanup is per connection; no shared backend transaction buffer. |
| FFI | Explicit owned handle on the driver/session object, core contract routing and generated Swift/Kotlin bindings; drop/close cleans up. Existing Exclusive SQL/transaction methods retain compatibility. |
| NoSQL HTTP | Native authenticated lifecycle routes and session-bound native CRUD using the same contract. NoSQL driver needs no SQL or GraphQL scaffolding. |

Conformance compares all six golden runs, then mixes two server transports to test outsider visibility and ownership; a second process supplies the commit-race oracle. Compatibility review covers new public results/errors/warnings and every binding; reuse the one error table, not transport switches. Keep existing routes and tokenless semantics; new setup is documented bounded session configuration using current server authentication, not a new user/account model. Resource/timeout diagnostics include redacted lifecycle reason and commit identity when available. Repository format does not change; ephemeral sessions disappear on restart, and clients must handle conflict/expiry/unknown outcomes. Ship the six bindings and migration guidance in one release after CI, using `dev/bin/release` when release is authorized.

## Unresolved Questions
None. The maintainer confirmed HEAD-change rejection and single-process CLI transaction ownership on 2026-09-13. Private type names, wire encodings, and measured bounded configuration values are autonomous Phase 0 design deliverables.
