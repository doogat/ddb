# Split note: 00183-split-oversized-modules-round-2-v1.md
planned=10 prd_tasks=10 expansion=1.00 reasons=module_drift

## ddb-core/src (UNLISTED)

- 6 Sweep group C: split sql_engine/builders.rs, crdt_resolver.rs, git_ops/mod.rs, indexer/search.rs; re-measure sync_manager/mod.rs: ddb-core/src/crdt_resolver.rs
- 9 Inventory SqlBackend::sql_conn callers and decide the transitional-exception disposition: ddb-core/src/traits.rs

## ddb-core/src/crdt_resolver (UNLISTED)

- 6 Sweep group C: split sql_engine/builders.rs, crdt_resolver.rs, git_ops/mod.rs, indexer/search.rs; re-measure sync_manager/mod.rs: ddb-core/src/crdt_resolver/mod.rs, ddb-core/src/crdt_resolver/frontmatter.rs, ddb-core/src/crdt_resolver/body.rs, ddb-core/src/crdt_resolver/lww.rs, ddb-core/src/crdt_resolver/append_log.rs, ddb-core/src/crdt_resolver/tests.rs

## ddb-core/src/git_ops (UNLISTED)

- 6 Sweep group C: split sql_engine/builders.rs, crdt_resolver.rs, git_ops/mod.rs, indexer/search.rs; re-measure sync_manager/mod.rs: ddb-core/src/git_ops/mod.rs, ddb-core/src/git_ops/repo_core.rs, ddb-core/src/git_ops/trait_impls.rs, ddb-core/src/git_ops/lock_hook_tests.rs

## ddb-core/src/indexer (UNLISTED)

- 4 Sweep group A: split sql_engine/dml.rs, sql_engine/ddl.rs, indexer/materialize.rs: ddb-core/src/indexer/materialize.rs
- 5 Sweep group B: split schema/base_types.rs, indexer/graph.rs, parser/mod.rs: ddb-core/src/indexer/graph.rs
- 6 Sweep group C: split sql_engine/builders.rs, crdt_resolver.rs, git_ops/mod.rs, indexer/search.rs; re-measure sync_manager/mod.rs: ddb-core/src/indexer/search.rs

## ddb-core/src/indexer/graph (UNLISTED)

- 5 Sweep group B: split schema/base_types.rs, indexer/graph.rs, parser/mod.rs: ddb-core/src/indexer/graph/mod.rs, ddb-core/src/indexer/graph/staleness.rs

## ddb-core/src/indexer/materialize (UNLISTED)

- 4 Sweep group A: split sql_engine/dml.rs, sql_engine/ddl.rs, indexer/materialize.rs: ddb-core/src/indexer/materialize/mod.rs, ddb-core/src/indexer/materialize/schema_helpers.rs, ddb-core/src/indexer/materialize/tests.rs

## ddb-core/src/indexer/search (UNLISTED)

- 6 Sweep group C: split sql_engine/builders.rs, crdt_resolver.rs, git_ops/mod.rs, indexer/search.rs; re-measure sync_manager/mod.rs: ddb-core/src/indexer/search/mod.rs, ddb-core/src/indexer/search/helpers.rs

## ddb-core/src/parser (UNLISTED)

- 5 Sweep group B: split schema/base_types.rs, indexer/graph.rs, parser/mod.rs: ddb-core/src/parser/mod.rs, ddb-core/src/parser/yaml_bridge.rs, ddb-core/src/parser/zones.rs, ddb-core/src/parser/links.rs

## ddb-core/src/sql_engine (UNLISTED)

- 2 Split helpers.rs into sql_engine/helpers/ submodules: ddb-core/src/sql_engine/helpers.rs
- 4 Sweep group A: split sql_engine/dml.rs, sql_engine/ddl.rs, indexer/materialize.rs: ddb-core/src/sql_engine/dml.rs, ddb-core/src/sql_engine/ddl.rs
- 6 Sweep group C: split sql_engine/builders.rs, crdt_resolver.rs, git_ops/mod.rs, indexer/search.rs; re-measure sync_manager/mod.rs: ddb-core/src/sql_engine/builders.rs

