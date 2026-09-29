# Implementation Plan: collapse-actor-plumbing

## Overview

Server-internal refactor of the `ddb-server` write actor (PRD 00172). Converge the
write path onto the closure idiom the read pool already proves: one generic
`ActorHandle::call<R, F>` carrying a boxed `FnOnce(&mut DoogatService) -> R` closure
plus its own typed reply channel replaces every per-verb method and enum variant;
`handle_command` disappears (the closure IS the dispatch); event emission moves from
`match &msg.cmd` re-introspection to an explicit `EventIntent` carried on the message;
and the five hand-wired schema-reload triggers collapse into one derivation helper
called from one place — the actor bridge.

**Overriding obligation: byte-identical behavior.** Every existing server/e2e test must
pass unmodified, and GraphQL/REST/NoSQL/PgWire responses must be identical pre/post.
Verification follows the 00156 method — diff the moved logic, rely on the unchanged
suites as the oracle — not output re-derivation.

**Reversibility strategy (explicit in task ordering): the twin enums
(`ActorCommand`/`ActorReply`) and `handle_command` stay compiling through the end of
Phase 1. They are deleted only in Phase 2, after every verb, every event, and every
reload trigger has been migrated onto the closure path and the suites are green. This
keeps each Phase 0/1 task independently revertible.**

