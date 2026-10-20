# Split note: 00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md
planned=5 prd_tasks=4 expansion=1.25 reasons=module_drift

## . (UNLISTED)

- 5 Author index-evolution e2e/conformance scenarios, docs, and CHANGELOG entry: CHANGELOG.md

## docs/src/technical (UNLISTED)

- 5 Author index-evolution e2e/conformance scenarios, docs, and CHANGELOG entry: docs/src/technical/schema-apply.md, docs/src/technical/ffi.md

## tests/e2e (UNLISTED)

- 5 Author index-evolution e2e/conformance scenarios, docs, and CHANGELOG entry: tests/e2e/schema_apply.rs

## tests/e2e/conformance (UNLISTED)

- 5 Author index-evolution e2e/conformance scenarios, docs, and CHANGELOG entry: tests/e2e/conformance/workflows.rs

## tests/swift/Sources/DoogatDB (UNLISTED)

- 4 Add zone field to FFI ColumnDefRecord matching GraphQL null-when-undeclared semantics: tests/swift/Sources/DoogatDB/ddb_core.swift

## ddb-core/src/ffi (listed)

- 4 Add zone field to FFI ColumnDefRecord matching GraphQL null-when-undeclared semantics: ddb-core/src/ffi/records.rs

## ddb-core/src/schema_diff (listed)

- 1 Diff unique_together deltas on existing types into AddUniqueConstraint/DropUniqueConstraint PlanOps: ddb-core/src/schema_diff/plan.rs, ddb-core/src/schema_diff/differ.rs

## ddb-core/src/service (listed)

- 3 Integration-test index-evolution apply: migration, idempotency, and the duplicate-data error case: ddb-core/src/service/schema_apply.rs

## ddb-core/src/sql_engine (listed)

- 2 Execute unique-constraint PlanOps via custom-DDL handlers with a pre-commit duplicate check: ddb-core/src/sql_engine/ddl/unique_ops.rs, ddb-core/src/sql_engine/helpers/regexes.rs, ddb-core/src/sql_engine/ddl/mod.rs, ddb-core/src/sql_engine/mod.rs
