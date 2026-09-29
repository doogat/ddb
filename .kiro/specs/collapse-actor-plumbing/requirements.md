# Requirements Document

## Introduction

This feature is a server-internal refactor of the `ddb-server` write actor (PRD 00172).
Today, adding one verb to the write actor requires editing four synchronized places:
an `ActorCommand` variant (36 struct-like variants), an `ActorReply` variant (29
variants), an `ActorHandle` verb method (each ending in a copy-pasted "unexpected
reply" fallback — 36 identical arms), and a dispatch arm in `handle_command` (a 36-arm
match). That is roughly 1000 lines of pure plumbing whose only job is relaying a call
to `DoogatService` on the actor's blocking OS thread. The loop additionally re-derives
each mutation's event intent by re-matching command variants before emitting events,
and schema-reload is hand-wired at five scattered trigger sites across adapters.

This refactor converges the write path on the closure idiom that the read pool already
proves. One generic `ActorHandle::call<R>` carrying a boxed
`FnOnce(&mut DoogatService) -> R` closure plus its own typed reply channel replaces
every per-verb method; `handle_command` disappears because the closure is the dispatch;
and the twin enums are deleted. Event emission moves from command re-matching to an
explicit `EventIntent` carried on the message. The five hand-wired schema-reload
triggers collapse into one derivation helper called from one place — the actor bridge.

The overriding correctness obligation is byte-identical behavior: every existing
server and e2e test must pass unchanged, and GraphQL/REST/NoSQL/PgWire responses must
be identical pre and post refactor. Verification follows the 00156 method — diff the
moved logic and rely on the unchanged suites as the oracle, not output re-derivation.
There is no user-facing surface change.

## Glossary

- **Actor_Handle**: The `ActorHandle` type in `ddb-server/src/actor/mod.rs` that adapters
  use to route work onto the write actor's blocking OS thread.
- **Call_Entry_Point**: The single generic method `ActorHandle::call<R, F>(event, f)`
  that routes one verb through the actor.
- **Actor_Loop**: The blocking OS thread function `actor_loop` that owns the single
  `DoogatService` instance and processes messages one at a time.
- **Actor_Msg**: The one message the actor understands, carrying a boxed closure
  (`run`) and an `EventIntent` (`event`).
- **Doogat_Service**: The `ddb_core::service::DoogatService` core type that every verb
  invokes to perform read or mutation work.
- **Event_Intent**: The enum carried on `Actor_Msg` that explicitly declares what kind
  of event a verb emits (`None`, `Created`, `Updated`, `Deleted`, `CreateMany`,
  `BatchUpdate`, `Upsert`).
- **Event_Bus**: The `EventBus` that receives `DoogatEvent`s emitted after a mutation.
- **Schema_Change**: The enum (`None`, `Changed`) that signals whether a mutation
  changed the schema.
- **Schema_Reloader**: The `SchemaReloader` type whose `apply` method fires at most one
  schema reload per actor-routed mutation.
- **Actor_Bridge**: The single call site inside the actor path that derives the
  schema-change signal and calls `Schema_Reloader.apply`.
- **Read_Pool**: The `ReadPool` type whose closure API (`with_service`/`with_service_mut`)
  is the pre-existing idiom this refactor converges the write path onto.
- **Transport**: A public application interface — CLI, GraphQL, REST, PgWire, FFI, or
  NoSQL HTTP — through which a verb is invoked.
- **Actor_Result**: The return type `ActorResult<T> = Result<T, DoogatError>` that every
  verb routed through `Call_Entry_Point` produces.
- **Structured_Error**: A typed `DoogatError` value returned through the normal adapter
  error mapping, as opposed to a panic or an "unexpected reply" string.

## Requirements

### Requirement 1: Single generic closure entry point

**User Story:** As a ddb-server maintainer, I want one generic entry point to route any
verb through the write actor, so that adding a verb no longer requires editing four
synchronized plumbing sites.

#### Acceptance Criteria

