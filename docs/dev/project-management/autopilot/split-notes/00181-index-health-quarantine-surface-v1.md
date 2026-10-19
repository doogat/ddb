# Split note: 00181-index-health-quarantine-surface-v1.md
planned=7 prd_tasks=4 expansion=1.75 reasons=module_drift

## . (UNLISTED)

- 7 Docs + CHANGELOG + walkthrough for the doctor/indexHealth surface: CHANGELOG.md

## ddb-cli/src (UNLISTED)

- 2 CLI: ddb doctor command: ddb-cli/src/main.rs

## ddb-core/src/app_contract (UNLISTED)

- 1 Foundation: IndexHealthCommand/IndexHealthResult + service/ReadPool verb: ddb-core/src/app_contract/commands.rs, ddb-core/src/app_contract/results.rs, ddb-core/src/app_contract/mod.rs

## ddb-server/src (UNLISTED)

- 1 Foundation: IndexHealthCommand/IndexHealthResult + service/ReadPool verb: ddb-server/src/read_pool.rs
- 4 REST: GET /health/index handler: ddb-server/src/rest.rs

## dev/local/walkthroughs (UNLISTED)

- 7 Docs + CHANGELOG + walkthrough for the doctor/indexHealth surface: dev/local/walkthroughs/00181-ddb-doctor.md

## docs/src/technical (UNLISTED)

- 7 Docs + CHANGELOG + walkthrough for the doctor/indexHealth surface: docs/src/technical/indexer.md, docs/src/technical/server.md

## tests/e2e (UNLISTED)

- 6 Parity: cross-interface e2e scenario for quarantine listing: tests/e2e/main.rs, tests/e2e/index_health_parity.rs

## ddb-cli/src/commands (listed)

- 2 CLI: ddb doctor command: ddb-cli/src/commands/maintenance.rs

## ddb-core/src/ffi (listed)

- 5 FFI: index_health driver method + records: ddb-core/src/ffi/driver.rs, ddb-core/src/ffi/records.rs

## ddb-core/src/service (listed)

- 1 Foundation: IndexHealthCommand/IndexHealthResult + service/ReadPool verb: ddb-core/src/service/utility.rs

## ddb-server/src/schema (listed)

- 3 GraphQL: indexHealth query: ddb-server/src/schema/discovery_queries.rs
