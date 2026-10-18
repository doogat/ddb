# Split note: 00178-structured-fields-input-v1.md
planned=5 prd_tasks=5 expansion=1.00 reasons=module_drift

## . (UNLISTED)

- 5 E2E scenarios, docs, CHANGELOG, walkthrough, compat review for object-form fields: CHANGELOG.md

## docs/src/technical (UNLISTED)

- 5 E2E scenarios, docs, CHANGELOG, walkthrough, compat review for object-form fields: docs/src/technical/app-contract.md, docs/src/technical/rest-api.md, docs/src/technical/server.md, docs/src/technical/data-model.md, docs/src/technical/compatibility-checklist.md

## tests/e2e (UNLISTED)

- 5 E2E scenarios, docs, CHANGELOG, walkthrough, compat review for object-form fields: tests/e2e/main.rs

## tests/e2e/conformance (UNLISTED)

- 4 Conformance: create-with-typed-fields step (GraphQL leg; REST deferred): tests/e2e/conformance/workflows.rs

## ddb-core/src/app_contract (listed)

- 1 Core fields normalization (app_contract) + FFI routing: ddb-core/src/app_contract/fields_normalize.rs, ddb-core/src/app_contract/mod.rs

## ddb-core/src/ffi (listed)

- 1 Core fields normalization (app_contract) + FFI routing: ddb-core/src/ffi/driver.rs

## ddb-server/src (listed)

- 2 REST: accept object fields through boundary decoder + core normalization: ddb-server/src/rest.rs

## ddb-server/src/schema (listed)

- 3 GraphQL: fields to JSON scalar, decode through core normalization, update singleton mutations: ddb-server/src/schema/base_types.rs, ddb-server/src/schema/input.rs, ddb-server/src/schema/type_defs.rs, ddb-server/src/schema/mod.rs

## ddb-server/src/schema/mutations (listed)

- 3 GraphQL: fields to JSON scalar, decode through core normalization, update singleton mutations: ddb-server/src/schema/mutations/operations.rs, ddb-server/src/schema/mutations/singleton.rs