1. THE Actor_Handle SHALL expose a single generic method `call<R, F>` that accepts an Event_Intent and a closure of type `FnOnce(&mut DoogatService) -> R`.
2. WHEN a verb is routed through the Call_Entry_Point, THE Call_Entry_Point SHALL invoke the supplied closure with a mutable reference to the single Doogat_Service instance on the Actor_Loop thread.
3. WHEN the supplied closure returns a value of type `R` on the Actor_Loop thread, THE Call_Entry_Point SHALL return that same value of type `R` to the caller.
4. THE Actor_Handle SHALL constrain the closure return type `R` and the closure `F` to be `Send` and `'static`.
5. WHILE a verb is routed through the Call_Entry_Point, THE Actor_Handle SHALL NOT allocate an `ActorCommand` value or an `ActorReply` value.
6. IF the supplied closure panics while executing on the Actor_Loop thread, THEN THE Call_Entry_Point SHALL return a Structured_Error of the verb's Actor_Result type and SHALL NOT terminate the process.
7. WHEN a verb is routed through the Call_Entry_Point, THE Call_Entry_Point SHALL route exactly one Actor_Msg carrying the closure and its Event_Intent, and SHALL await exactly one reply value.

### Requirement 2: Removal of the twin-enum plumbing

**User Story:** As a ddb-server maintainer, I want the per-verb enums and dispatch
removed, so that the write path has one transport pattern and no duplicated fallbacks.

#### Acceptance Criteria

1. THE Actor_Handle SHALL route every verb through the Call_Entry_Point rather than through a per-verb method.
2. THE ddb-server actor module SHALL NOT define the `ActorCommand` enum, the `ActorReply` enum, or the `handle_command` dispatch function after the refactor completes.
3. THE ddb-server actor module SHALL NOT contain any reply-mismatch fallback arm (the arm previously handling a reply variant that did not match the requested verb) after the refactor completes.
4. WHEN a maintainer routes a new verb through the actor, THE Actor_Handle SHALL require exactly one added or modified touch point, located at the verb's call site.
5. IF a verb is routed through a per-verb method rather than through the Call_Entry_Point, THEN THE Actor_Handle SHALL fail to compile.
6. THE Actor_Handle SHALL preserve the existing observable result and error behavior of every verb routed through the Call_Entry_Point, such that each verb returns the same success result and the same error indication as before the refactor.

### Requirement 3: Explicit event intent on mutations

**User Story:** As a ddb-server maintainer, I want event emission derived from an
explicit intent carried on the message, so that emission no longer depends on
re-matching command variants in the loop.

#### Acceptance Criteria

1. WHEN a verb is routed through the Call_Entry_Point, THE caller SHALL supply exactly one Event_Intent value on the Actor_Msg that declares the kind of event the verb emits, selected from the set {None, Created, Updated, Deleted, CreateMany, BatchUpdate, Upsert}.
2. THE Actor_Loop SHALL derive emitted events solely from the Event_Intent carried on the Actor_Msg and SHALL NOT re-match any command variant to determine emitted events.
3. WHERE a verb is a read or a non-mutating statement, THE caller SHALL supply `EventIntent::None` and THE Event_Bus SHALL receive zero events for that verb.
4. WHEN a `create` mutation succeeds, THE Event_Bus SHALL receive exactly one `Created` event carrying the id and doogat type of the returned doogat.
5. WHEN an `update` mutation succeeds, THE Event_Bus SHALL receive exactly one `Updated` event carrying the id and doogat type of the returned doogat.
6. WHEN a `delete` mutation succeeds, THE Event_Bus SHALL receive exactly one `Deleted` event carrying the id and doogat type resolved before the delete closure ran.
7. WHEN a `create_many` mutation returning N doogats (N ≥ 0) succeeds, THE Event_Bus SHALL receive exactly N `Created` events, one per returned doogat.
8. WHEN a `batch_update` mutation returning N doogats (N ≥ 0) succeeds, THE Event_Bus SHALL receive exactly N `Updated` events, one per returned doogat.
9. WHEN an `upsert_singleton` mutation succeeds, THE Event_Bus SHALL receive exactly one event whose kind is `Created` if the upsert outcome created a new doogat and `Updated` if the upsert outcome modified an existing doogat.
10. IF a mutation returns an error, THEN THE Event_Bus SHALL receive zero events for that mutation and the pre-mutation persisted state SHALL remain unchanged.
11. FOR ALL mutation kinds in {create, update, delete, batch_update, create_many, upsert_singleton}, THE set of events emitted via Event_Intent SHALL equal the set of events emitted by the pre-refactor command-matching path, compared by event kind, id, and doogat type.
12. IF the Event_Bus rejects or fails to accept an event derived from a succeeded mutation's Event_Intent, THEN THE Actor_Loop SHALL surface an error indicating event emission failed while preserving the committed mutation result.