Language: Rust (grounded in the existing `ddb-server` codebase — no pseudocode in the
design; the design's algorithms map directly to Rust).

**Tier 1 per-task gate (AGENTS.md).** Every non-optional task below is NOT complete
until `cargo build`, `cargo clippy --workspace --all-targets` (zero warnings), and
`cargo test-ci` all pass locally, and its colocated TDD unit test passes under
`cargo test-ci`. Do NOT run Tier 2 per task (`cargo test --workspace`, the e2e suite
except the CLAUDE.md conditional in Phase 2, property tests, coverage, showboat verify)
— those are delegated to CI. _Requirements: 12.1, 12.2, 12.3, 12.4, 12.5_

Grounding references (verified in-tree):
- `ddb-server/src/actor/mod.rs`: `ActorCommand` (36 variants, :44-173), `ActorReply`
  (29 variants, :176-207), `ActorMsg { cmd, reply }` (:210), 36 `ActorHandle` verb
  methods each with an `_ => Err(... "unexpected reply")` arm, `send` (:actor-gone /
  dropped-reply fallbacks), `actor_loop` event introspection (`match &msg.cmd`
  :512-537), `emit_mutation_events`, upsert emit block.
- `ddb-server/src/actor/handlers.rs`: `handle_command` 36-arm dispatch.
- `ddb-server/src/actor/tests.rs`: existing shared-transaction-scope tests.
- `ddb-server/src/reload.rs`: `SchemaReloader::{trigger_reload_and_wait, version}`.
- Reload trigger sites to retire: `schema/mutations/operations.rs:266-269, :307-311,
  :578-582`, `rest.rs:464-468`, `pgwire.rs:117-119`.
- Verb call sites: `schema/mutations/**`, `rest.rs` (`delete_doogat` :445, apply-schema
  :464), `nosql_api.rs` (`nosql_get` :38, `nosql_scan_type`/`nosql_scan_tag` :70-71,
  `nosql_backlinks` :94), `pgwire.rs` (`execute_sql` :110-114), `schema/subscriptions.rs`
  (`get_doogat` :102, :129), `maintenance.rs` (`compact` :14), `reload.rs`
  (`get_type_schemas` :90 — internal, still routed through `call`), `lib.rs`
  (`get_type_schemas` :59, `health_check` :231).
- `read_pool.rs`: doc comments at :23-25 (only text changes here — NOT a caller).

---

## Tasks

### Phase 0 — Foundation (add the closure transport BESIDE the enums; remove nothing)

- [ ] 1. Add the closure transport (EventIntent, ActorMsg, generic `call`) alongside the existing twin enums
  - [x] 1.1 Write the Phase 0 acceptance unit test first (TDD) in `ddb-server/src/actor/tests.rs`
    - Add a test that spawns an `ActorHandle` and drives ONE read through `call`
      (`EventIntent::None`, e.g. `|svc| svc.get_type_schemas()` or `svc.nosql_get(&id)`)
      asserting it returns the value and emits NO event.
    - Add a test that drives ONE mutation through `call` (e.g. `EventIntent::Created`
      with `|svc| svc.create(cmd)`) asserting it returns the value AND emits the
      expected `DoogatEvent` (subscribe to the `EventBus` before the call).
    - These tests must compile and pass WHILE `ActorCommand`/`ActorReply`/`handle_command`
      still exist (proves additive, reversible).
    - _Requirements: 1.1, 1.2, 1.3, 3.4, 3.3_
  - [x] 1.2 Add `EventIntent` enum, closure-carrying `ActorMsg`, and generic `ActorHandle::call<R, F>`
    - In `ddb-server/src/actor/mod.rs`, add `pub enum EventIntent { None, Created,
      Updated, Deleted { id: String, doogat_type: Option<String> }, CreateMany,
      BatchUpdate, Upsert { type_name: String } }`.
    - Add a NEW closure message type (do not touch the existing `ActorMsg { cmd, reply }`
      yet — introduce a distinct message channel or an internal variant so both compile;
      the design's target `ActorMsg { run: Box<dyn FnOnce(&mut DoogatService, &EventBus)
      + Send>, event: EventIntent }` is the end-state, reached without deleting the old
      one in Phase 0).
    - Implement `pub async fn call<R, F>(&self, event: EventIntent, f: F) -> R where
      F: FnOnce(&mut DoogatService) -> R + Send + 'static, R: Send + 'static`. It
      allocates a `oneshot::channel::<R>()`, boxes a closure that runs `f`, performs
      Ok-gated emission from `(event, &result)` at the closure boundary (Option A from
      the design), then sends `R` (ignoring a dropped receiver).
    - Add the internal `FromActorError` trait (one impl over `Result<T, DoogatError>`)
      so the actor-gone and dropped-reply fallbacks stay total and construct a
      structured `Err` of `R` — never a panic, never an "unexpected reply" string.
      Preserve the exact structured-error semantics of the current `send`
      (actor-stopped / actor-dropped-reply).
    - Extend `actor_loop` to run the new closure message (`(msg.run)(&mut svc,
      &event_bus)`) alongside the still-present `match &msg.cmd` path.
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 1.7, 6.1, 6.2, 6.3, 6.4, 6.5, 7.1, 7.3, 7.4, 10.2, 10.5_
  - [~] 1.3 Write the actor-gone / dropped-reply structured-error unit test
    - In `ddb-server/src/actor/tests.rs`: drop the actor thread (or its receiver), call
      a verb through `call`, assert a structured `DoogatError` returns over the oneshot
      within budget — never a panic, never "unexpected reply".
    - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5, 10.5_ / **Property 4**

- [~] 2. Checkpoint — closure path works end-to-end for one read + one mutation with enums still present
  - Ensure `cargo build`, `cargo clippy --workspace --all-targets` (clean), and
    `cargo test-ci` all pass; the twin enums still compile. Ask the user if questions arise.

### Phase 1 — Core (move EVERY verb, event, and reload onto the closure path; enums still present)

- [ ] 3. Centralize schema-reload derivation (helper + single bridge call)
  - [~] 3.1 Write reload derivation unit tests first (TDD) in `ddb-server/src/reload.rs`
    - Test `derive_schema_change` returns `Changed` for `applied && !dry_run`
      (applySchema), for `requires_schema_reload(sql) && Ok` (executeSql), for any
      `requires_schema_reload(stmt) && Ok` in a batch, and `None` otherwise.
    - Test the bridge `apply(Changed)` bumps `version()` by exactly one and
      `apply(None)` bumps it by zero.
    - _Requirements: 4.1, 4.4, 4.5, 4.7_ / **Property 3**
  - [~] 3.2 Add `SchemaChange` enum, `derive_schema_change`, and `SchemaReloader::apply`
    - In `ddb-server/src/reload.rs`, add `pub enum SchemaChange { None, Changed }`,
      the `derive_schema_change(verb_kind, result, sql_or_none) -> SchemaChange` helper
      (deriving from `SchemaApplyReport.applied` gated on `!dry_run`, and
      `requires_schema_reload` for SQL/batch), and `pub async fn apply(&self, change:
      SchemaChange)` firing exactly one `trigger_reload_and_wait` iff `Changed`.
    - Do NOT yet remove the five hand-wired trigger sites (done in 3.3 as call sites
      migrate); keep behavior additive until each site is retired.
    - _Requirements: 4.1, 4.2, 4.4, 4.5, 4.8_
  - [~] 3.3 Wire the actor bridge to call `apply` once per actor-routed mutation and retire the 5 hand-wired triggers
    - Have the actor bridge (the `call` path / actor_loop) invoke `SchemaReloader::apply`
      exactly once per actor-routed mutation, deriving `SchemaChange` via the helper.
    - Retire all five hand-wired triggers as their owning call sites migrate to `call`:
      `schema/mutations/operations.rs:266-269, :307-311, :578-582`, `rest.rs:464-468`,
      `pgwire.rs:117-119` (PgWire DDL already reaches the actor via `execute_sql`, so no
      PgWire-side trigger remains — retiring it prevents a double-fire).
    - _Requirements: 4.1, 4.2, 4.3, 4.6, 4.7, 4.8_

- [ ] 4. Migrate read + non-mutating verb call sites to `call` (EventIntent::None)
  - [~] 4.1 Migrate NoSQL HTTP routes in `nosql_api.rs` to `call`
    - `nosql_get` (:38), `nosql_scan_type`/`nosql_scan_tag` (:70-71), `nosql_backlinks`
      (:94) call `actor.call(EventIntent::None, |svc| svc.nosql_*(...))`. Response
      shapes byte-identical.
    - _Requirements: 2.1, 2.6, 3.3, 5.1, 5.5, 10.1_
  - [~] 4.2 Migrate GraphQL read resolvers and subscription reads to `call`
    - Query resolvers under `schema/` and `schema/subscriptions.rs` (`get_doogat` at
      :102, :129) route through `call` with `EventIntent::None`. Return types unchanged.
    - _Requirements: 2.1, 2.6, 3.3, 5.1, 5.5, 10.1_
  - [~] 4.3 Migrate internal/read callers in `reload.rs` and `lib.rs` to `call`
    - `reload.rs` `get_type_schemas` (:90), `lib.rs` `get_type_schemas` (:59) and
      `health_check` (:231) route through `call` with `EventIntent::None`. Leave
      `read_pool.rs` untouched — only update its doc comment at :23-25 to describe the
      closure transport (it is NOT a caller and must not invoke the actor at runtime).
    - _Requirements: 2.1, 5.1, 9.1, 9.2, 9.3, 9.4_
  - [~] 4.4 Write colocated unit test asserting reads emit no events through `call`
    - Assert a read routed via `call(EventIntent::None, ...)` produces zero `EventBus`
      events and returns the identical value a direct `DoogatService` call produces.
    - _Requirements: 3.3, 5.2_ / **Property 1**

- [ ] 5. Migrate mutating verbs to `call` with explicit EventIntent + emission parity
  - [~] 5.1 Write per-kind event-parity unit tests first (TDD) in `ddb-server/src/actor/tests.rs`
    - One test per mutation kind — create (`Created`), update (`Updated`), delete
      (`Deleted{id,type}` captured pre-delete), batch_update (`BatchUpdate` → N
      `Updated`), create_many (`CreateMany` → N `Created`), upsert_singleton
      (`Upsert{type}` → `Created` iff `outcome.created` else `Updated`).
    - Assert the emitted `DoogatEvent` multiset (kind, id, doogat_type) equals what the
      pre-refactor `match &msg.cmd` block emits, and that an `Err` result emits nothing.
    - Write these BEFORE removing the old introspection block so both paths are
      comparable.
    - _Requirements: 3.1, 3.4, 3.5, 3.6, 3.7, 3.8, 3.9, 3.10, 3.11_ / **Property 2**
  - [~] 5.2 Migrate GraphQL mutation resolvers (`schema/mutations/**`) to `call` with correct EventIntent
    - `operations.rs`, `singleton.rs`, `types.rs`: `create` → `EventIntent::Created`,
      `update` → `Updated`, `delete` → `Deleted{id,type}` (resolve id+type BEFORE the
      delete closure runs), `batch_create` → `CreateMany`, `batch_update` →
      `BatchUpdate`, `upsert_singleton` → `Upsert{type_name}`, `execute_sql`/
      `execute_batch`/`apply_schema` → `EventIntent::None` (reload derived by the
      bridge). Return types unchanged (`apply_schema` still yields
      `AppOutput<SchemaApplyReport>`).
    - _Requirements: 2.1, 2.6, 3.1, 3.2, 3.4, 3.5, 3.7, 3.8, 3.9, 5.1, 5.2, 5.5, 10.1, 10.2_
  - [~] 5.3 Migrate REST mutating handlers (`rest.rs`) to `call`
    - `delete_doogat` (:445) → `EventIntent::Deleted{id,type}`; REST schema-apply
      (:464) → `call` with reload derived by the bridge (retire the local trigger,
      coordinated with 3.3). Other REST mutations thread the matching `EventIntent`.
    - _Requirements: 2.1, 2.6, 3.6, 4.3, 5.1, 5.5, 10.1_
  - [~] 5.4 Migrate PgWire `execute_sql` (`pgwire.rs:110-114`) to `call`
    - Route through `call(EventIntent::None, |svc| svc.execute_sql(query))`; the reload
      trigger (:117-119) is covered by the bridge and retired (coordinated with 3.3).
    - _Requirements: 2.1, 2.6, 4.6, 5.1, 5.5, 10.1_
  - [~] 5.5 Replace the `match &msg.cmd` event introspection with EventIntent-driven emission
    - Remove the `actor_loop` introspection (mod.rs:512-537) and the upsert emit block;
      `emit_mutation_events` becomes the closure-boundary helper driven by
      `EventIntent` + the Ok/Err result (Option A). No command re-match remains.
    - Surface an error if the bus rejects an event derived from a succeeded mutation,
      preserving the committed result.
    - _Requirements: 3.2, 3.10, 3.11, 3.12_ / **Property 2**

- [ ] 6. Migrate maintenance verbs (sync, compact, run_maintenance) to `call`, kept inline on the write loop
  - [~] 6.1 Write colocated unit test first (TDD) for a maintenance verb through `call`
    - Assert `compact`/`run_maintenance`/`sync` routed via `call` returns the same
      report type and, on failure, leaves repo state unchanged and reports the failure.
    - _Requirements: 8.1, 8.4_
  - [~] 6.2 Route `sync`, `compact`, `run_maintenance` through `call` (inline, no worker extraction)
    - `maintenance.rs` (`compact` :14) and any sync/maintenance call sites use `call`
      with `EventIntent::None`; keep them executing inline on the actor write loop,
      serialized, holding the repo write lock for the full git-write duration (00174
      defers worker extraction). Preserve the post-compact / post-sync
      `rebuild_if_stale` behavior currently in `handle_command`.
    - _Requirements: 8.1, 8.2, 8.3, 8.4_

- [~] 7. Checkpoint — every verb, event, and reload routes through `call`; enums still present
  - Ensure `cargo build`, `cargo clippy --workspace --all-targets` (clean), and
    `cargo test-ci` pass; confirm no remaining runtime user of `ActorCommand`/
    `ActorReply` other than the now-dead definitions. Ask the user if questions arise.

### Phase 2 — Integration (delete dead plumbing; prove no behavior change)

- [ ] 8. Delete the twin enums and dispatch; the closure IS the dispatch now
  - [~] 8.1 Delete `ActorCommand`, `ActorReply`, `handle_command`, the 36 verb methods, and the 36 "unexpected reply" fallbacks
    - Remove `ActorCommand` (mod.rs:44-173), `ActorReply` (:176-207), the old
      `ActorMsg { cmd, reply }`, the old `send`, all 36 per-verb `ActorHandle` methods,
      and delete `ddb-server/src/actor/handlers.rs` (`handle_command`). Collapse
      `ActorMsg` to the single `{ run, event }` shape. No reply-mismatch arm remains.
    - Confirm `cargo build`, `cargo clippy --workspace --all-targets` (clean),
      `cargo test-ci` green; the existing shared-transaction-scope tests in
      `actor/tests.rs` still pass through the migrated verbs.
    - _Requirements: 2.2, 2.3, 2.4, 2.5, 10.1_
  - [~] 8.2 Verify the "add a verb = one touch point" invariant with a compile-fenced note or doc test
    - Confirm (by a doc comment / minimal compile check) that a new verb requires only
      one call-site touch point and that a per-verb-method route would not compile.
    - _Requirements: 2.4, 2.5_

- [ ] 9. Author the property-based test harness (Tier 2 / CI-executed) for Properties 1–3
  - [~] 9.1 Add `ddb-server/tests/property_tests.rs` with proptest generators for P1, P2, P3
    - **Property 1 (byte-identical / `call` == direct DoogatService):** generate random
      valid create/update/delete/search inputs; assert the `call`-routed result equals a
      direct `DoogatService` call's `AppOutput`/`SqlResult`/error, field-for-field.
      **Validates: Requirements 5.1, 5.2, 5.3, 5.5, 2.6, 10.2**
    - **Property 2 (event multiset parity):** generate random batch sizes (0..N) for
      `batch_create`/`batch_update` and random upsert `created` outcomes; assert one
      event per returned doogat with correct kind, `Err` emits nothing, upsert emits
      `Created` iff `outcome.created`. **Validates: Requirements 3.2, 3.4–3.11**
    - **Property 3 (reload version delta == schema-statement count):** generate a random
      interleaving of schema-changing and non-schema statements routed through the
      actor; assert `SchemaReloader::version()` delta equals the count of
      schema-changing statements (no double-fire, no miss).
      **Validates: Requirements 4.1, 4.2, 4.4, 4.5, 4.6, 4.7**
    - Note the harness lives under `ddb-server/tests/` (not `ddb-core/tests/`) because
      P1–P3 exercise `ActorHandle`, which is a `ddb-server` type; the proptest idiom and
      `PROPTEST_CASES` conventions mirror `ddb-core/tests/property_tests.rs`. Execution
      is Tier 2 / CI (`cargo test -p ddb-server --test property_tests`), not per-task.
    - _Requirements: 5.1, 5.2, 5.3, 3.11, 4.7, 12.6_ / **Properties 1, 2, 3**

- [ ] 10. Author e2e/integration scenarios (Tier 2 / CI-executed) and register them
  - [~] 10.1 Add `tests/e2e/integration_closure_actor.rs` and register it in `tests/e2e/main.rs`
    - **Happy path:** a GraphQL create + a REST search return responses identical to
      pre-refactor; the create emits exactly one `Created` event.
    - **Edge:** a batch update and an upsert emit the same batch/upsert events from
      `EventIntent` that the old introspection produced.
    - **Error:** a `DoogatService` method returns `Err` inside the closure with the
      actor thread gone → the error propagates over the oneshot unchanged; a
      dropped-reply yields a structured error (never a panic, never "unexpected reply").
    - Register the new module in `tests/e2e/main.rs`. Execution is delegated to CI
      (Tier 2).
    - _Requirements: 5.1, 5.3, 5.4, 5.5, 6.1, 6.2, 6.4, 10.2, 12.6_ / **Properties 1, 2, 4**
  - [~] 10.2 Run the CLAUDE.md conditional e2e deletion safety net
    - Because Phase 2 deletes/replaces code paths, run `cargo build -p ddb-cli` then
      `cargo test -p ddb-e2e` once (the scope-limited deletion safety net from
      CLAUDE.md, not a general Tier 1 relaxation). Fix any regression before proceeding.
    - _Requirements: 5.3, 5.4_

- [ ] 11. Update documentation to describe the closure actor
  - [~] 11.1 Update `docs/src/technical/server.md` and `docs/src/technical/walkthrough.md`
    - Describe the closure actor (`call`, `EventIntent`, single `ActorMsg`, centralized
      reload derivation); remove every remaining description of the twin-enum dispatch
      (`ActorCommand`/`ActorReply`/`handle_command`). No CHANGELOG entry and no showboat
      walkthrough (no user-facing surface change).
    - _Requirements: 12.7, 12.8_

- [~] 12. Final checkpoint — Tier 1 gate green, then push master
  - Ensure `cargo build`, `cargo clippy --workspace --all-targets` (clean), and
    `cargo test-ci` all pass. Confirm no `ActorCommand`/`ActorReply`/`handle_command`
    references remain and responses are byte-identical (suites as oracle). After the
    PRD's commits land, push `master` per the AGENTS.md convention so nightly Tier 2 CI
    (`full-validation.yml`) exercises the new commits on `origin/master`. Ask the user
    if questions arise.
  - _Requirements: 12.1, 12.2, 12.3, 12.4, 11.1, 11.2, 11.3, 11.4_

## Notes

- Tasks marked with `*` are optional (test authoring / verification aids) and may be
  skipped for a faster MVP; core implementation and migration tasks are never optional.
  Note that most `*` tasks here encode the correctness properties (P1–P4) and the
  per-PRD e2e/property deliverables — skipping them weakens the byte-identical
  guarantee, so they are strongly recommended.
- **Required vs optional at a glance:** Required — 1.1, 1.2, 3.1, 3.2, 3.3, 4.1, 4.2,
  4.3, 5.1, 5.2, 5.3, 5.4, 5.5, 6.1, 6.2, 8.1, 10.2, 11.1 (plus checkpoints 2, 7, 12).
  Optional (`*`) — 1.3, 4.4, 8.2, 9.1, 10.1.
- **Reversibility:** the twin enums, `handle_command`, and the old `send`/`ActorMsg`
  stay compiling through Phase 1; only Task 8.1 deletes them. Any Phase 0/1 task can be
  reverted without breaking the build.
- **Byte-identical obligation** is pinned by Property 1 (9.1), the happy/edge/error e2e
  (10.1), and the unmodified server + e2e suites acting as the oracle (10.2), per the
  00156 method.
- Tier 1 gate (`cargo build`, `cargo clippy --workspace --all-targets`, `cargo test-ci`)
  runs after every non-optional task. Tier 2 (workspace tests, e2e except the 10.2
  conditional, property tests, coverage, showboat verify) is delegated to CI.
- `read_pool.rs` is doc-comment-only (Task 4.3); it is never a runtime caller of the
  actor (Requirement 9.4).
- Maintenance verbs stay inline on the write loop (Task 6.2); worker extraction is
  deferred to 00174.

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1"] },
    { "id": 1, "tasks": ["1.2"] },
    { "id": 2, "tasks": ["1.3", "3.1", "5.1", "6.1"] },
    { "id": 3, "tasks": ["3.2"] },
    { "id": 4, "tasks": ["3.3"] },
    { "id": 5, "tasks": ["4.1", "4.2", "4.3", "5.3", "5.4", "6.2"] },
    { "id": 6, "tasks": ["5.2"] },
    { "id": 7, "tasks": ["5.5"] },
    { "id": 8, "tasks": ["4.4"] },
    { "id": 9, "tasks": ["8.1"] },
    { "id": 10, "tasks": ["8.2", "9.1", "10.1", "11.1"] },
    { "id": 11, "tasks": ["10.2"] }
  ]
}
```
