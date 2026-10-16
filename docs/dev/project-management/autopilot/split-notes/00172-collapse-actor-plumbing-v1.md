# Split note: 00172-collapse-actor-plumbing-v1.md
planned=6 prd_tasks=6 expansion=1.00 reasons=module_drift

## ddb-server/src (UNLISTED)

- 1 Add ReloadIntent/ReloadSignal and extend ActorHandle::call to 3 args: ddb-server/src/lib.rs
- 4 Delete the 5 hand-wired adapter reload trigger sites and add reload parity test: ddb-server/src/rest.rs, ddb-server/src/pgwire.rs

## docs/src/technical (UNLISTED)

- 6 Update architecture docs to describe the closure actor: docs/src/technical/server.md, docs/src/technical/walkthrough.md

## ddb-server/src/actor (listed)

- 1 Add ReloadIntent/ReloadSignal and extend ActorHandle::call to 3 args: ddb-server/src/actor/mod.rs, ddb-server/src/actor/tests.rs
- 2 Migrate all 37 ActorHandle verb methods onto call: ddb-server/src/actor/mod.rs
- 3 Add event-bus parity test for IntentEmit-derived emission: ddb-server/src/actor/tests.rs
- 5 Delete ActorCommand, ActorReply, handle_command and the dead introspection block: ddb-server/src/actor/mod.rs, ddb-server/src/actor/handlers.rs, ddb-server/src/actor/tests.rs

## ddb-server/src/schema (listed)

- 4 Delete the 5 hand-wired adapter reload trigger sites and add reload parity test: ddb-server/src/schema/mutations/operations.rs