### Requirement 4: Centralized schema-reload derivation

**User Story:** As a ddb-server maintainer, I want schema-reload decided in one helper
called from one place, so that no adapter re-decides reload and no reload double-fires
or is missed.

#### Acceptance Criteria

1. THE ddb-server actor path SHALL derive the Schema_Change signal for an actor-routed mutation in exactly one derivation helper.
2. THE Actor_Bridge SHALL be the single call site that invokes `Schema_Reloader.apply` for an actor-routed mutation.
3. THE ddb-server adapters SHALL NOT retain a hand-wired schema-reload trigger in `schema/mutations/operations.rs`, `rest.rs`, or `pgwire.rs` after the refactor completes.
4. WHEN the derived Schema_Change signal is `Changed`, THE Schema_Reloader SHALL fire exactly one reload.
5. WHEN the derived Schema_Change signal is `None`, THE Schema_Reloader SHALL fire no reload.
6. WHEN a schema-changing statement is executed through PgWire DDL, THE Actor_Bridge SHALL derive the Schema_Change signal through the single derivation helper and fire exactly one reload for that statement.
7. WHEN a sequence of schema-changing and non-schema-changing statements is routed through the actor, THE Schema_Reloader version delta SHALL equal the count of schema-changing statements in the sequence.
8. IF a schema reload fails or times out, THEN THE ddb-server SHALL surface the failure and SHALL leave the already-applied schema state intact, and the triggering mutation's result SHALL remain unaffected.

### Requirement 5: Byte-identical behavior across transports

**User Story:** As a downstream consumer of any ddb transport, I want responses to be
identical before and after this refactor, so that no integration breaks.

#### Acceptance Criteria

1. WHEN a request is issued over any Transport in {GraphQL, REST, NoSQL HTTP, PgWire}, THE ddb-server SHALL return a response whose serialized bytes are equal to the pre-refactor serialized response for the same request under identical inputs and repository state.
2. WHEN a verb is invoked through the Call_Entry_Point, THE Call_Entry_Point SHALL return an Actor_Result value equal, field-for-field including ordering, to the Actor_Result that a direct Doogat_Service call for the same verb and inputs produces.
3. WHEN the server integration suite and the `tests/e2e` suite are executed against the refactored ddb-server, THE ddb-server SHALL cause every test in both suites to pass with the suite source unmodified.
4. IF any test in the server integration suite or the `tests/e2e` suite fails against the refactored ddb-server, THEN THE ddb-server SHALL be treated as non-conforming and the failing behavior SHALL block the refactor from being considered complete.
5. WHEN a response, error, or warning is emitted by the GraphQL, REST, NoSQL HTTP, or PgWire adapter, THE ddb-server SHALL emit the identical response shape, error shape, and warning shape (same fields, field names, and structure) that adapter produced before the refactor.

### Requirement 6: No-panic teardown

**User Story:** As a downstream consumer, I want the actor to fail with a structured
error when the actor thread is gone, so that a teardown never crashes the process or
masks a real error.

#### Acceptance Criteria

