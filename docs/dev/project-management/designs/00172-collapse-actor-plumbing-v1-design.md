# Design: Collapse actor command/reply plumbing into a closure message

## Architecture fit

Server-internal only; no transport contract changes. Lands entirely in
`ddb-server/src/actor/` (the write-actor transport) plus the five adapter call
sites that currently hand-wire schema reload: `ddb-server/src/schema/mutations/operations.rs`
(GraphQL), `ddb-server/src/rest.rs` (REST), `ddb-server/src/pgwire.rs` (PgWire),
and `ddb-server/src/reload.rs` (the new derivation helper's home). `read_pool.rs`
is untouched code-wise — it already uses the `with_service`/`with_service_mut`
closure idiom the write actor is converging on; only its doc comment at
`read_pool.rs:25` (which names `ActorHandle` as a contrast) may need a one-line
wording update once the contrast is gone.

**Phase 0 (closure transport primitive) is already merged** — commit
`eec6884` (`refactor(server): add closure actor transport beside the enum
path`) added `EventIntent`, `ClosureMsg`/`Envelope`, and
`ActorHandle::call<R>` beside the legacy `ActorCommand`/`ActorReply` enums,
with a unit test (`actor/tests.rs`) driving one read and one mutation through
`call`. This design covers the two phases still open: **Phase 1** (migrate
every verb call site onto `call`, thread `EventIntent`, and add the
schema-reload derivation) and **Phase 2** (delete the dead enums/dispatch,
docs). No Phase 0 work is proposed here.

## Module placement

- **Edit** `ddb-server/src/actor/mod.rs`:
  - Add `ReloadIntent` enum and `ReloadSignal` trait (new, alongside the
    existing `EventIntent`/`IntentEmit`).
  - Add a reloader handle field to `ActorHandle` (new: `reloader:
    Arc<OnceLock<Arc<SchemaReloader>>>`) and a `set_reloader` setter.
  - Extend `ActorHandle::call<R>`'s signature to take a `ReloadIntent` and,
    after the reply is received, consult `ReloadSignal` and fire the reload.
  - Change `EventIntent::Deleted` to carry `{ id: String, doogat_type:
    Arc<Mutex<Option<String>>> }` — a shared cell the delete closure fills in
    (it alone has `&mut DoogatService` to resolve the type with) and `emit`
    reads back after the closure returns, so the emitted `Deleted` event
    still carries the real `doogat_type`, preserving today's behavior exactly
    (see Interfaces & contracts for the full mechanism).
  - Replace every `pub async fn <verb>(&self, ...)` body (37 methods incl.
    `send`) with a `call(event, reload, closure)` call. `send` itself and the
    `ActorCommand`/`ActorReply` enums, `handle_command`, and the `Envelope::Enum`
    arm of `actor_loop` are deleted in Phase 2, once every caller has moved.
  - Delete the `match &msg.cmd` introspection block (`delete_id`/`delete_type`/
    `is_batch_update`/`is_create_many`/`upsert_type`/`mutation_kind`,
    `emit_mutation_events`, the post-dispatch `Upsert` special case) — this is
    already dead the moment every verb emits via `IntentEmit` instead, but it
    physically can't go until `handle_command` goes (same enum).
- **Edit** `ddb-server/src/actor/tests.rs`: the two Phase-0 tests
  (`actor.call(EventIntent::None, |svc| ...)` / `actor.call(EventIntent::Created,
  |svc| {...})` at `tests.rs:102,143`) are 2-arg calls against the current
  signature; both gain a third `ReloadIntent::None` argument in the same task
  that changes `call`'s signature, so the crate keeps compiling at every
  commit. This file is now an explicit edit target, not an omission.
- **Edit** `ddb-server/src/actor/handlers.rs`: deleted whole-file in Phase 2
  (its 36-arm match is `handle_command`, which has no callers once `send` is
  gone).
- **New** `ddb-core::sql_engine::classify::requires_schema_reload` stays
  exactly as-is (string-based DDL classification) — it is *reused*, not
  replaced; see Reuse inventory.
- **New** (small, additive) `ddb-server/src/reload.rs`: add
  `SchemaReloader` is unchanged; `ActorHandle` gets the setter call added
  right after `SchemaReloader::new(...)` in `ddb-server/src/lib.rs` (one new
  line: `actor.set_reloader(reloader.clone())`).
- **Edit** the 5 adapter call sites that currently hand-wire a reload
  trigger — each loses its `if ... { reloader.trigger_reload_and_wait().await; }`
  block entirely, with NOTHING added in its place (the decision moves inside
  the matching `ActorHandle` verb method, covered by the `actor/mod.rs` edit
  above, not into the adapter):
  - `ddb-server/src/schema/mutations/operations.rs:262-269` (`executeSql`)
  - `ddb-server/src/schema/mutations/operations.rs:296-310` (`executeBatch`)
  - `ddb-server/src/schema/mutations/operations.rs:568-581` (`applySchema`)
  - `ddb-server/src/rest.rs:454-467` (REST `apply_schema`)
  - `ddb-server/src/pgwire.rs:102-121` (non-SELECT `execute_sql` in PgWire)
- **No other adapter edits.** Every GraphQL resolver, REST handler, and
  NoSQL/PgWire call site keeps calling the same `ActorHandle::<verb>(...)`
  method with the same arguments, unchanged — see Resolver/handler-side
  change below. Only the 37 `ActorHandle` method bodies (covered above) move
  onto `call`; adapters never call `ActorHandle::call` directly.

## Interfaces & contracts

### `ReloadIntent` (new enum, `ddb-server/src/actor/mod.rs`)

```rust
/// Explicit schema-reload signal carried alongside a verb's closure, mirroring
/// EventIntent. Decided by the caller (it knows which verb this is); the actual
/// "did it change" decision is made centrally, once, inside `call` — this is
/// the ONE derivation site the PRD's Success Metrics require, covering every
/// actor-routed mutation including `applySchema`, with no second trigger path
/// anywhere else.
pub enum ReloadIntent {
    /// Nothing can change the schema: no reload check, regardless of result.
    None,
    /// Caller has already decided, from SQL text classified via
    /// `requires_schema_reload` before the write ran (over one statement for
    /// `execute_sql`/PgWire, or `.any(...)` over every statement for
    /// `execute_batch`). Reload iff `self.is_ok() && bool`.
    Explicit(bool),
    /// `applySchema` only: reload iff the result is
    /// `Ok(AppOutput<SchemaApplyReport>)` AND `!dry_run && value.applied`.
    /// `dry_run` travels on the intent because it is not part of the result;
    /// `applied` is read from the result itself by the type-specific
    /// `ReloadSignal` impl below — this is the one verb whose signal is only
    /// knowable from its own output, not from anything available before the
    /// write runs.
    SchemaApply { dry_run: bool },
}
```

### `ReloadSignal` (new trait, `ddb-server/src/actor/mod.rs`)

```rust
/// Decide, from a verb's actual result and its declared [`ReloadIntent`],
/// whether the schema changed. Mirrors `IntentEmit`'s shape exactly: a
/// handful of concrete, non-overlapping impls plus a `no_reload_intent!`
/// macro for every verb that never reloads — NOT a single blanket
/// `impl<T> ReloadSignal for Result<T, DoogatError>`. A blanket impl would
/// make a later type-specific override (e.g. for `SchemaApplyReport`)
/// impossible under Rust's coherence rules (a concrete impl for
/// `Result<AppOutput<SchemaApplyReport>, DoogatError>` would conflict with
/// an already-present `impl<T>`), and `applySchema` genuinely needs to read
/// `.value.applied` — a field no type-blind impl can see. Per-type impls
/// keep that override legal and keep every reload decision inside this one
/// trait, consulted from the one site inside `call`.
pub trait ReloadSignal {
    fn schema_changed(&self, intent: &ReloadIntent) -> bool;
}

// SQL-text verbs: execute_sql / execute_batch. Reload iff Ok and the
// pre-classified `Explicit(true)` was passed.
impl ReloadSignal for Result<SqlResult, DoogatError> {
    fn schema_changed(&self, intent: &ReloadIntent) -> bool {
        matches!(intent, ReloadIntent::Explicit(true)) && self.is_ok()
    }
}
impl ReloadSignal for Result<Vec<SqlResult>, DoogatError> {
    fn schema_changed(&self, intent: &ReloadIntent) -> bool {
        matches!(intent, ReloadIntent::Explicit(true)) && self.is_ok()
    }
}

// applySchema: the one verb whose signal comes from its own result.
impl ReloadSignal for Result<AppOutput<SchemaApplyReport>, DoogatError> {
    fn schema_changed(&self, intent: &ReloadIntent) -> bool {
        matches!(intent, ReloadIntent::SchemaApply { dry_run: false })
            && matches!(self, Ok(o) if o.value.applied)
    }
}

/// Every other verb return type never reloads, regardless of intent — see
/// the note below this block for exactly which types and why this list is
/// NOT identical to `no_emit_intent!`'s.
macro_rules! no_reload_intent {
    ($($ty:ty),* $(,)?) => {
        $(
            impl ReloadSignal for Result<$ty, DoogatError> {
                fn schema_changed(&self, _intent: &ReloadIntent) -> bool { false }
            }
        )*
    };
}
no_reload_intent!(
    (), bool, i64, ParsedDoogat, Option<ParsedDoogat>, Vec<ParsedDoogat>,
    AppOutput<ParsedDoogat>, PaginatedSearchResult, Vec<TableSchema>, Vec<String>,
    ddb_core::types::AttachmentInfo, Vec<ddb_core::types::AttachmentInfo>,
    CompactionReport, MaintenanceReport, SyncReport, Vec<UnlinkedMention>,
    Vec<Suggestion>, Vec<StaleDoogat>, Vec<OrphanDoogat>, SequenceInfo,
    Vec<SequenceNode>, Vec<BrokenSequence>, UpsertOutcome,
);
```

This list is every `no_emit_intent!` type (`actor/mod.rs:321-345`) MINUS
`SqlResult`, `Vec<SqlResult>`, and `AppOutput<SchemaApplyReport>` (which have
their own concrete `ReloadSignal` impls above) PLUS `AppOutput<ParsedDoogat>`
— `create_doogat`/`update_doogat`'s return type, which `no_emit_intent!`
never lists (it gets its own `IntentEmit` impl for `Created`/`Updated`
instead) but which still needs a `ReloadSignal` impl since creating or
updating a doogat never changes the schema. The two macro lists are
deliberately NOT identical — `no_reload_intent!`'s list is "every verb
return type minus the ones with a concrete reload-specific impl",
`no_emit_intent!`'s is "every verb return type minus the ones with a
concrete event-specific impl" — and the two "ones with a concrete impl" sets
differ (`AppOutput<ParsedDoogat>`/`Vec<ParsedDoogat>` are event-specific but
not reload-specific; `AppOutput<SchemaApplyReport>` is reload-specific but
not event-specific). Enumerated explicitly here, not derived, exactly like
`no_emit_intent!` is today.
```

`apply_schema`'s own body now looks exactly like every other migrated verb —
`self.call(EventIntent::None, ReloadIntent::SchemaApply { dry_run }, f).await`
— with the reload check happening inside `call`, the same single site every
other verb uses. There is no second trigger path.

### `ActorHandle` additions

```rust
pub struct ActorHandle {
    tx: mpsc::Sender<Envelope>,
    event_bus: EventBus,
    reloader: Arc<OnceLock<Arc<SchemaReloader>>>, // new; shared across clones
}

impl ActorHandle {
    /// Wire the reloader in once, after `SchemaReloader::new(actor.clone(), ...)`
    /// returns (construction order: actor spawns first, reloader is built from
    /// a clone of it — see `ddb-server/src/lib.rs`). A second call is a no-op
    /// (`OnceLock::set` returns `Err`, intentionally ignored: the reloader
    /// never changes identity after startup).
    pub fn set_reloader(&self, reloader: Arc<SchemaReloader>) {
        let _ = self.reloader.set(reloader);
    }

    pub async fn call<R, F>(&self, event: EventIntent, reload: ReloadIntent, f: F) -> R
    where
        F: FnOnce(&mut DoogatService) -> R + Send + 'static,
        R: FromActorError + IntentEmit + ReloadSignal + Send + 'static,
    {
        // ... unchanged allocation/send/await machinery ...
        let result = /* reply_rx.await.unwrap_or_else(...) as today */;
        if result.schema_changed(&reload) {
            if let Some(reloader) = self.reloader.get() {
                reloader.trigger_reload_and_wait().await;
            }
        }
        result
    }
}
```

`reloader` is `Option`-shaped via `OnceLock` rather than a constructor
argument because `SchemaReloader::new` needs an already-spawned
`ActorHandle` (circular otherwise) and because `ddb-server`'s test harness
constructs `ActorHandle` without ever building a `SchemaReloader` — those
tests must keep compiling with zero reload wiring. `Arc<OnceLock<...>>`
(not a bare `OnceLock`) because `ActorHandle` is `#[derive(Clone)]` and every
clone must observe the same reloader once `set_reloader` runs.

### Verb migration pattern (one example per return-type family; all 36 follow the same shape)

```rust
// Read, no event, no reload (18 of the 36 pre-migration methods):
pub async fn get_doogat(&self, id: String) -> ActorResult<ParsedDoogat> {
    self.call(EventIntent::None, ReloadIntent::None, move |svc| svc.get_doogat_parsed(&id)).await
}

// Mutation, event, no reload:
pub async fn create_doogat(&self, title: Option<String>, /* ... */) -> ActorResult<AppOutput<ParsedDoogat>> {
    self.call(EventIntent::Created, ReloadIntent::None, move |svc| {
        svc.create(CreateCommand { title, /* ...same fields handlers.rs builds today... */
            unregistered_type_policy: UnregisteredTypePolicy::Strict })
    }).await
}

// delete: the caller cannot resolve doogat_type before the closure runs (it
// has no &DoogatService) and the row is gone once the closure returns — so
// the closure resolves the type itself, into a shared cell, before deleting.
// EventIntent::Deleted carries that cell; `emit`'s Deleted arm reads it back
// after the closure has run (both execute on the actor thread, sequentially,
// before the reply is sent — no synchronization race):
pub async fn delete_doogat(&self, id: String) -> ActorResult<()> {
    let doogat_type: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let doogat_type_for_closure = doogat_type.clone();
    self.call(
        EventIntent::Deleted { id: id.clone(), doogat_type },
        ReloadIntent::None,
        move |svc| {
            let resolved = svc
                .get_doogat_parsed(&id)
                .ok()
                .and_then(|z| z.meta.doogat_type);
            *doogat_type_for_closure.lock().unwrap() = resolved;
            svc.delete_doogat(&id, &format!("delete doogat {id}"))
                .map(|_broken| ())
        },
    ).await
}
// This is a literal relocation of actor_loop's existing pre-dispatch lookup
// (mod.rs:944-952: `svc.get_doogat_parsed(id).ok().and_then(|z|
// z.meta.doogat_type)`, run before `handle_command` dispatches the delete)
// into the closure that now IS the dispatch — same lookup, same ordering
// relative to the delete, same resulting DoogatEvent.doogat_type. No event
// field is dropped or changed.

// SQL, reload pre-classified from text (no result-shape coupling needed):
pub async fn execute_sql(&self, sql: String) -> ActorResult<SqlResult> {
    let reload = ReloadIntent::Explicit(requires_schema_reload(&sql));
    self.call(EventIntent::None, reload, move |svc| svc.execute_sql(&sql)).await
}

// executeBatch: same idea, classified once across every statement:
pub async fn execute_batch(&self, statements: Vec<String>) -> ActorResult<Vec<SqlResult>> {
    let reload = ReloadIntent::Explicit(statements.iter().any(|s| requires_schema_reload(s)));
    let combined = statements.join(";\n");
    self.call(EventIntent::None, reload, move |svc| svc.execute_batch(&combined)).await
}

// applySchema: uses call's generic reload machinery exactly like every other
// verb — ONE site, no second trigger path:
pub async fn apply_schema(&self, schema_doc: String, dry_run: bool, allow_destructive: bool)
    -> ActorResult<AppOutput<SchemaApplyReport>>
{
    self.call(
        EventIntent::None,
        ReloadIntent::SchemaApply { dry_run },
        move |svc| svc.apply_schema(ApplySchemaCommand { schema_doc, dry_run, allow_destructive }),
    ).await
}
```

Every closure body is copied verbatim from the matching arm in today's
`handlers::handle_command` (see Reuse inventory) — this is a relocation, not
a rewrite, which is what keeps behavior byte-identical.

### Resolver/handler-side change

Every GraphQL resolver, REST handler, and NoSQL/PgWire call site keeps calling
`a.execute_sql(sql).await` / `a.apply_schema(...).await` etc. **unchanged** —
the signature of every public `ActorHandle` verb method stays the same; only
its body moves onto `call`. The adapters lose only their own
`if requires_schema_reload(...) { reloader.trigger_reload_and_wait().await }`
/ `if !dry_run && output.value.applied { ... }` blocks (5 sites, enumerated in
Module placement), because that decision now happens inside `call` itself.
PgWire additionally drops its own `self.reloader` field if nothing else in
that file reads it after this change (confirm at task time with `rg
"\.reloader" ddb-server/src/pgwire.rs` — if `reloader` is otherwise unused the
field and its constructor argument are dead code to remove in the same task,
not a separate cleanup).

## Data flow

1. Adapter (GraphQL/REST/PgWire/NoSQL) builds its verb's args and calls the
   unchanged `ActorHandle::<verb>(...)` method.
2. That method computes its `EventIntent` and `ReloadIntent` (both pure,
   caller-local — no I/O) and calls `self.call(event, reload, closure)`.
3. `call` allocates the oneshot, boxes `closure` + an `emit` step, sends it
   over the existing `mpsc` channel to `actor_loop`.
4. `actor_loop` (unchanged after Phase 2 deletes the enum arm) runs the
   closure against `&mut DoogatService`, which calls `result.emit(bus,
   &event)` synchronously on the actor thread (as today, Phase 0), then
   replies `R` over the oneshot.
5. Back on the async side, `call` awaits the reply, then (NEW) evaluates
   `result.schema_changed(&reload)` and conditionally `.await`s
   `reloader.trigger_reload_and_wait()` — this is why the reload decision
   must live in `call` and not inside the boxed closure: triggering reload is
   async, and the boxed closure runs synchronously on a blocking OS thread.
6. `call` returns `R` to the adapter exactly as today.

## Reuse inventory

- `ddb_core::sql_engine::classify::requires_schema_reload(sql: &str) -> bool` —
  reused unchanged as the text-classification half of `ReloadSignal`. Greps
  tried: `requires_schema_reload` (found, `ddb-core/src/sql_engine/classify.rs:28`),
  confirming no second implementation exists to consolidate.
- `EventIntent` / `IntentEmit` (Phase 0, `actor/mod.rs:217-400`) —
  the reload mechanism is deliberately shaped as a sibling pattern
  (`ReloadIntent`/`ReloadSignal`) rather than a new idiom, since the PRD
  explicitly wants "the same closure-over-`&mut
  DoogatService` idiom" applied uniformly.
  Greps tried: `grep -rn "IntentEmit\|EventIntent" ddb-server/src/actor/` (the
  existing impls, read in full above).
- `handlers::handle_command`'s 36 match arms — reused as the literal source
  of every migrated closure body (copy, not reimplement). Greps tried: read
  `ddb-server/src/actor/handlers.rs` in full (202 lines, all 36 arms).
- `SchemaReloader::trigger_reload_and_wait` / `SchemaReloader::new` — reused
  unchanged; only one new caller line (`set_reloader`) in `lib.rs`. Greps
  tried: `trigger_reload_and_wait`, `SchemaReloader::new` (both found,
  `reload.rs:26,64`).
- `read_pool.rs`'s `with_service`/`with_service_mut` closures — the PRD's own
  "prior art" for this idiom; not reused as code (different pool, no actor
  channel) but confirms the shape is already proven at 22 call sites. Greps
  tried: `with_service` in `read_pool.rs`.

## Alternatives considered

1. **(Chosen) `ReloadIntent` enum + `ReloadSignal` trait, per-type impls plus
   a `no_reload_intent!` macro, decided in ONE place inside `call` —
   `apply_schema` included, via `ReloadIntent::SchemaApply`.** Mirrors
   `IntentEmit`'s existing shape exactly rather than inventing a second
   pattern, and keeps every verb's reload decision — `applySchema`'s
   included — behind the same single `call` site; nothing triggers reload a
   second time anywhere else. Extra cost over option 2: one new enum
   (3 variants), one new trait, ~4 concrete impls, one macro invocation
   (~50 LOC) versus zero.
2. **Smallest-diff: keep the 5 hand-wired `if ... trigger_reload_and_wait()`
   blocks exactly where they are, migrate only the enum→closure part.**
   Rejected: this PRD's own Success Metrics require the reload decision to
   collapse to ONE derivation helper with ONE call site ("Zero hand-wired
   per-adapter reload triggers remain, pgwire.rs:116-117 included") — keeping
   them would leave that metric unmet while still doing the (larger) enum
   deletion work. Not smaller in the dimension that matters (adapter touch
   points); only smaller in diff lines, and the PRD explicitly prices that
   trade against future PRDs (00175-00178) that each add a new adapter touch
   point if the duplication survives.
3. **Push the SQL-text classification into the closure itself** (classify
   before calling `svc.execute_sql`, pass the boolean as part of a generic
   `ReloadIntent::Explicit(bool)` computed inside the closure, smuggled out
   via a side channel). Rejected: the closure runs on the blocking actor
   thread and its return type `R` is the verb's own result — there is no
   channel to smuggle a second boolean out without changing every verb's
   return type to a tuple, which breaks the "same type as before" acceptance
   criterion in Phase 1 task 1. Deciding reload from the *already-returned*
   `R` (option 1) avoids this entirely.

## Risks & edge cases

- **`delete_doogat`'s `doogat_type` — resolved in Interfaces & contracts, not
  deferred here.** `EventIntent::Deleted` carries a shared
  `Arc<Mutex<Option<String>>>` the closure fills in before deleting (the
  closure has `&mut DoogatService`; the caller doesn't); `emit` reads it back
  after the closure returns. The emitted event's `doogat_type` is unchanged
  from today, not dropped — see the `delete_doogat` example and its
  relocation note in Interfaces & contracts.
- **`no_reload_intent!`'s type list must be audited against the full verb
  list by hand, and is NOT a copy of `no_emit_intent!`'s** (see the note
  under the macro in Interfaces & contracts for the exact set difference:
  `AppOutput<ParsedDoogat>` needs a no-reload impl despite having no
  no-emit impl, and `SqlResult`/`Vec<SqlResult>`/`AppOutput<SchemaApplyReport>`
  need a no-emit impl despite having a reload-specific impl instead of a
  no-reload one). Call this out explicitly in the Phase 1 task that adds the
  macro so the implementor checks every one of the 36 verbs' return types
  against both macro invocations, not just mirrors one list into the other.
- **`executeBatch`'s batch-of-statements reload text — resolved in
  Interfaces & contracts.** `ReloadIntent::Explicit(bool)` takes the
  pre-classified boolean directly; `execute_batch`'s own body folds
  `.any(requires_schema_reload)` across every statement before calling
  `call`, so there is no per-statement coupling inside the trait at all.
- **PgWire's `reloader` field may become dead.** Confirm before deleting —
  see Module placement note.
- **Likely next changes this design should not box in**: (1) 00174
  (decouple-maintenance) moves `Compact`/`GitMaintenance`/`Sync` off the write
  loop onto a dedicated worker — those three verbs keep running inline on the
  closure path for now (PRD explicitly defers this), so their `call` sites
  must not assume they stay on this actor's thread forever; nothing in this
  design pins that assumption. (2) 00175-00178 each add new verbs through
  `call` directly (one touch point) — the `ReloadIntent`/`EventIntent` pair
  must stay the only two parameters a new verb needs to declare, no third
  "intent" axis should creep in without good reason. (3) A future verb that
  needs a reload signal from something other than SQL text or
  `SchemaApplyReport.applied` will need a third `ReloadSignal` impl, not a
  third enum — the trait is the extension point, not the enum.

## Test strategy outline

- **Unit (colocated, `actor/tests.rs`)**: update the two existing Phase 0 call
  sites for the new 3-arg `call` signature (`ReloadIntent::None`), and add
  coverage for: (a) `ReloadIntent::Explicit(true)` triggers
  `trigger_reload_and_wait` exactly once on an `Ok` result, via `call`'s one
  reload check; (b) `ReloadIntent::SchemaApply { dry_run: false }` does the
  same iff the result's `value.applied` is true, and a `dry_run: true` or
  `applied: false` result triggers zero reloads; (c) `delete_doogat` emits
  exactly one `Deleted` event carrying the resolved `doogat_type` (regression
  pin for the shared-cell mechanism above); (d) actor-gone and dropped-reply
  still return the structured `FromActorError` values for every migrated
  verb family (read, create-shape, list-shape, SQL-shape).
- **Event-bus parity test** (Phase 1 task 2's own acceptance criterion):
  one test per mutation kind (create/update/delete/batch-update/create-many/
  upsert) asserting `emit_mutation_events`'s old behavior and the new
  `IntentEmit`-derived behavior produce the identical `DoogatEvent` sequence —
  this already has Phase 0 groundwork; extend it to the verbs Phase 1 migrates.
- **Reload parity test** (Phase 1 task 3's own acceptance criterion): drive
  `executeSql`/`executeBatch`/PgWire-DDL/`applySchema` through GraphQL, REST,
  and PgWire respectively; assert exactly one reload (schema version bump)
  for a schema-changing statement and zero for a non-schema-changing one, on
  each of the three surfaces.
- **Existing server integration + `tests/e2e/**` suites are the regression
  oracle** (per the PRD's Success Metrics: "every existing server/e2e test
  passes unchanged") — no new golden workflow; Tier 1 (`cargo build`,
  `cargo clippy --workspace --all-targets`, `cargo test-ci`) runs per task, the
  e2e suite runs under Tier 2 CI except the CLAUDE.md conditional run for the
  Phase 2 deletion task (it deletes/replaces `handlers.rs` and the enum
  variants, so `cargo test -p ddb-e2e` runs locally for that task per
  CLAUDE.md's safety net).
- **Diff-the-moved-logic verification** (the PRD's "00156 way"): for each
  migrated verb, diff the closure body against the deleted `handle_command`
  arm it replaces — this is a review-time check (the Phase 1 task reviewer
  confirms byte-identical logic), not an automated test; call it out
  explicitly in the Phase 1/Phase 2 task descriptions so the implementor and
  reviewer both know to do it.

## Review log

- question: double `matches!` in `ReloadSignal::schema_changed` for SqlText reads as unresolved pseudocode — resolved: `SqlText` variant removed entirely in favor of `Explicit(bool)`.
dispatch 1 (claude): cardinal-sin 0, blocker 4, non-blocker 1, question 1
- non-blocker: ReloadSignal's per-type impl list must be kept in sync with IntentEmit's no_emit_intent! list by hand — accepted as a known, called-out risk (Risks & edge cases), same shape IntentEmit already carries.
dispatch 2 (codex): cardinal-sin 0, blocker 3, non-blocker 2, question 0
dispatch 3 (codex): cardinal-sin 0, blocker 1, non-blocker 0, question 0 — fixed: no_reload_intent! omitted AppOutput<ParsedDoogat> (create_doogat/update_doogat's return type), which no concrete ReloadSignal impl covered; added it, and documented the exact (non-identical) relationship between no_reload_intent!'s and no_emit_intent!'s type lists.

