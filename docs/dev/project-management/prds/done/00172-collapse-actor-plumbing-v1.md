---
default_model: opus
model_tier_rationale: concurrency - closure actor thread + oneshot reply; byte-identical behavior obligation across 36 verbs
---

# Collapse actor command/reply plumbing into a closure message

**Priority:** P2 · **Roadmap:** Phase 2 (app contract) · **Depends on:** none — the enabling refactor that UNBLOCKS 00175 / 00176 / 00177 / 00178 (routing a verb through the server drops from four touch points to one). Sequence before 00175-00178 (the routing PRDs it unblocks).

> **Provenance:** the schema-reload centralization was added by the 2026-07-16 assessment (F6; backlog-review-2026-07-16.md Blocking-3) and folded into the body's Features/tasks at the 2026-08-14 backlog review. Sites re-verified at `d77ad58`: `operations.rs:265-267,:307-308,:577-579`, `rest.rs:463-465` ("Mirror the GraphQL applySchema guard"), `pgwire.rs:116-117`. The missed-reload bug shipped twice during 00161 (GraphQL caught in blind review `b34dd30`, REST in doubt review `5a4f9e1`).

## Overview

### Problem Statement
The server actor pays a per-verb boilerplate tax that dwarfs every other server cost. Adding one verb today means editing four synchronized places before any transport can call it:

- **A command variant** — `ActorCommand` (`ddb-server/src/actor/mod.rs:38-173`) carries 36 struct-like variants, one per verb.
- **A reply variant** — `ActorReply` (`:176-207`) carries 29 variants, one per distinct return shape.
- **An `ActorHandle` method** — the verb methods span `:239-643` (the last, `apply_schema`, at `:626-643`), each building its command, `await`ing `send`, matching the one expected reply, and ending in a copy-pasted `_ => Err(DoogatError::Validation("unexpected reply".into()))` fallback — 36 identical fallback arms.
- **A dispatch arm** — `handlers::handle_command` (`ddb-server/src/actor/handlers.rs:9`) matches all 36 `ActorCommand::` variants (`:11-188`) to concrete `DoogatService` calls.

That is ~1000 LOC of pure plumbing growing ~30 lines per verb that buys nothing — it only relays a call to `DoogatService` on the actor's blocking OS thread (`actor_loop`, `:664`). Worse, the pre-dispatch event introspection (`:677-710`) reconstructs each mutation's intent by re-matching `&msg.cmd` variants (`mutation_kind` at `:693-698`, `is_batch_update`/`is_create_many` at `:687-688`, `upsert_type` at `:689-692`) before calling `emit_mutation_events` (`:701-709`), so every event-bearing verb also touches the emission switch.

The target idiom already ships in-repo: `ReadPool::with_service`/`with_service_mut` take `move |svc| svc.method(...)` closures at 22 call sites in `ddb-server/src/read_pool.rs` (and `ffi/driver.rs` uses the same shape), so a pool-served read adds no enum boilerplate — although ~19 read verbs still ALSO exist as `ActorCommand` variants (the pool duplicates them rather than replacing them; grounded 2026-09-05). The write actor (`ActorHandle` + the twin enums) is the last holdout of the old dispatch. This PRD converges the write path on the pattern the read pool already proves and deletes the duplicated read variants with it.

Until this collapses, PRDs 00175-00178 each pay the four-touch-point tax to route a verb through the app contract. Removing it first makes those PRDs one-touch.

### Target Users
ddb maintainers, and every downstream (jink) indirectly: each future capability the contract exposes through the server ships faster and with less drift once the actor is a thin closure relay.