1. IF the Actor_Loop thread is gone when a verb is routed, THEN THE Call_Entry_Point SHALL return a Structured_Error of the verb's Actor_Result type that identifies the actor-unavailable condition and preserves any in-flight request state without partial mutation.
2. IF the reply channel for a routed verb is dropped before a reply is received, THEN THE Call_Entry_Point SHALL return a Structured_Error of the verb's Actor_Result type that identifies the dropped-reply condition and preserves any in-flight request state without partial mutation.
3. WHILE handling an actor-gone or dropped-reply condition, THE Call_Entry_Point SHALL return control to the caller within 100 milliseconds and SHALL NOT panic, abort, or terminate the process.
4. WHILE handling an actor-gone or dropped-reply condition, THE Call_Entry_Point SHALL NOT substitute an "unexpected reply" placeholder value for the Structured_Error described in criteria 1 and 2.
5. WHEN the Call_Entry_Point returns a Structured_Error for an actor-gone or dropped-reply condition, THE Call_Entry_Point SHALL return exactly one Structured_Error variant per routed verb such that two callers observing the same condition receive the same variant.

### Requirement 7: Closure boundary soundness

**User Story:** As a ddb-server maintainer, I want the compiler to enforce that closures
capture only owned data, so that no adapter-local borrow crosses the thread boundary.

#### Acceptance Criteria

1. WHEN a closure is submitted at the Call_Entry_Point, THE Actor_Handle SHALL accept it only if it satisfies both the `Send` and `'static` trait bounds, and SHALL reject any closure that fails either bound.
2. IF a closure submitted at the Call_Entry_Point captures adapter-local borrowed state, THEN THE ddb-server SHALL fail to compile with a compiler error indicating the offending borrow does not satisfy the `Send + 'static` bound.
3. THE Call_Entry_Point SHALL pass the Doogat_Service to the closure exclusively as a `&mut` parameter, such that no borrow of Doogat_Service is captured by the closure and no borrow of Doogat_Service crosses the thread boundary.
4. WHILE a closure is executing at the Call_Entry_Point, THE Call_Entry_Point SHALL grant the closure exactly one exclusive `&mut` borrow of the Doogat_Service for the duration of that single closure invocation, releasing the borrow before the next closure is invoked.

### Requirement 8: Maintenance verbs migrate to the closure entry point

**User Story:** As a ddb-server maintainer, I want maintenance verbs routed through the
same entry point, so that the write path has one transport pattern while worker
extraction stays deferred.

#### Acceptance Criteria

1. WHEN the ddb-server receives a `sync`, `compact`, or `run_maintenance` verb, THE ddb-server SHALL dispatch it through the Call_Entry_Point and SHALL NOT use any per-verb actor command or reply plumbing.
2. WHILE this refactor is in effect, THE ddb-server SHALL execute each `sync`, `compact`, and `run_maintenance` verb inline on the Actor_Loop write thread, serialized so that no two of these verbs execute concurrently and each completes before the next Actor_Loop message is processed.
3. WHILE executing a `sync`, `compact`, or `run_maintenance` verb, THE ddb-server SHALL hold the repository write lock for the full duration of the verb's git write operations.
4. IF a `sync`, `compact`, or `run_maintenance` verb dispatched through the Call_Entry_Point fails, THEN THE ddb-server SHALL return a result indicating the failure to the caller and SHALL leave the repository state unchanged from before the verb began.

### Requirement 9: Read pool API unchanged

**User Story:** As a ddb-server maintainer, I want the read pool untouched, so that the
refactor's scope stays on the write path.

#### Acceptance Criteria

1. WHEN the refactor completes, THE Read_Pool SHALL expose the same closure API function signatures, parameter types, and return types that existed before the refactor.
2. WHEN the refactor completes, THE Read_Pool SHALL produce identical observable behavior for identical closure inputs compared to before the refactor.
3. WHERE the Read_Pool references the Actor_Handle in a documentation comment, THE ddb-server SHALL update only the text of that documentation comment and SHALL NOT alter any Read_Pool code, signature, or behavior.
4. THE Read_Pool SHALL NOT invoke the Actor_Handle at runtime.

### Requirement 10: App-contract-first routing and single error policy

**User Story:** As a ddb architecture owner, I want every verb to flow through the app
contract and errors to derive from one policy table, so that transports adapt rather
than re-decide behavior.

