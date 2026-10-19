# Design: Split oversized server/core modules (round 2)

## Architecture fit

Pure structural refactor inside two existing layers, no new layer or crate boundary:

- `ddb-server/src/{filter.rs, schema/queries.rs, schema/base_types.rs}` — the GraphQL adapter layer (where-clause compilation, query-field registration, value-conversion helpers). Stays `ddb-server`-local; no adapter-neutral-type rule applies here (`types/` guardrail is a `ddb-core` rule).
- `ddb-core/src/{sql_engine/*, indexer/*, parser/mod.rs, crdt_resolver.rs, git_ops/mod.rs}` — the core library. Every split here must keep `ddb-core/src/types/**` adapter-neutral (unaffected — no type moves) and must not touch the public trait surface in `traits.rs` except the one explicit `sql_conn` disposition task.

This PRD repeats the exact pattern PRD 00156 established (`crud.rs`, `indexer/mod.rs`, `ffi.rs`, `mutations.rs`): turn one oversized file into a `name/` directory with a `mod.rs` that re-exports the prior public surface unchanged, and move code **verbatim** into cohesion-grouped siblings. Where the oversized file is a single `impl Type { ... }` block (all of `dml.rs`, `ddl.rs`, `indexer/materialize.rs::impl Index`, `indexer/graph.rs::impl Index`, `indexer/search.rs::impl Index`), the split additionally relies on Rust's multiple-`impl`-blocks-per-type-per-crate rule: each sibling file holds its own `impl Type { ... }` fragment for the same type, which is legal and is exactly how 00156 split `crud.rs`'s `impl DoogatService`.

## Module placement

All line counts below are **production-only** (re-measured at design time against HEAD `62659a6`, excluding `#[cfg(test)]`/`mod tests` modules — the metric this PRD and PRD 00156 both use). Counts drifted from the PRD body's 2026-09-05 figures (ongoing work since); re-measure again immediately before each task's split, since every other task in the batch can move these numbers.

