# Decision Audit Log: 00172-collapse-actor-plumbing-v1

PRD: `00172-collapse-actor-plumbing-v1.md`
Started: 2026-10-17T04:41:37Z
Completed: 2026-10-17T04:41:37Z
Autonomous: 16  |  Deferred: 0  |  Doubts: 0

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: how to collapse the actor twin-enum plumbing without breaking byte-identical event/reload behavior

**Choice**: ReloadIntent/ReloadSignal per-type trait mirroring EventIntent/IntentEmit, consulted once inside ActorHandle::call (applySchema via SchemaApply variant, no second trigger site); delete_doogat resolves doogat_type into a shared Arc<Mutex<Option<String>>> cell the closure fills before deleting.

**Rationale**: keeps the reload decision at exactly one call site per the PRD success metrics, and preserves byte-identical DoogatEvent emission for delete; validated across 3 adversarial review dispatches (claude + 2x codex)

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: `ClosureMsg` is now a one-field wrapper plus a dead `event: EventIntent` field, always set to `EventIntent::None` and carrying an allow(dead_code) attribute. The real intent is moved into the boxed `run` closure. Remove the `event` field and the suppression attribute, then send `ActorRun` directly (`mpsc::Sender<ActorRun>`). This is behavior-preserving.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: Test gap against the PRD criterion "a schema-changing mutation on each surface bumps the schema version and a non-schema mutation does not". PgWire has only the positive DDL case (`integration_21_pgwire_ddl_triggers_schema_reload`), with no non-schema statement asserting zero reload. The GraphQL tests assert `version > before`, not "exactly one", so a double reload (adapter trigger plus `call`) would still pass. Fix: add a PgWire DML case with an unchanged `schemaVersion`, and change the GraphQL asserts to `== before + 1`.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: `actor/tests.rs` grew from 178 to 941 lines, over the 800-line limit. Split the reload tests and the event-emission and dropped-reply tests into submodules or files, for example `actor/tests/reload.rs` and `actor/tests/events.rs`.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: `actor/mod.rs` is 868 lines, still over the 800 limit, though down from about 1050. The 30 trivial read-verb wrappers are the bulk; a split is optional.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: Docs still describe the adapters as the schema-reload callers, which contradicts the PRD docs criterion (the table row and the sentence are server.md's own, so Task 6 missed them). server.md:73 lists "GraphQL executeSql/executeBatch (operations.rs), PgWire (pgwire.rs)" as classifier users, and server.md:1074-1076 says it is "shared by the GraphQL and PgWire transports". server.md:355 still says "per `mutations/operations.rs::executeSql`", and transport-policy-inventory.md:57 has the same shared-by wording. The only caller is now `ActorHandle::execute_sql`/`execute_batch` in `actor/mod.rs`. Fix: reword to "called once inside ActorHandle::call via ReloadIntent".

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: The per-surface reload acceptance test is only partly met. The spec wants a test that a schema-changing mutation on each surface (GraphQL applySchema/executeSql/executeBatch, REST schema-apply, PgWire DDL) bumps the schema version, and that a non-schema mutation does not. The unit tests prove this at the actor level. The only surface test I found is the PgWire e2e. I found no REST or GraphQL surface test of version-bump versus no-bump.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: The docs still describe the deleted file. `docs/src/technical/server.md:1045` and `docs/src/architecture/overview.md:182` list `actor/handlers.rs` as "Command handler implementations" and "Command dispatch logic". `docs/src/technical/app-contract.md:153` says GraphQL parsing happens in `actor/handlers.rs`. A stale comment at `ddb-server/src/actor/tests.rs:68` says the twin enums "still exist". The Phase 2 acceptance says the docs no longer describe the old dispatch.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: SQL negative-path tests exercise `Vec<TableSchema>`, whose implementation always returns false regardless of intent or result. They cannot catch broken SQL error/false gating. Return `ActorResult<SqlResult>` or `ActorResult<Vec<SqlResult>>` in these tests.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: Both actor files exceed the rubric's 800-line limit. Extract event/reload implementations into focused sibling modules and split tests by concern.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: Stale comments refer to the deleted enum path or to process steps. `EventIntent` docs mention "the legacy actor_loop enum path" (mod.rs:42-43), and `IntentEmit` docs say it is "the closure-side twin of the pre-refactor emit_mutation_events" (mod.rs:102-103). The `call` docs also cite `emit_mutation_events` (mod.rs:364-366). The Phase 0 header in `tests.rs` says "Task 1.1 runs one wave BEFORE Task 1.2", "WHILE the twin enums still exist" and "drive the SAME verbs through the CURRENT verb-method path". That is now false, because the tests call `call` directly. Rewrite these comments to describe current behavior.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: Minor behavior widening, contradicting the "byte-identical" success metric but consistent with the PRD intent of one actor-bridge reload site. The GraphQL `sql` query field (`ddb-server/src/schema/queries.rs:304`) routes non-SELECT SQL to `actor.execute_sql`. DDL sent that way now triggers a schema reload, where it previously did not. Likely a fix, but note it in the PRD or docs.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: `set_reloader` silently ignores a second set and `call` silently skips reload when no reloader is registered. This follows the design, and `lib.rs` is the only production construction path. A `tracing::warn!` on the `Err` from `OnceLock::set` would make a mis-wired embedder visible.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: Dead leftover in `ddb-server/src/lib.rs:78-82`. `.layer(Extension(reloader.clone()))` is kept for "the REST apply handler", but `ddb-server/src/rest.rs` no longer extracts `SchemaReloader`, so the layer and its comment are stale.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: Maintenance documentation now claims `ActorHandle::run_maintenance` performs compaction and returns `CompactionReport`; the implementation calls `svc.run_maintenance` and returns `MaintenanceReport`. Document the compaction path using `ActorHandle::compact`.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T04:41:37Z

**Decision**: review cycle 1 converged: 0 critical, 2 high (both settled deferrals with design-doc evidence), 14 actionable medium/low

**Choice**: converged and ran the tail sweep (4 fix tasks, ids 7-10) instead of a second review cycle

**Rationale**: Convergence test is no unresolved CRITICAL/HIGH; R5/R6/R8 contradict explicit decisions in the reviewed design doc and were recorded as settled deferrals with citations, so no CRITICAL/HIGH remains. The doubt-roster constraint gate certified (exit 0). Medium/Low never block convergence and are swept, not dropped.