### Success Metrics
- Adding or routing a verb touches ONE call site (a single generic `call`), not four synchronized enums/methods.
- `ActorCommand` and `ActorReply` are deleted; `handle_command`'s 36-arm match and the 36 `"unexpected reply"` fallbacks are gone.
- Event emission derives from an explicit event intent carried on the message, never from re-matching a command variant.
- The write actor uses the same closure-over-`&mut DoogatService` idiom as `ReadPool::with_service`; read and write share one transport pattern.
- The schema-reload decision lives in ONE derivation helper called from ONE place — the actor bridge, for every actor-routed mutation, PgWire DDL included (non-SELECT SQL already reaches the actor via `actor.execute_sql`, `pgwire.rs:110-114`). Zero hand-wired per-adapter reload triggers remain, `pgwire.rs:116-117` included.
- Behavior is byte-identical: every existing server/e2e test passes unchanged; GraphQL/REST/NoSQL responses are identical pre/post (verified the 00156 way — diff the moved logic, not the outputs).

## Functional Decomposition

### Capability: Closure-message actor transport
#### Feature: Generic closure `call` replacing the twin enums
- **Description**: the actor message carries a boxed closure over `&mut DoogatService` plus its own typed reply channel; one generic `ActorHandle::call<R>` replaces every per-verb method.
- **Inputs**: `impl FnOnce(&mut DoogatService) -> R + Send + 'static` from the caller.
- **Outputs**: `R` — the verb's own return type (e.g. `AppOutput<SchemaApplyReport>`), awaited over a `oneshot`.
- **Behavior**: `call` allocates a `oneshot`, boxes a closure that runs the caller's closure and sends `R` into the sender, ships it as the message, and awaits. No `ActorCommand`/`ActorReply` variant, no `"unexpected reply"` arm. `handle_command` disappears — the closure IS the dispatch.

#### Feature: Explicit event intent on mutations
- **Description**: mutating call sites declare their event intent explicitly instead of the loop re-deriving it from a command variant.
- **Inputs**: an `EventIntent` (mutation kind, deleted id/type, batch/create-many flags, upsert type) passed alongside the closure for event-bearing verbs.
- **Outputs**: the same `DoogatEvent`s on the `EventBus` as today.
- **Behavior**: replace the `match &msg.cmd` introspection (`mod.rs:678-710`) with a field read on the message's `EventIntent`; `actor_loop` calls `emit_mutation_events` from that field. Non-mutating verbs carry `EventIntent::None`.

#### Feature: Centralized schema-reload derivation
- **Description**: ONE shared helper decides "this mutation changed the schema" from the core result; the hand-wired per-adapter reload triggers are retired.
- **Inputs**: the mutation's core result (`SchemaApplyReport.applied` / `requires_schema_reload`-style signals).
- **Outputs**: a `SchemaReloader` trigger fired exactly when the helper says the schema changed — for every actor-routed mutation (GraphQL/REST/NoSQL) via one call in the actor bridge.
- **Behavior**: retire all 5 hand-wired sites (`operations.rs:265-267,:307-308,:577-579`, `rest.rs:463-465`, `pgwire.rs:116-117`). PgWire DOES route its non-SELECT SQL (all DDL) through the actor (`self.actor.execute_sql` at `pgwire.rs:110-114`, grounded 2026-09-05), so the actor-bridge call covers it; keeping a PgWire-side trigger would double-fire. Net: 5 hand-wired decisions → 1 policy helper with 1 call site.

## Structural Decomposition

### Repository Structure

```
ddb-server/src/actor/
├── mod.rs        # Maps to: Generic closure call (ActorMsg closure + ActorHandle::call, delete twin enums); Explicit event intent (EventIntent on the message, actor_loop emits from it)
└── handlers.rs   # Maps to: Generic closure call (handle_command removed — dispatch is the closure)
ddb-server/src/
├── read_pool.rs  # Maps to: Generic closure call (existing pool closure API unchanged; update actor-pattern documentation only)
├── reload.rs     # Maps to: Centralized schema-reload derivation (the one helper; actor bridge is its sole caller, including PgWire DDL)
├── schema/       # Maps to: Generic closure call (resolvers call via closures); reload trigger sites retired
├── pgwire.rs     # Maps to: Centralized schema-reload derivation (hand-wired trigger at :116-117 retired; DDL already actor-routed via execute_sql at :110-114, which moves onto `call`)
└── rest.rs       # Maps to: Generic closure call (REST handlers call via closures); reload trigger site retired
```

