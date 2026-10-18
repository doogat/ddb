# Split note: 00176-route-search-through-contract-v1.md
planned=6 prd_tasks=6 expansion=1.00 reasons=module_drift

## . (UNLISTED)

- 6 e2e scenarios + docs + walkthrough + CHANGELOG for contract-routed search (Phase 2): CHANGELOG.md

## ddb-cli/src (UNLISTED)

- 2 Route GraphQL + CLI search through SearchCommand (Phase 1): ddb-cli/src/main.rs

## ddb-server/src (UNLISTED)

- 2 Route GraphQL + CLI search through SearchCommand (Phase 1): ddb-server/src/read_pool.rs
- 3 REST: filter parity + full hit shape + PageRequest-based pagination (Phase 1): ddb-server/src/rest.rs

## dev/local (UNLISTED)

- 6 e2e scenarios + docs + walkthrough + CHANGELOG for contract-routed search (Phase 2): dev/local/walkthroughs

## docs/src/technical (UNLISTED)

- 6 e2e scenarios + docs + walkthrough + CHANGELOG for contract-routed search (Phase 2): docs/src/technical/app-contract.md, docs/src/technical/server.md, docs/src/technical/rest-api.md, docs/src/technical/ffi.md, docs/src/technical/indexer.md, docs/src/technical/transport-policy-inventory.md, docs/src/technical/compatibility-checklist.md

## tests/e2e (UNLISTED)

- 6 e2e scenarios + docs + walkthrough + CHANGELOG for contract-routed search (Phase 2): tests/e2e/main.rs, tests/e2e/integration_rest_search_filtered.rs, tests/e2e/smoke_search.rs

## tests/e2e/conformance (UNLISTED)

- 5 Conformance harness: StepOp::Search type/tag filter + full-field-set assertion (Phase 2): tests/e2e/conformance/fixture.rs, tests/e2e/conformance/cross_driver_crud.rs, tests/e2e/conformance/driver_graphql.rs, tests/e2e/conformance/driver_cli.rs

## ddb-cli/src/commands (listed)

- 2 Route GraphQL + CLI search through SearchCommand (Phase 1): ddb-cli/src/commands/query.rs

## ddb-core/src/app_contract (listed)

- 1 Extend SearchCommand + PageRequest + search_command facade (Phase 0 foundation): ddb-core/src/app_contract/commands.rs, ddb-core/src/app_contract/results.rs, ddb-core/src/app_contract/mod.rs

## ddb-core/src/ffi (listed)

- 4 FFI: filtered search + widened record + reimplemented search_paginated (Phase 1): ddb-core/src/ffi/driver.rs, ddb-core/src/ffi/records.rs
- 6 e2e scenarios + docs + walkthrough + CHANGELOG for contract-routed search (Phase 2): ddb-core/src/ffi/driver.rs

## ddb-core/src/service (listed)

- 1 Extend SearchCommand + PageRequest + search_command facade (Phase 0 foundation): ddb-core/src/service/search.rs

## ddb-server/src/schema (listed)

- 2 Route GraphQL + CLI search through SearchCommand (Phase 1): ddb-server/src/schema/queries.rs
