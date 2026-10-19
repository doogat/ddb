# Split note: 00182-where-filter-correctness-completeness-v2.md
planned=7 prd_tasks=6 expansion=1.17 reasons=module_drift

## . (UNLISTED)

- 3 Ship the every+NULL rule (NULL fails every) and flip the pinned e2e test: CHANGELOG.md
- 7 Update server.md docs and CHANGELOG for null-skip and LIKE-escaping: CHANGELOG.md

## docs/src/technical (UNLISTED)

- 3 Ship the every+NULL rule (NULL fails every) and flip the pinned e2e test: docs/src/technical/server.md
- 7 Update server.md docs and CHANGELOG for null-skip and LIKE-escaping: docs/src/technical/server.md

## ddb-core/src/indexer (listed)

- 4 Add junction ref-id secondary index at both junction-table creation sites: ddb-core/src/indexer/materialize.rs

## ddb-core/src/sql_engine (listed)

- 4 Add junction ref-id secondary index at both junction-table creation sites: ddb-core/src/sql_engine/ddl.rs

## ddb-server/src (listed)

- 1 Null-skip explicit-null quantifier values in forward+reverse relation filters: ddb-server/src/relation_filter.rs
- 2 Escape LIKE wildcards in contains/startsWith operators: ddb-server/src/filter.rs
- 3 Ship the every+NULL rule (NULL fails every) and flip the pinned e2e test: ddb-server/src/relation_filter.rs
- 6 Correct the stale self-ref reverse-membership comment in filter.rs: ddb-server/src/filter.rs

## tests/e2e (listed)

- 3 Ship the every+NULL rule (NULL fails every) and flip the pinned e2e test: tests/e2e/typed_where_filter.rs
- 5 Add every-quantifier forward+reverse, vacuous+nonempty e2e scenarios: tests/e2e/typed_where_filter.rs
