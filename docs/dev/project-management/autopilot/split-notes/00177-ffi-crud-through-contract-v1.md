# Split note: 00177-ffi-crud-through-contract-v1.md
planned=5 prd_tasks=5 expansion=1.00 reasons=module_drift

## . (UNLISTED)

- 5 Update FFI docs, app-contract docs, conformance-harness docs, and CHANGELOG for structured FFI CRUD: CHANGELOG.md

## ddb-core/src/service (UNLISTED)

- 2 Implement create_structured/update_structured and remove raw FFI CRUD exports: ddb-core/src/service/create.rs, ddb-core/src/service/update.rs

## docs/src/technical (UNLISTED)

- 5 Update FFI docs, app-contract docs, conformance-harness docs, and CHANGELOG for structured FFI CRUD: docs/src/technical/app-contract.md, docs/src/technical/ffi.md, docs/src/technical/conformance-harness.md

## ddb-core/src/app_contract (listed)

- 1 Add FFI structured input/result records and name BaseOnly as the FFI policy default: ddb-core/src/app_contract/commands.rs

## ddb-core/src/ffi (listed)

- 1 Add FFI structured input/result records and name BaseOnly as the FFI policy default: ddb-core/src/ffi/records.rs, ddb-core/src/ffi/tests.rs
- 2 Implement create_structured/update_structured and remove raw FFI CRUD exports: ddb-core/src/ffi/driver.rs, ddb-core/src/ffi/tests.rs
- 3 Replace the lossy FFI error allowlist with the policy-derived ffi_variant: ddb-core/src/ffi/records.rs, ddb-core/src/ffi/tests.rs
- 4 Regenerate FFI bindings and add warnings/policy parity tests: ddb-core/src/ffi/tests.rs