## ddb-core/src/sql_engine/builders (UNLISTED)

- 6 Sweep group C: split sql_engine/builders.rs, crdt_resolver.rs, git_ops/mod.rs, indexer/search.rs; re-measure sync_manager/mod.rs: ddb-core/src/sql_engine/builders/mod.rs, ddb-core/src/sql_engine/builders/typedef.rs, ddb-core/src/sql_engine/builders/data_doogat.rs, ddb-core/src/sql_engine/builders/schema_parse.rs, ddb-core/src/sql_engine/builders/update_apply.rs

## ddb-core/src/sql_engine/ddl (UNLISTED)

- 4 Sweep group A: split sql_engine/dml.rs, sql_engine/ddl.rs, indexer/materialize.rs: ddb-core/src/sql_engine/ddl/mod.rs, ddb-core/src/sql_engine/ddl/create.rs, ddb-core/src/sql_engine/ddl/alter.rs, ddb-core/src/sql_engine/ddl/zone_ops.rs, ddb-core/src/sql_engine/ddl/rename.rs, ddb-core/src/sql_engine/ddl/drop.rs

## ddb-core/src/sql_engine/dml (UNLISTED)

- 4 Sweep group A: split sql_engine/dml.rs, sql_engine/ddl.rs, indexer/materialize.rs: ddb-core/src/sql_engine/dml/mod.rs, ddb-core/src/sql_engine/dml/insert.rs, ddb-core/src/sql_engine/dml/update.rs, ddb-core/src/sql_engine/dml/validate.rs, ddb-core/src/sql_engine/dml/materialize_row.rs

## ddb-core/src/sync_manager (UNLISTED)

- 6 Sweep group C: split sql_engine/builders.rs, crdt_resolver.rs, git_ops/mod.rs, indexer/search.rs; re-measure sync_manager/mod.rs: ddb-core/src/sync_manager/mod.rs

## ddb-server/src (UNLISTED)

- 1 Split filter.rs into filter/ submodules: ddb-server/src/filter.rs

## docs/src/architecture (UNLISTED)

- 9 Inventory SqlBackend::sql_conn callers and decide the transitional-exception disposition: docs/src/architecture/design-decisions.md
- 10 Update module docs for the new split module layout: docs/src/architecture/modules.md

## docs/src/technical (UNLISTED)

- 10 Update module docs for the new split module layout: docs/src/technical/walkthrough.md

## ddb-core/src/sql_engine/helpers (listed)

- 2 Split helpers.rs into sql_engine/helpers/ submodules: ddb-core/src/sql_engine/helpers/mod.rs, ddb-core/src/sql_engine/helpers/template.rs, ddb-core/src/sql_engine/helpers/inline_zone.rs, ddb-core/src/sql_engine/helpers/regexes.rs, ddb-core/src/sql_engine/helpers/tests.rs

## ddb-server/src/filter (listed)

- 1 Split filter.rs into filter/ submodules: ddb-server/src/filter/mod.rs, ddb-server/src/filter/scalars.rs, ddb-server/src/filter/aggregates.rs, ddb-server/src/filter/where_build.rs, ddb-server/src/filter/junction.rs, ddb-server/src/filter/reverse.rs, ddb-server/src/filter/tests.rs

## ddb-server/src/schema (listed)

- 3 Decompose build_query_fields into named helpers: ddb-server/src/schema/queries.rs
- 5 Sweep group B: split schema/base_types.rs, indexer/graph.rs, parser/mod.rs: ddb-server/src/schema/base_types.rs, ddb-server/src/schema/base_types/mod.rs, ddb-server/src/schema/base_types/to_gql.rs, ddb-server/src/schema/base_types/schema_object.rs, ddb-server/src/schema/base_types/naming.rs, ddb-server/src/schema/base_types/validation.rs, ddb-server/src/schema/base_types/tests.rs

## (no files declared)

- 7 Per-split behavior check: diff moved bodies, run fast tests
- 8 Full Tier-1 gate + e2e (deletion/replace safety net)