| File (current) | Prod lines (now) | New directory |
|---|---|---|
| `ddb-server/src/filter.rs` | ~1162 (+ ~2234 test lines, excluded) | `ddb-server/src/filter/` |
| `ddb-server/src/schema/queries.rs` | 909 (one fn: `build_query_fields`, no colocated tests) | stays `queries.rs`, decomposed in place (see below) |
| `ddb-core/src/sql_engine/helpers.rs` | ~1091 (+ tests from 1092) | `ddb-core/src/sql_engine/helpers/` |
| `ddb-core/src/sql_engine/dml.rs` | 1417 (no colocated tests) | `ddb-core/src/sql_engine/dml/` |
| `ddb-core/src/sql_engine/ddl.rs` | 1654 (no colocated tests) | `ddb-core/src/sql_engine/ddl/` |
| `ddb-core/src/indexer/materialize.rs` | ~1234 (+ tests from 1235) | `ddb-core/src/indexer/materialize/` |
| `ddb-server/src/schema/base_types.rs` | ~1150 (+ tests from 1151) | `ddb-server/src/schema/base_types/` |
| `ddb-core/src/indexer/graph.rs` | 1020 (no colocated tests) | `ddb-core/src/indexer/graph/` |
| `ddb-core/src/parser/mod.rs` | 990 (tests are an out-of-line `mod tests;` already — not in this file's count) | `ddb-core/src/parser/` (already a dir; split `mod.rs` itself) |
| `ddb-core/src/sql_engine/builders.rs` | 916 (no colocated tests) | `ddb-core/src/sql_engine/builders/` |
| `ddb-core/src/crdt_resolver.rs` | ~872 (+ tests from 873) | `ddb-core/src/crdt_resolver/` |
| `ddb-core/src/git_ops/mod.rs` | ~907 (out-of-line `mod tests;`/`mod remote_timeout_tests;` not in this file's count; `mod lock_hook_tests` at 917 IS inline — re-measure its true end at execution) | `ddb-core/src/git_ops/` (already a dir; split `mod.rs` itself) |
| `ddb-core/src/indexer/search.rs` | 812 (through line ~812; re-measure — PRD's own text says 812, right at the cap) | `ddb-core/src/indexer/search/` |
| `ddb-core/src/sync_manager/mod.rs` | 798 | **no split** — stays under 800; re-measure only, do not touch unless re-crossed |
| `ddb-core/src/traits.rs` | 601 | **no split** — only the `sql_conn` exception disposition task touches it |

Every new `mod.rs` re-exports the prior public surface via `pub use self::<child>::*;` (or named re-exports where a glob would collide) so external callers, FFI, and existing tests see no path change — the same re-export shape `indexer/mod.rs` and `ffi.rs` used in 00156.

## Interfaces & contracts

No signatures change. This section pins the **exact submodule grouping** so the planner can turn each into a task without re-deriving it; the implementor re-measures line counts immediately before splitting (modules shift under concurrent batch work) and may rebalance a group across sibling files if a resulting file would still exceed 800 lines, as long as the grouping-by-concern below is preserved and nothing crosses a group boundary without a stated reason recorded in the task's commit message.

### filter.rs → filter/
- `filter/mod.rs` — re-exports; keeps `pub struct WhereClause` and `pub(crate) type SchemaLookup` visible at `crate::filter::*` exactly as today.
- `filter/scalars.rs` — `string_filter`, `int_filter`, `float_filter`, `bool_filter`, `id_filter`, `tags_filter`, `filter_type_for_column`, `build_where_input`, `sort_order_enum`, `base_order_column`, `build_order_by_input`, `build_order_sql` (lines 16–318 today).
- `filter/aggregates.rs` — `build_connection_type`, `is_numeric`, `add_aggregate_fields`, `build_aggregate_group_type`, `build_aggregate_type`, `build_aggregate_sql`, `build_aggregate_sql_grouped`, `build_aggregate_selects`, `aggregate_row_to_value` (lines 321–595).
- `filter/where_build.rs` — `WhereClause` struct + impl, `try_build_where_sql`, `build_where_sql`, `build_conditions_into`, `route_field_condition`, `build_column_conditions`, `build_logical_combinator`, `build_operator_condition`, `build_in_condition`, `build_not_in_condition`, `gql_to_sql` (lines 597–671, 985–1162).
- `filter/junction.rs` — `build_tags_filter_condition` (lines 901–984) — the tag/junction/EXISTS handling the PRD body names explicitly.
- `filter/reverse.rs` — `resolve_reference_target`, `current_field_names`, `reverse_relations`, `pick_reverse_field_name`, `resolve_reverse_target` (lines 751–900) — the reverse-relation discovery the PRD body names explicitly.
- `filter/tests.rs` — the existing `mod tests { ... }` body moved verbatim (unconstrained by the 800-line cap: it is colocated tests, the metric this PRD and 00156 both exclude).

### queries.rs (decompose `build_query_fields`, no directory — file stays <=800 after decomposition)
`build_query_fields` becomes a <=50-line composer. Each independently-registered top-level field (already delimited by the file's own `// name(...)` comments) becomes its own named function, each taking and returning the `Object` builder:
- `register_doogat_field`, `register_doogats_field`, `register_search_field`, `register_normalize_search_query_field`, `register_type_defs_field`, `register_sql_field`, `register_schema_version_field`, `register_checkbox_items_field`, `register_open_actions_field`, `register_tags_field`, `register_tag_entries_field` — one per existing block, signature `fn register_x(query: Object) -> Object`.
- The per-type loop (today's lines 425–797, the actual ~370-line body the PRD's "~842-line function" claim refers to) becomes `fn register_dynamic_type_fields(query: Object, type_schemas: &[TableSchema], known_types: &mut HashMap<String,String>, dynamic_types: &mut Vec<Object>, dynamic_inputs: &mut Vec<InputObject>) -> Result<(Object, Object, Object, Object), String>` (returning `query` plus the three discovery-output types it threads through), itself composed of:
  - `register_typed_object_and_inputs(schema, type_schemas, known_types) -> (Object, InputObject, InputObject, Object, Object, Object)` — builds `typed_obj`/`where_input`/`order_by_input`/`connection_type`/`aggregate_type`/`aggregate_group_type` (today's lines 467–481).
  - `register_per_type_list_field(query, schema, type_name, table_name, schema_lookup, plural, where_type_name, order_by_type_name, connection_type_name) -> Object` (today's lines 499–627).
  - `register_per_type_aggregate_field(query, schema, type_name, table_name, schema_lookup, plural, where_type_name) -> Object` (today's lines 630–723).
  - `register_per_type_singleton_field_if_applicable(query, schema, type_name, table_name, emitted_query_fields) -> Result<Object, String>` (today's lines 725–796, the SINGLETON-collision-fallback logic).
- **Inline-justified exception** (PRD's "or is justified inline" clause): the `async move { ... }` resolver closures passed to `Field::new` stay as single closures — splitting a closure's body into a separately-named function would require threading every captured variable through an explicit argument list for no behavioral gain, and 00156 did not split closure bodies either. Each `register_*` function's *own* body (constructing the `Field`, attaching arguments/description, calling `.field()`) stays at or under 50 lines; only the resolver closure's line count is exempted, and each is already single-purpose (one GraphQL field, matching its existing `// name(...)` comment boundary).

### helpers.rs → sql_engine/helpers/
- `helpers/mod.rs` — re-exports; keeps the free-function helpers imported via `super::helpers::{...}` elsewhere in `sql_engine` (e.g. `dml.rs`'s `use super::helpers::{eval_values_nullable, ...}`) resolving unchanged.
- `helpers/template.rs` — `re_title_placeholder`, `TemplatePlaceholder`, `parse_title_template` (lines 91–354).
- `helpers/inline_zone.rs` — `re_create_table_guard`, `re_create_table_preamble`, `preceding_word`, `find_zone_in_text`, `strip_column_list`, `find_column_list_bounds`, `split_top_level_segments`, `column_name_and_end`, `parse_zone_value`, `parse_segment_zone` (lines 355–926) — the `strip_inline_zones` family the PRD body names explicitly.
- `helpers/regexes.rs` — any remaining cached `re_*`/general SQL-token helpers not already covering (e.g. `extract_eq_pair` and any sibling helper between the inline-zone block and `mod tests`, lines 927–1091); re-measure at execution — the exact line where "inline-zone helpers" end and "general regex/token helpers" begin may shift.
- `helpers/tests.rs` — the existing `mod tests { ... }` moved verbatim.

### dml.rs → sql_engine/dml/
Single `impl<'a> SqlEngine<'a> { ... }` block (lines 124–1262) plus leading/trailing free functions. Split into multiple `impl<'a> SqlEngine<'a>` fragments across sibling files (the exact pattern 00156 used for `crud.rs`'s `impl DoogatService`) — legal because Rust allows multiple inherent-impl blocks for the same type within one crate:
- `dml/mod.rs` — re-exports; keeps `RESERVED_COLUMNS`, `type_check_value`, `parse_varchar_length`, `is_strict_check_type`, `is_null_literal` (lines 1–123, shared by every fragment below).
- `dml/insert.rs` — `impl<'a> SqlEngine<'a>` fragment: `handle_insert` through `merge_insert_results` (lines 124–429).
- `dml/update.rs` — `impl<'a> SqlEngine<'a>` fragment: `handle_update` through `update_indexes_atomically` (lines 430–981).
- `dml/validate.rs` — `impl<'a> SqlEngine<'a>` fragment: `validate_row_against_schema` through `find_conflict_match` (lines 982–1153).
- `dml/materialize_row.rs` — `impl<'a> SqlEngine<'a>` fragment: `insert_materialized_row` through `append_core_column_sets` (lines 1154–1266), plus the trailing free functions `classify_materialized_insert_error`, `lookup_singleton_existing_id`, `is_singleton_lock_failure`, `parse_unique_failure_columns`, `build_insert_columns`, `append_update_set_clauses` (lines 1267–1417).

**00196 dependency note**: PRD 00196 ("route SQL DML through write pipeline") depends on this split landing first (per the PRD body's task ordering — sweep group A runs before B/C). 00196 is out of this PRD's scope; this split must preserve every symbol path `dml.rs` exposed today (via `dml/mod.rs` re-exports) so 00196 is unaffected by which sibling file a given method physically lives in.

### ddl.rs → sql_engine/ddl/
Same multiple-`impl` pattern as `dml.rs`:
- `ddl/mod.rs` — re-exports; keeps `create_table_with_default_seed_commit_msg`, `apply_path_pair_rewrites`, `TypeKind` + its `impl` (lines 1–94, shared).
- `ddl/create.rs` — fragment: `handle_create_table`, `extract_columns`, `create_materialized_table` (lines 95–437).
- `ddl/alter.rs` — fragment: `validate_next_default`, `handle_alter_table`, `handle_alter_column_type`, `preflight_narrow_string`, `preflight_numeric`, `update_typedef`, `rematerialize_or_defer` (lines 438–824).
- `ddl/zone_ops.rs` — fragment: `handle_set_zone`, `handle_title_template`, `handle_search_key`, `handle_set_singleton`, `handle_drop_singleton`, `validate_title_template` (lines 825–1049).
- `ddl/rename.rs` — fragment: `handle_rename_table`, `build_rename_plan`, `plan_typedef_rewrite`, `plan_data_doogats_move`, `plan_references_rewrite`, `plan_backlinks_rewrite`, `handle_rename_column`, `buffer_rename_writes`, `reindex_rename_from_buffer`, `rewrite_data_doogats_for_rename`, `reindex_after_rename` (lines 1050–1399).
- `ddl/drop.rs` — fragment: `handle_drop`, `handle_drop_table`, `find_type_data_doogats`, `cascade_delete_doogats`, `soft_drop_doogats`, `cleanup_materialized_tables` (lines 1400–1548).
- `ddl/mod.rs` also keeps (or add `ddl/load.rs` if this pushes `mod.rs` over budget) `load_typedef_location`, `load_schema`, `read_typedef_content`, `extract_unique_constraints`, `validate_next_partition_refs` (lines 1549–1654).

### indexer/materialize.rs → indexer/materialize/
`impl Index { ... }` (lines 101–927) plus leading/trailing free functions:
- `materialize/mod.rs` — re-exports; keeps `is_core_column`, `normalize_bool_str`, `build_column_definitions`, `junction_table_name`, `junction_parent_id_column`, `junction_ref_id_column`, `junction_table_ddl` (lines 1–100, shared).
- Split the 826-line `impl Index` fragment into 2 sibling files by its internal grouping (re-measure the exact midpoint at execution — this impl's method names were not individually inventoried for this design since the file is already at the boundary; the implementor groups by the natural DDL-vs-row-materialization split visible in the method bodies).
- `materialize/schema_helpers.rs` — trailing free functions `check_cross_zone_duplicates` through `widen_types` (lines 928–1234).
- `materialize/tests.rs` — the existing `mod tests { ... }` moved verbatim.

### base_types.rs → schema/base_types/ (ddb-server)
All free functions (no `impl` blocks), grouped by what they convert:
- `base_types/mod.rs` — re-exports.
- `base_types/to_gql.rs` — the ParsedDoogat/row → `GqlValue` direction: `decode_json_fields_object`, `gql_to_value_strict`, `zone_str`, `build_tags_list`, `build_inline_fields_list`, `build_links_list`, `build_attachments_list`, `doogat_to_value`, `checkbox_row_to_value`, `search_hit_to_value`, `tag_info_to_value`, `tag_entry_to_value`, `sql_result_to_value`, `column_def_to_value`, `typedef_to_value`, `typed_doogat_to_value`, `extract_typed_field` (lines 23–509).
- `base_types/schema_object.rs` — dynamic-schema `Object`/`Field` construction: `is_valid_graphql_name`, `simple_field`, `gql_to_field_value`, `obj_field`, `add_typed_doogat_accessors`, `doogat_object`, `ref_target_gql_type`, `extract_sort_key`, `extract_singular_id`, `build_singular_ref_field`, `extract_ref_ids`, `sort_and_limit_values`, `build_plural_ref_field`, `build_raw_ref_field`, `add_reference_fields`, `build_typed_object`, `column_to_gql_type` (lines 510–929).
- `base_types/naming.rs` — name-sanitizing helpers: `capitalize`, `strip_id_suffix`, `pluralize`, `pluralize_preserving_case`, `sanitize_type_name`, `singleton_field_base`, `sanitize_field_name`, `resolve_column` (lines 939–1046).
- `base_types/validation.rs` — request-arg validation: `event_stream`, `check_limit`, `check_offset`, `check_format`, `validate_limit`, `validate_offset`, `validate_format` (lines 1047–1150).
- `base_types/tests.rs` — the existing `mod tests { ... }` moved verbatim.

### indexer/graph.rs → indexer/graph/
`impl Index { ... }` (lines 21–920) plus trailing free functions:
- `graph/mod.rs` — re-exports.
- Split the 900-line `impl Index` fragment into 2 sibling files by its internal method grouping, re-measured at execution (not individually inventoried here — same note as `materialize/`).
- `graph/staleness.rs` — trailing free functions `parse_date_to_naive`, `resolve_last_date`, `compute_staleness`, `count_links`, `normalize_suggestion_scores` (lines 921–1020).

### parser/mod.rs → split in place (already a directory)
`parser/mod.rs` is itself a flat file inside the existing `parser/` directory (sibling to its own out-of-line `mod tests;`). Split by concern, re-exported from `parser/mod.rs`:
- `parser/yaml_bridge.rs` — `fence_regex`, `inline_code_regex`, `From<serde_yaml::Error>`, `RawDoogatMeta` + its `From` impl, `from_serde_yaml`, `to_serde_yaml`, `yaml_quote`, `serialize_yaml_value` (lines 10–94, 642–679).
- `parser/zones.rs` — `strip_wikilink`, `is_reference_line`, `lazy_static_regex`, `split_zones`, `find_frontmatter`, `has_frontmatter`, `find_separators_after`, `parse_frontmatter` (lines 95–265).
- `parser/links.rs` — `extract_inline_fields`, `extract_embeds`, `extract_markdown_links`, `extract_bare_urls`, `extract_links`, `extract_wikilinks_from`, `extract_wikilinks`, `rewrite_wikilinks`, `rewrite_links` (lines 266–641).
- `parser/mod.rs` keeps (or spills to `parser/doogat.rs` if over budget) `serialize`, `rewrite_id_field`, `extract_checkboxes`, `extract_sections`, `extract_hashtags`, `parse`, `generate_id`, `generate_unique_id`, `extract_id_from_path` (lines 680–990) plus the re-exports and `mod tests;` declaration.

### sql_engine/builders.rs → sql_engine/builders/
All free functions, grouped by direction (schema→doogat vs doogat→schema vs YAML column parsing):
- `builders/mod.rs` — re-exports.
- `builders/typedef.rs` — `build_column_yaml`, `build_columns_yaml`, `build_typedef_extra_fields`, `build_typedef_doogat` (lines 13–139).
- `builders/data_doogat.rs` — `ColumnZoneOutput`, `process_reference_column`, `process_column_zones`, `resolve_insert_title`, `render_title_template`, `resolve_reference_field`, `recompute_template_title`, `derive_date`, `join_sections`, `build_data_doogat` (lines 140–473).
- `builders/schema_parse.rs` — `OptionalColumnFields`, `parse_optional_column_fields`, `parse_single_column`, `parse_column_definitions`, `parse_unique_together`, `extract_optional_schema_fields`, `OptionalSchemaFields`, `schema_from_parsed` (lines 474–759).
- `builders/update_apply.rs` — `apply_updates_to_doogat`, `update_body_section`, `update_reference_line`, `rename_key_in_doogat` (lines 760–916).

### crdt_resolver.rs → crdt_resolver/
All free functions plus one trait impl, grouped by what they resolve:
- `crdt_resolver/mod.rs` — re-exports; keeps `DefaultResolver` and its `impl ConflictResolver` (lines 860–872) since it dispatches across every sibling.
- `crdt_resolver/frontmatter.rs` — `derive_actor`, `resolve_conflicts`, `parse_zones`, `FmValue`, `merge_frontmatter`, `partition_fm`, `merge_list_fields`, `yaml_to_map`, `map_to_yaml`, `apply_scalar_diff` (lines 11–364) — this is `merge_frontmatter`, called from both `resolve_conflicts` and `resolve_append_log` per the Gotchas note; both call sites move with their callers, the shared function stays here as the single definition.
- `crdt_resolver/body.rs` — `merge_body`, `apply_text_diff`, `merge_reference`, `ref_lines`, `refs_list_id`, `apply_list_diff` (lines 365–560).
- `crdt_resolver/lww.rs` — `resolve_lww`, `lww_pick`, `pick_lww_winner` (lines 561–626).
- `crdt_resolver/append_log.rs` — `resolve_append_log`, `split_body_sections`, `is_log_section`, `parse_log_entries`, `entry_dedup_key`, `merge_log_section`, `extract_entry_date`, `merge_body_append_log` (lines 627–859) — calls `merge_frontmatter` from `frontmatter.rs`.
- `crdt_resolver/tests.rs` — the existing `mod tests { ... }` moved verbatim.

### git_ops/mod.rs → split in place (already a directory)
`git_ops/mod.rs` already declares `mod hlc_clock; mod merge; mod read; mod remote; mod rename;` as siblings — this split moves the remaining monolithic content (the `GitRepo` struct, its primary `impl GitRepo` with ~550 lines of methods, and the trait-impl forwarders) the same way those five were already split out:
- `git_ops/mod.rs` keeps `doogat_path`, `From<git2::Error>`, `validate_path`, `GitRepo` struct, `DepthGuard` + its `Drop` impl, and the module declarations (lines 1–126, ~910–917).
- `git_ops/repo_core.rs` — the primary `impl GitRepo { ... }` (lines 127–684, ~550 lines of the repo's own methods — not the trait forwarders).
- `git_ops/trait_impls.rs` — every `impl crate::traits::Git*For GitRepo` forwarder (lines 685–907): `DoogatSource`, `DoogatStore`, `GitRemote`, `GitMerge`, `GitHistory`, `GitBinary`, `GitRename`, `GitDesktopHooks`, `GitBackend`.
- `mod lock_hook_tests { ... }` (line 917 onward) is an inline test module in the current file (unlike the out-of-line `mod tests;`/`mod remote_timeout_tests;` declarations beside it) — move it verbatim into `git_ops/lock_hook_tests.rs` with a `mod lock_hook_tests;` declaration in `mod.rs`, excluded from the production-line count like every other colocated test module.

### indexer/search.rs → indexer/search/
`impl Index { ... }` (lines 25–792) plus leading struct and trailing free functions. At 812 lines total this file is right at the cap — re-measure before deciding whether the split is still needed; if it has dropped to <=800 by execution time, skip this task's module-directory work (note the re-measurement in the task's commit message) and treat it like `sync_manager/mod.rs` below.
- `search/mod.rs` — re-exports; keeps `CompiledSearchInputs` (lines 19–24, shared input to the `impl Index` fragment).
- Split the `impl Index` fragment (lines 25–792) into 2 sibling files by its internal method grouping if still needed, re-measured at execution (same note as `materialize/` and `graph/`).
- `search/helpers.rs` — trailing free functions `strip_leading_and`, `apply_fields_to_hits` (lines 793–812).

### sync_manager/mod.rs — no split
798 prod lines at HEAD `62659a6`, still under 800. Re-measure immediately before this task starts (other tasks in this batch touch nothing in `sync_manager`, so it should not move, but the batch's own earlier splits are the reason this re-check exists at all); if still <=800, do nothing and record the re-measurement in the task's commit message. If it has crossed 800 again, split it the same way as the other `impl`-block files above, grouping by concern from its own structure at that time.

### traits.rs — `sql_conn` exception disposition only
No split. This file stays untouched except the one disposition task (inventory `SqlBackend::sql_conn` callers post-split, decide remove-vs-retain, record the outcome in `traits.rs`'s doc comment at lines 68–77 and in `docs/src/architecture/design-decisions.md`). The round-2 splits above may change *which file* calls `sql_conn` (e.g. a `dml/` or `ddl/` sibling instead of the monolithic file), so this task must re-run its caller inventory **after** the sql_engine splits land, not before.

## Data flow

No data flow changes anywhere in this PRD — every split is a verbatim move of existing code into a new file location, with the identical call graph. The only flow-relevant fact is ordering: sweep group A (dml/ddl/materialize) must land before group B and C per the PRD body (00196 depends on `dml/`'s split shape), and the `sql_conn` disposition task must run after the sql_engine splits (dml/ddl/helpers/builders) so its caller inventory reflects the final file layout.

## Reuse inventory

This PRD introduces no new capability and needs no new helper - it is exclusively "move existing code to a new path, re-export the old path." The only reusable pattern is the *shape* of the previous split, already in the tree:
- `ddb-core/src/indexer/mod.rs`, `ddb-core/src/ffi.rs` area (PRD 00156) - the `name/mod.rs` re-export directory pattern this PRD repeats for every oversized file here.
- `ddb-core/src/git_ops/mod.rs`'s existing `mod hlc_clock; mod merge; mod read; mod remote; mod rename;` - proof the same crate already splits one struct's methods (`GitRepo`) across sibling files via multiple `impl` blocks; `git_ops/mod.rs`'s own split in this PRD extends that existing pattern rather than inventing a new one.
- `ddb-core/src/sql_engine/dml.rs`'s single `impl<'a> SqlEngine<'a>` block is itself proof that the crate already tolerates one type's impl spread across whatever number of files is convenient - the round-2 sweep's `dml/`, `ddl/`, `materialize/`, `graph/`, `search/` splits are the same technique applied five more times.

No grep sweep for a generic "split a module" helper was run - there is no such helper to find; a module split is a file-system/compiler-boundary operation with no library surface to reuse.

## Alternatives considered

1. **Chosen: `name/` directory + `mod.rs` re-export, grouped by concern** (detailed above). Preserves every public/`pub(crate)` path exactly (an existing caller's `use` statement is unaffected), matches the established 00156/`git_ops` precedent, and groups cohesively enough that a future reader opening `filter/junction.rs` finds only junction logic.
2. **Smallest-diff alternative: flat numbered split** (`dml_1.rs`, `dml_2.rs`, ... each holding a contiguous chunk of lines regardless of concern, re-exported from the original `dml.rs` which keeps its name). Strictly less work per file (no need to read the file for concern boundaries, just slice by line count) and technically satisfies the <=800-line rubric. **Rejected**: the PRD's own Functional Decomposition explicitly names concern-based splits for `filter.rs` (junction/reverse) and `helpers.rs` (regex/inline-zone family), and a numbered split would make every future read of `dml_2.rs` require re-discovering "insert, or update, or validate?" - trading today's small time saving for a permanent comprehension tax, which is exactly the problem this PRD exists to remove.
3. **Defer the round-2 sweep's 10 extra modules, ship only the original 3** (`filter.rs`, `queries.rs`, `helpers.rs`). Smaller PRD, faster to land. **Rejected**: this is already the decision the PRD's own Provenance note describes being made and reversed twice (00158-00161 deferrals, then the 2026-07-16 assessment extending the list) - re-deferring a third time repeats the exact "oversized-file debt compounds" problem this PRD's Problem Statement names, and the PRD body has already committed to the 3-group task structure.

## Risks & edge cases

- **Re-export glob collisions**: two sibling files both defining a `tests` module name, or a free function name colliding with another once both are `pub use self::*`'d into `mod.rs`. Mitigation: `cargo build` after every single-file split (not batched) surfaces this immediately as a compile error, and 00156 hit zero such collisions across 4 splits, so the base rate is low. Where a glob would collide (e.g. two `ColumnZoneOutput`-shaped private structs), re-export by explicit name or keep the struct `pub(self)` + referenced only within its own file.
- **Split boundary drift during the batch**: round-2 sweep tasks for `dml.rs`/`ddl.rs` land in the same batch as other PRDs (00196 depends on `dml/`'s shape; other backlog PRDs may edit `base_types.rs` or `builders.rs` concurrently if the batch runs multiple lanes). Mitigation: each split task re-measures line counts and re-reads the target file **immediately before splitting**, not from this design doc's line numbers, which are a point-in-time snapshot at `62659a6`.
- **Likely next changes after this PRD** (and what in this design boxes them in): (1) PRD 00196 (route SQL DML through write pipeline) edits `dml/insert.rs`/`dml/update.rs` next - the concern-based split makes its diff land in exactly the files it needs, not a numbered chunk; (2) a future zone/schema PRD likely touches `ddl/zone_ops.rs` and `materialize/schema_helpers.rs` together - keeping zone logic in its own named file in both places (rather than interleaved with rename/drop logic) keeps that future PRD's diff small; (3) the `sql_conn` exception, if retained rather than removed, becomes a recurring audit item for the next evolution assessment - this design's requirement to re-run the caller inventory *after* the splits land is what keeps that audit accurate rather than stale.
- **Impl-block split changing method resolution order for overlapping trait bounds**: none of the split types (`SqlEngine<'a>`, `Index`, `GitRepo`) have ambiguous inherent-vs-trait method names today: a `cargo build` after each split would fail loudly (E0034 multiple applicable items) if this ever became a risk, so it is self-detecting, not silent.
- **Test-module line counts are excluded from the 800-line metric but are not exempt from file-health norms**: `filter/tests.rs` (≈2234 lines) and `git_ops`'s `lock_hook_tests.rs` stay oversized test files by design (metric exclusion, per the PRD's own "even excluding its colocated test module" framing) - this PRD does not split them further, and no future PRD should read their size as unaddressed debt under *this* rubric.

## Test strategy outline

- **Per split, before committing**: `cargo build` (workspace) to catch any re-export/visibility break immediately; `cargo clippy --workspace --all-targets` to catch any new lint (the PRD's Success Metric is zero *new* lints - `helpers.rs`'s 3 historical lints are already cleared per the PRD body, this split must not reintroduce anything of that shape); `cargo test-ci` (fast tier) scoped to the touched crate at minimum, full `cargo test-ci` before the task's final commit.
- **Behavior-preservation check (Phase 1 of the PRD's own plan)**: for each split, `git diff <pre-split-commit> <post-split-commit> -- <old-path> <new-paths>` and confirm the union of added lines across the new files is byte-identical to the removed lines from the old file, modulo whitespace/import reordering - the same check 00156 used. Any hunk that is not a pure move is a scope violation for this PRD.
- **`queries.rs` decomposition specifically**: the existing GraphQL schema-snapshot/integration tests (already present per the PRD's Critical Scenarios) must stay green with no assertion changes beyond import paths - this is the proof that extracting `build_query_fields` into named helpers did not reorder field registration (Risk already named in the PRD body).
- **Full-suite gate (Phase 2 of the PRD's own plan)**: `cargo test-ci` + `cargo test -p ddb-e2e` + `cargo clippy --workspace --all-targets` after all ten round-2 splits plus the three original splits plus the `sql_conn` disposition, before the PRD's final commit.
- **`sql_conn` disposition task**: no new test unless removal actually changes a caller (then that caller's existing test already covers the behavior unchanged); if retained, the test strategy is "no test" - the deliverable is the written disposition in `traits.rs` and `design-decisions.md`, not runtime behavior.

## Review log

- non-blocker: `filter/where_build.rs`'s cited line ranges (597–671, 985–1162) don't cover `route_field_condition` (actually at line 720, inside the gap that belongs to `filter/reverse.rs`). No doc change needed — the doc already instructs re-measuring before splitting; flagged only as the specific range proven stale by spot-check.
- question: the PRD body's Phase 1 "behavior check" task is one undifferentiated step depending on all 6 Phase 0 tasks, while this design's own Test Strategy specifies a per-split check. Not a blocker; worth tightening when plan-tasks grains this out.
dispatch 1 (claude): cardinal-sin 0, blocker 0, non-blocker 1, question 1
- non-blocker: `queries.rs`'s inner sub-helper line numbers (`register_typed_object_and_inputs` 467–481, `register_per_type_list_field` 499–627) drift ~9-11 lines from the real comment-marker boundaries; the overall per-type-loop span and named-function grouping are correct. No doc change needed — same re-measure-before-splitting mitigation already stated.
- question: `graph.rs`/`search.rs`'s 2-way `impl Index` split has no inventoried internal boundary (acknowledged already in the design); acceptable given the re-measure-at-execution policy.
dispatch 2 (claude-fallback): cardinal-sin 0, blocker 0, non-blocker 1, question 1
