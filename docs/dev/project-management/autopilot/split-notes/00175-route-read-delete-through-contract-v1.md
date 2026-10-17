# Split note: 00175-route-read-delete-through-contract-v1.md
planned=5 prd_tasks=5 expansion=1.00 reasons=module_drift

## . (UNLISTED)

- 5 Author e2e scenarios, docs, CHANGELOG, and walkthrough for read/delete contract routing: CHANGELOG.md

## ddb-core/src/app_contract (UNLISTED)

- 1 Route service read/delete through the app contract: ddb-core/src/app_contract/results.rs, ddb-core/src/app_contract/commands.rs

## ddb-server/src (UNLISTED)

- 2 Route CLI, GraphQL, and REST read/delete through the command: ddb-server/src/read_pool.rs, ddb-server/src/rest.rs

## ddb-server/src/actor (UNLISTED)

- 2 Route CLI, GraphQL, and REST read/delete through the command: ddb-server/src/actor/mod.rs

## ddb-server/src/schema (UNLISTED)

- 2 Route CLI, GraphQL, and REST read/delete through the command: ddb-server/src/schema/queries.rs, ddb-server/src/schema/mod.rs

## ddb-server/src/schema/mutations (UNLISTED)

- 2 Route CLI, GraphQL, and REST read/delete through the command: ddb-server/src/schema/mutations/operations.rs, ddb-server/src/schema/mutations/types.rs

## docs/src/technical (UNLISTED)

- 5 Author e2e scenarios, docs, CHANGELOG, and walkthrough for read/delete contract routing: docs/src/technical/app-contract.md, docs/src/technical/server.md, docs/src/technical/rest-api.md, docs/src/technical/ffi.md, docs/src/technical/conformance-harness.md

## tests/e2e (UNLISTED)

- 5 Author e2e scenarios, docs, CHANGELOG, and walkthrough for read/delete contract routing: tests/e2e/main.rs

## tests/e2e/conformance (UNLISTED)

- 4 Add REST conformance driver and read/delete parity step: tests/e2e/conformance/driver_rest.rs, tests/e2e/conformance/cross_driver_crud.rs, tests/e2e/conformance/mod.rs

## ddb-cli/src/commands (listed)

- 2 Route CLI, GraphQL, and REST read/delete through the command: ddb-cli/src/commands/crud.rs

## ddb-core/src/ffi (listed)

- 3 Add FFI read_doogat/delete_doogat records embedding warnings: ddb-core/src/ffi/records.rs, ddb-core/src/ffi/driver.rs

## ddb-core/src/service (listed)

- 1 Route service read/delete through the app contract: ddb-core/src/service/read.rs, ddb-core/src/service/delete.rs
