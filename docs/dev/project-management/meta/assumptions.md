# Assumption ledger — PRD 00172-collapse-actor-plumbing-v1

## 1: Add ReloadIntent/ReloadSignal and extend ActorHandle::call to 3 args

Devon's adversarial pass (opus):

- One combined wrong implementation run once is enough proof for all 5 weak points, instead of one run per exploit.
- `list_type_schemas` exists on DoogatService and `SchemaApplyReport` has `applied` and `ops` fields; the clean compile confirmed both.
- Weak points 4 and 5 cover behavior the brief does not define; reported as gaps without a correct answer to check against.

Tess (strengthen retry, opus):

- A dry-run apply of a brand-new type returns a non-empty `ops` list with `applied == false`.
- Point 5 (double reloader registration): OnceLock set-once semantics are the intended contract; inferred, not stated. If a later task wants "last registration wins", it needs its own test.

Ivan (initial implementation, opus):

- Held the reloader with a strong `Arc` inside the handle (reference cycle with the actor; acceptable for a process-lifetime server).
- Assumed `ReloadIntent::Explicit(true)` reloads on any `Ok`, whatever the result type — **corrected during per-task review**: the task's literal contract scopes `Explicit(true)` to `SqlResult`/`Vec<SqlResult>` only; every other type never reloads regardless of intent (`no_reload_intent!`). The orchestrator fixed this directly (mod.rs + two tests) after Pat's per-task review raised it as two MEDIUM findings; both closed on re-review.
- A poisoned lock is recovered with `into_inner()` rather than returning an error — moot after the review fix replaced the lock with `OnceLock`.
- Did not wire `actor.set_reloader(...)` in `ddb-server/src/lib.rs` despite the task naming it explicitly; the orchestrator added the one-line wiring directly.

## 2: Migrate all 37 ActorHandle verb methods onto call

Ivan (opus):

- Changing `EventIntent::Deleted`'s `doogat_type` field from `Option<String>` to `Arc<Mutex<Option<String>>>` is in scope, because the contract requires the shared cell and the change stays inside mod.rs.
- Removing the now-orphaned `ActorHandle::send` is allowed, because it is not one of the items the task protects (`ActorCommand`, `ActorReply`, `handle_command`, the `match &msg.cmd` block).
- Poison-tolerant locking (`unwrap_or_else(|e| e.into_inner())`) replaces the contract snippet's bare `.unwrap()`, to follow the no-panics guardrail.
- Flagged, not fixed (out of this task's scope): a possible double schema-reload at `pgwire.rs:116` and `schema/mutations/operations.rs:265/301`, which still call `requires_schema_reload` and trigger their own reload directly, now alongside `call`'s built-in reload machinery. This is task 4's scope.

Orchestrator, resolving Ivan's `send`-removal choice:

- Removing `send` orphaned the `Envelope::Enum` variant, producing one `dead_code` clippy warning. Verified empirically that restoring `send` makes it worse (clippy then flags `send` itself as unused too, since nothing legitimately calls it under this task's acceptance criteria). Left the single warning in place as a known, task-sequenced gap: task 5 in this PRD deletes `ActorCommand`/`ActorReply`/`handle_command`/the dead introspection block, which resolves it. Confirmed this never reaches CI's `-D warnings` gate because this repo pushes `master` once per PRD (after task 6), not per task.

## 10: [D1] prose (PRD 00173, review cycle 1 rework)

Ivan (opus):

- "The per-code status table" in `errors.md#unified-transport-rendering-policy-prd-00173` means that section as a whole; the statuses are prose bullets there, not a literal Markdown table, so the link names the section the way server.md:82 already does.
- Named the catch-all row's variants (`Git`, `Yaml`, `Sql`, `Io`, "and the other internal variants") instead of keeping a bare "All others", to make the exclusion of `Parse` and `Structured` explicit.
- Cited `SCHEMA_APPLY_PARTIAL` as the example registered `Internal` code in the new server.md `Structured` row, taking its registration from the dispatch's verified facts rather than reading `app_contract/error.rs` (not on the allowlist).
- Changed the errors.md note's opening count from "two" to "several" rather than picking a new exact number, since the changes now span statuses, redaction, and two GraphQL paths.