### Module: actor (ddb-server/src/actor/mod.rs, handlers.rs)
- **Maps to capability**: Closure-message actor transport
- **Responsibility**: own the closure message type, the generic `call`, the blocking loop, and event emission from explicit intent.
- **Exports**:
  - `ActorHandle::call<R>(...)` - the single generic entrypoint replacing 36 per-verb methods
  - `EventIntent` + `ActorMsg { run: Box<dyn FnOnce(&mut DoogatService) + Send>, event: EventIntent }`
  - (removed) `ActorCommand`, `ActorReply`, `handle_command`

### Module: reload and server callers (ddb-server/src/reload.rs, read_pool.rs, schema/, pgwire.rs, rest.rs, nosql_api.rs)
- **Maps to capability**: Closure-message actor transport
- **Responsibility**: migrate actor callers, preserve existing read-pool closures, and derive schema reload once in the actor bridge; retire adapter triggers.
- **Exports**: the shared reload derivation and existing transport handlers; no new public API.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **closure message + generic `call`**: the new transport primitive, added alongside the enums so migration is incremental.

### Core Layer (Phase 1)
- **verb migration + event intent**: Depends on [closure message] - move every `ActorHandle` verb call site onto `call`; thread `EventIntent` through mutations.

### Integration Layer (Phase 2)
- **enum removal + verification + docs**: Depends on [verb migration] - delete the dead enums/dispatch, confirm suites green, update the architecture doc.

## Implementation Phases

### Phase 0: Foundation
**Goal**: introduce the closure transport without removing anything.

**Tasks**:
- [ ] Add `EventIntent`, the closure-carrying `ActorMsg`, and `ActorHandle::call<R>` (allocates the oneshot, boxes the closure, awaits) beside the existing enums (no deps) - Acceptance: a unit test drives one read and one mutation through `call`, asserting the returned value and (for the mutation) the emitted `DoogatEvent`; the enums still compile.

**Exit Criteria**: the closure path works end-to-end for one read + one mutation; nothing removed yet.

### Phase 1: Core
**Goal**: move every verb and every event onto the closure path.

