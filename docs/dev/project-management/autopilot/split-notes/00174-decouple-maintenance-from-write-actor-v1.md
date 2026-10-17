# Split note: 00174-decouple-maintenance-from-write-actor-v1.md
planned=4 prd_tasks=5 expansion=0.80 reasons=module_drift

## . (UNLISTED)

- 4 Document the maintenance worker, remote timeout, and NoSQL constraint; register the new e2e scenarios: CHANGELOG.md

## ddb-core/src (UNLISTED)

- 1 Bound remote fetch/push with a configurable timeout and REMOTE_TIMEOUT error: ddb-core/src/error.rs
- 3 Confirm NoSQL reads are unblocked by the decoupling; document the redb exclusive-open constraint: ddb-core/src/nosql.rs

## ddb-core/src/app_contract (UNLISTED)

- 1 Bound remote fetch/push with a configurable timeout and REMOTE_TIMEOUT error: ddb-core/src/app_contract/error.rs

## ddb-server/src (UNLISTED)

- 1 Bound remote fetch/push with a configurable timeout and REMOTE_TIMEOUT error: ddb-server/src/config.rs, ddb-server/src/lib.rs

## docs/src/technical (UNLISTED)

- 4 Document the maintenance worker, remote timeout, and NoSQL constraint; register the new e2e scenarios: docs/src/technical/server.md, docs/src/technical/sync.md

## tests/e2e (UNLISTED)

- 2 Spawn a dedicated maintenance worker, routing Sync/Compact/GitMaintenance off the write-actor thread: tests/e2e/integration_sync_decoupling.rs
- 3 Confirm NoSQL reads are unblocked by the decoupling; document the redb exclusive-open constraint: tests/e2e/integration_nosql_during_compact.rs
- 4 Document the maintenance worker, remote timeout, and NoSQL constraint; register the new e2e scenarios: tests/e2e/main.rs

## ddb-core/src/git_ops (listed)

- 1 Bound remote fetch/push with a configurable timeout and REMOTE_TIMEOUT error: ddb-core/src/git_ops/remote.rs

## ddb-server/src/actor (listed)

- 2 Spawn a dedicated maintenance worker, routing Sync/Compact/GitMaintenance off the write-actor thread: ddb-server/src/actor/mod.rs