#### Acceptance Criteria

1. WHEN any transport receives a verb request, THE ddb-server SHALL route it through the app contract path (`AppCommand` → Doogat_Service → `AppOutput`) such that zero verbs use per-verb command or reply enum plumbing that bypasses this path.
2. WHEN a Doogat_Service method returns an error inside a closure, THE Call_Entry_Point SHALL propagate that error over the reply channel with its error code, category, and message content unchanged.
3. THE ddb-server transports SHALL derive error category, transport status, redaction, and FFI variant from exactly one error-policy table.
4. IF a transport module contains logic that assigns error status or redaction independently of the single error-policy table, THEN THE ddb-server SHALL treat that as a contract violation and SHALL NOT expose a second mapping path.
5. IF the ddb-server library or FFI code encounters an unexpected actor, reply-channel, mutex, database, filesystem, repository, or user-input state, THEN THE ddb-server SHALL return an error derived from the single error-policy table and SHALL NOT panic, terminate the process, or leave the reply channel unanswered.

### Requirement 11: Performance neutrality

**User Story:** As a ddb operator, I want this refactor to be performance-neutral, so
that the closure transport introduces no regression.

#### Acceptance Criteria

1. THE ddb-server SHALL execute all write operations on exactly one blocking Actor_Loop thread after the refactor, preserving the single-writer concurrency model.
2. WHEN a verb is routed through the Call_Entry_Point, THE ddb-server SHALL allocate exactly one reply channel for that call.
3. WHILE running the nfr01, nfr02, and nfr03 performance-threshold tests at the 5,000-doogat scale on the same platform as the pre-refactor baseline, THE ddb-server SHALL keep query latency at or below 10 milliseconds, repo growth at or below 50 megabytes per year, and sync latency within the nfr03 target of 2 seconds, without exceeding the corresponding pre-refactor measured value.
4. IF any of the nfr01, nfr02, or nfr03 threshold results exceeds its pre-refactor measured baseline at the 5,000-doogat scale, THEN THE ddb-server SHALL be treated as failing performance neutrality, and the regressing measurement SHALL be reported.

### Requirement 12: Validation and per-PRD deliverables

**User Story:** As a ddb maintainer, I want the change to pass the local gate and ship
its authored deliverables, so that the refactor meets the Definition of Done.

#### Acceptance Criteria

1. WHEN the Tier 1 local gate runs, THE ddb-server change SHALL cause `cargo build` to complete with exit code 0.
2. WHEN the Tier 1 local gate runs, THE ddb-server change SHALL cause `cargo clippy --workspace --all-targets` to complete with exit code 0 and report zero warnings and zero errors.
3. WHEN the Tier 1 local gate runs, THE ddb-server change SHALL cause `cargo test-ci` to complete with zero failed tests.
4. IF any of `cargo build`, `cargo clippy --workspace --all-targets`, or `cargo test-ci` returns a nonzero exit code or reports at least one warning or failure, THEN THE ddb-server change SHALL be treated as not meeting the Tier 1 local gate and SHALL NOT be considered complete.
5. THE ddb-server change SHALL include at least one colocated unit test in the same module file (or its adjacent `tests` submodule) for every module file it modifies, and each such test SHALL pass under `cargo test-ci`.
6. THE ddb-server change SHALL author at least one `integration_` or `smoke_` scenario file under `tests/e2e/` and SHALL register every newly added `tests/e2e/` module in `tests/e2e/main.rs`, with execution of these scenarios delegated to CI (Tier 2) rather than run in the Tier 1 local gate.
7. THE ddb-server change SHALL update `docs/src/technical/server.md` and `docs/src/technical/walkthrough.md` so that both files describe the closure actor and contain no remaining description of the twin-enum dispatch.
8. WHERE this refactor introduces no change observable at any public application interface (CLI, GraphQL, REST, PgWire, FFI, NoSQL HTTP), THE ddb-server change SHALL NOT add a CHANGELOG entry and SHALL NOT add a showboat walkthrough.