**Tasks**:
- [ ] Migrate all `ActorHandle` verb call sites (GraphQL resolvers, REST handlers, NoSQL, PgWire's `execute_sql` at `pgwire.rs:110-114`, subscriptions — `read_pool.rs` never calls `ActorHandle`, it only names it in a doc comment at `:25`) to `call`, passing the `DoogatService` method as the closure (depends on: Phase 0) - Acceptance: no caller references `ActorCommand`/`ActorReply`; each migrated verb returns the same type as before (e.g. `apply_schema` still yields `AppOutput<SchemaApplyReport>`); the `Sync`/`Compact`/`GitMaintenance` paths migrate to `call` like every other verb and keep running inline on the write loop for now (00174, decouple-maintenance, moves them to a dedicated worker afterwards).
- [ ] Replace the `match &msg.cmd` event introspection (`mod.rs:678-710`) with reads of the message `EventIntent`; thread intent through the mutating call sites (depends on: Phase 0) - Acceptance: `emit_mutation_events` fires the identical events for create/update/delete/batch-update/create-many/upsert; an event-bus test pins parity.
- [ ] Centralize the schema-reload derivation: one shared helper computes "schema changed" from the core result; the actor bridge calls it once for every actor-routed mutation, retiring all 5 hand-wired trigger sites (`operations.rs:265-267,:307-308,:577-579`, `rest.rs:463-465`, `pgwire.rs:116-117` — PgWire DDL already reaches the actor via `actor.execute_sql` at `pgwire.rs:110-114`, so no PgWire-side call remains) (depends on: Phase 0) - Acceptance: applySchema/executeSql/executeBatch via GraphQL, REST schema-apply, and PgWire DDL each still trigger exactly one reload when (and only when) the schema changed; a test proves a schema-changing mutation on each surface bumps the schema version and a non-schema mutation does not; no per-adapter reload decision remains anywhere, PgWire included.

**Exit Criteria**: all verbs route through `call`; events derive from explicit intent.

### Phase 2: Integration
**Goal**: delete the dead plumbing and prove no behavior change.

**Tasks**:
- [ ] Delete `ActorCommand`, `ActorReply`, `handle_command`, and the 36 `"unexpected reply"` fallbacks; confirm `cargo build`/`cargo clippy --workspace --all-targets`/`test-ci` clean (depends on: Phase 1) - Acceptance: the twin enums and dispatch are gone; the fast tier is green.
- [ ] Apply CLAUDE.md's conditional e2e deletion safety net in Claude-routed sessions (this task replaces existing code paths per CLAUDE.md) and update `docs/src/technical/server.md` + `docs/src/technical/walkthrough.md` to describe the closure actor (depends on: Phase 1) - Acceptance: e2e green; the architecture docs no longer describe the twin-enum dispatch. (Refactor — no CHANGELOG entry, no walkthrough: no user-facing surface change.)

**Exit Criteria**: enums removed, e2e green, docs match; server responses byte-identical to pre-refactor.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: a GraphQL `create` and a REST `search` routed through `call` → Expected: identical responses to the pre-refactor build; the create emits one `Created` event.
- **Edge case**: a batch update and an upsert → Expected: `emit_mutation_events` fires the same batch/upsert events from `EventIntent`, matching the old command-variant introspection.
- **Error case**: a `DoogatService` method returns `Err` inside the closure, and the actor thread is gone → Expected: the error propagates over the `oneshot` unchanged; the dropped-reply path still yields a structured error (never a panic, never `"unexpected reply"` masking).

## Risks

- **Behavior drift during a large mechanical migration**: 36 verbs move at once. Mitigation: keep the enums through Phase 1 so migration is incremental and reversible; verify the 00156 way (diff moved logic); rely on the unchanged server/e2e suites as the oracle.
- **Lost event emission**: the introspection is easy to under-port (batch-update/create-many/upsert are subtle). Mitigation: an event-bus parity test per mutation kind before deleting the `match &msg.cmd` block.
- **`oneshot`/thread teardown semantics**: the current `send` returns a structured error when the actor is gone (`mod.rs:652-659`). Mitigation: preserve that path in `call`; test the actor-dropped case.
- **Closure `Send`/`'static` bounds**: captured borrows must not cross the thread. Mitigation: closures take `&mut DoogatService` as a parameter and own their captures; the compiler enforces the rest.
- **Maintenance stays inline for now**: sync/compact/git-maintenance keep executing on the write loop after this refactor (as closure messages). 00174 (decouple-maintenance) then moves them onto a dedicated worker; building that worker on the closure model instead of the twin enums is why this PRD runs first. Mitigation: none needed here; the worker constraint lives in 00174.

## Transport Impact

Server-internal refactor with no public surface change. Every transport's observable contract is unchanged; conformance = the existing suites stay green.

- **CLI**: Unchanged — does not use the actor (calls `DoogatService` directly); no code touched.
- **GraphQL**: Unchanged — resolvers call `ActorHandle::call` instead of a per-verb method; responses byte-identical.
- **REST**: Unchanged — handlers call `call`; responses byte-identical.
- **PgWire**: Unchanged — its non-SELECT SQL already routes through the actor's `execute_sql` (`pgwire.rs:110-114`); that call moves onto `call` like every other verb and its hand-wired reload trigger (`:116-117`) is retired; responses byte-identical.
- **FFI**: Unchanged — does not use the server actor.
- **NoSQL HTTP**: Unchanged — the three read routes (`nosql_api.rs:29-31`: get/scan/backlinks, fronting four actor verbs) route through `call`; responses byte-identical.
- **Cross-interface conformance plan**: conformance = the existing server integration tests and `tests/e2e/**` (which absorbed the shell scenarios via 00171) all pass unmodified. No new golden workflow is added; the refactor's contract is "the current suites stay green." jink impact: indirect — every future capability jink needs ships faster because routing a verb through the server is now one touch point.
