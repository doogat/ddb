# Split note: 00179-sql-identifier-escaping-materializer-v1.md
planned=9 prd_tasks=10 expansion=0.90 reasons=module_drift

## . (UNLISTED)

- 9 Update docs + CHANGELOG for the escaping and write-boundary validation; push master: CHANGELOG.md

## ddb-core/src (UNLISTED)

- 6 Add write-boundary charset guard: reject unsafe type names / field keys on create and batch: ddb-core/src/error.rs

## docs/src/technical (UNLISTED)

- 9 Update docs + CHANGELOG for the escaping and write-boundary validation; push master: docs/src/technical/indexer.md, docs/src/technical/errors.md

## tests/e2e (UNLISTED)

- 8 Author regression coverage: repo-wide bare-identifier pattern check + e2e create-with-odd-type scenario: tests/e2e/main.rs

## ddb-core/src/indexer (listed)

- 2 Consolidate escape_sql_ident and its 20 callers onto quote_ident; cross-reference the three charset validators: ddb-core/src/indexer/filter.rs, ddb-core/src/indexer/mod.rs, ddb-core/src/indexer/search.rs, ddb-core/src/indexer/cascade.rs
- 3 Escape every identifier interpolation in materialize.rs: ddb-core/src/indexer/materialize.rs

## ddb-core/src/schema_diff (listed)

- 1 Promote quote_ident into sql_engine::helpers; plan.rs reuses it: ddb-core/src/schema_diff/plan.rs
- 2 Consolidate escape_sql_ident and its 20 callers onto quote_ident; cross-reference the three charset validators: ddb-core/src/schema_diff/desired.rs

## ddb-core/src/service (listed)

- 2 Consolidate escape_sql_ident and its 20 callers onto quote_ident; cross-reference the three charset validators: ddb-core/src/service/delete.rs
- 6 Add write-boundary charset guard: reject unsafe type names / field keys on create and batch: ddb-core/src/service/create.rs, ddb-core/src/service/batch.rs

## ddb-core/src/sql_engine (listed)

- 1 Promote quote_ident into sql_engine::helpers; plan.rs reuses it: ddb-core/src/sql_engine/helpers.rs, ddb-core/src/sql_engine/mod.rs
- 2 Consolidate escape_sql_ident and its 20 callers onto quote_ident; cross-reference the three charset validators: ddb-core/src/sql_engine/helpers.rs, ddb-core/src/sql_engine/delete.rs
- 5 Sweep remaining bare identifier interpolations through quote_ident: dml.rs and relation_filter.rs: ddb-core/src/sql_engine/dml.rs
- 6 Add write-boundary charset guard: reject unsafe type names / field keys on create and batch: ddb-core/src/sql_engine/helpers.rs, ddb-core/src/sql_engine/mod.rs
- 7 Add charset guard at the sync-load boundary (schema_from_parsed): ddb-core/src/sql_engine/builders.rs
- 8 Author regression coverage: repo-wide bare-identifier pattern check + e2e create-with-odd-type scenario: ddb-core/src/sql_engine/helpers.rs

## ddb-server/src (listed)

- 4 Escape where-compiler column identifiers in ddb-server/src/filter.rs: ddb-server/src/filter.rs
- 5 Sweep remaining bare identifier interpolations through quote_ident: dml.rs and relation_filter.rs: ddb-server/src/relation_filter.rs
