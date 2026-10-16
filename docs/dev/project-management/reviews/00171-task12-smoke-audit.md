# Task 12 smoke assertion audit

Audited every source body in tests/smoke.sh:1-769 and tests/smoke.ps1:1-932, all 56 numbered sections in each dialect (112 rows), the smoke portion of the migration map, the actual mapped Rust test bodies and relevant fixture helpers, and main.rs registration. The inventory below preserves one row per source section per dialect. No row was discarded based on a DONE label or a paired-dialect claim.

All referenced target functions were inspected with their #[test] attribute; all 29 target modules are unconditionally registered in tests/e2e/main.rs. The audit initially found the discrepancies below; the authorized continuation repaired assertions inside existing tests. No smoke test function was added, and common.rs, production code, the source scripts, and the migration map were not edited. Mapping-only corrections remain for the parent to record.

## Findings and corrections

- **M01 — Mapping correction; §1.** The PowerShell init header is tests/smoke.ps1:66, not :67. The registered smoke_core::smoke_01_init target already covers the successful command. Source row keys: `tests/smoke.sh:26`, `tests/smoke.ps1:66`.
- **M02 — Mapping correction; §4.** Add sync_crdt::non_overlapping_edits_merge_cleanly alongside cli_update_app_error::cli_update_existing_prints_updated_and_changes_title. The former already runs --title and --tags together, asserts success, and reads back the updated title/tags after sync. No new compound-update test was needed. Source row keys: `tests/smoke.sh:47`, `tests/smoke.ps1:92`.
- **M03 — Mapping correction; §24.** Use set_flag::create_with_multiple_set_flags for the untyped two-field create, replacing create_with_set_on_typed_doogat as that sub-check's target. The original typed regression remains intact. Source row keys: `tests/smoke.sh:594`, `tests/smoke.ps1:715`.
- **S01 — Repaired; §7b.** tags::body_hashtag_indexed_with_source only checked tag names despite its name/comment. Added the source CLI body update, reindex, and a filtered hierarchical-tag row asserting gtd/act/next | body; original create/tag assertions remain. Source row keys: `tests/smoke.sh:77`, `tests/smoke.ps1:127`.
- **S02 — Repaired; §7c.** checkboxes::checkbox_filter_by_state only indexed created content. Retained that regression and added CLI body update/reindex with open/done/info items, then asserted the updated open item is returned and the done item is excluded. Source row keys: `tests/smoke.sh:83`, `tests/smoke.ps1:134`.
- **S03 — Repaired; §7d, 10b.** folder_namespace::insert_folder_typedef_creates_subdirectory_path and rename::rename_moves_file_and_rewrites_backlinks used exists(), weaker than Bash -f. Their destination checks now require is_file(), preserving the stronger original rename backlink checks. Source row keys: `tests/smoke.sh:89`, `tests/smoke.sh:128`, `tests/smoke.ps1:141`, `tests/smoke.ps1:184`.
- **S04 — Repaired; §8.** search_links::full_text_search checked a body-only term and title text, while the source searches title text and checks the ID. Preserved the body-search assertions and added a title-only Alpha query requiring its nonempty fixture ID. Source row keys: `tests/smoke.sh:100`, `tests/smoke.ps1:151`.
- **S05 — Repaired; §9.** tags::tag_join_filters_doogats used equality and selected only the title. Preserved that query and its positive/negative assertions, then added the source LIKE substring JOIN shape selecting id/title and asserting the matching ID and titles. Source row keys: `tests/smoke.sh:108`, `tests/smoke.ps1:161`.
- **S06 — Repaired; §10a.** PowerShell requires the word token url, while contains("url") also accepts bare_url. link_kinds::all_link_kinds_indexed now uses the same word-boundary regex, preserving embed/markdown/wikilink assertions. Source row keys: `tests/smoke.sh:117`, `tests/smoke.ps1:173`.
- **S07 — Repaired; §11.** sql_lifecycle::insert_and_select captured IDs without validating format or INSERT status. It now asserts successful INSERTs and 14 ASCII digits for each returned ID, retaining both row/value readbacks and the other mapped CREATE/UPDATE/DELETE tests. Source row keys: `tests/smoke.sh:136`, `tests/smoke.ps1:193`.
- **S08 — Repaired; §11e.** alter_table_rename::cli_alter_table_rename_to_renames_typedef_and_data omitted source CREATE/RENAME/DROP output checks. Added table-created and renamed-to messages plus successful DROP CASCADE with dropped output; row preservation and reserved-target rejection coverage remain. Source row keys: `tests/smoke.sh:182`, `tests/smoke.ps1:259`.
- **S09 — Repaired; §11f.** cli_create_fk_freshness::create_link_after_category_in_separate_process_succeeds_without_reindex omitted the two CREATE output checks and the source parent --set fixture. Added both table-created messages and fqn=test.fqn; preserved immediate fresh-process FK creation and the Part B ID assertions. Added the predicates import required by the new output checks. Source row keys: `tests/smoke.sh:193`, `tests/smoke.ps1:275`.
- **S10 — Repaired; §11g.** inline_zone::inline_zone_frontmatter_materializes_and_survives_reindex omitted source CREATE and DROP messages. Added both around the existing inline-zone readback/reindex regression. Source row keys: `tests/smoke.sh:204`, `tests/smoke.ps1:291`.
- **S11 — Repaired; §12a.** sql_lifecycle::install_meeting_minutes_type read attendees but omitted the quoted-table SELECT date and DELETE count checks. Added SELECT date by ID with 2026-03-10 readback and the 1 row(s) affected DELETE message. Source row keys: `tests/smoke.sh:215`, `tests/smoke.ps1:306`.
- **S12 — Repaired; §16d.** PowerShell requires 14-digit output from discover orphans and similar. discover::discover_orphans now validates its fixture ID; discover::discover_similar captures/validates the suggested row ID and asserts it in output, preserving existing title and nonempty-suggestion checks. Source row keys: `tests/smoke.sh:283`, `tests/smoke.ps1:382`.
- **S13 — Repaired; §16e.** fix::fix_dry_run_no_changes compared commit count, which does not prove HEAD identity. Added successful rev-parse HEAD before/after and exact byte equality, retaining commit counts. fix::fix_commits_changes now includes the source apple fixture/readback while preserving hash normalization; readback must exit successfully. Source row keys: `tests/smoke.sh:305`, `tests/smoke.ps1:409`.
- **S14 — Repaired; §18b.** cascade_delete::delete_rejected_by_not_null_references_sql_issue_10 ended after rejection/preservation checks. Appended removal of the blocking child, then successful deletion of the same parent, requiring both Bash one-row messages. Existing CLI rejection and diagnostic assertions remain. Source row keys: `tests/smoke.sh:485`, `tests/smoke.ps1:600`.
- **S15 — Repaired; §21.** sql_lifecycle::default_next_auto_increments now asserts the source table-created message; existing 1/2/3, explicit 99, and subsequent 100 assertions remain. Source row keys: `tests/smoke.sh:542`, `tests/smoke.ps1:658`.
- **S16 — Repaired; §23.** sql_expressions_test::insert_with_coalesce omitted source ID-format checks and the COALESCE(MAX(...),0)+1 composition after an existing row. Added the CREATE message, 14-digit checks for both inserts, the composed second insert and its value-1 readback. Existing IFNULL, NULLIF, nested-expression and rejected-function targets remain. Source row keys: `tests/smoke.sh:571`, `tests/smoke.ps1:688`.
- **S17 — Repaired; §24.** set_flag::create_with_multiple_set_flags now validates the 14-digit ID. update_set_adds_field preserves its add-field regression and additionally overwrites a nonempty field with another nonempty value, asserting replacement. Existing clear/unset/malformed-input checks remain. Source row keys: `tests/smoke.sh:594`, `tests/smoke.ps1:715`.
- **S18 — Repaired; §29.** alter_column_type::widens_varchar_to_text_and_accepts_longer_values only required nonempty IDs and never selected the widened row. Added 14-digit boundary/post-TEXT IDs and fresh-process SELECT id readback of the widened row. The Part B VARCHAR(32)->VARCHAR(100)->TEXT and original VARCHAR(255) cases remain. Source row keys: `tests/smoke.sh:671`, `tests/smoke.ps1:802`.
- **S19 — Repaired; §31.** schema_apply::reapply_is_idempotent_noop checked a later schema diff, not the actual second apply output. The second apply now independently asserts absence of create_type; all later diff/query assertions remain. Source row keys: `tests/smoke.sh:700`, `tests/smoke.ps1:835`.
- **S20 — Repaired; §6b, 7d, 10, 10a, 10b, 11, 16d, 16e, 16f, 17, 17b, 18, 18b.** Several mapped success-path fixture/read commands captured Output without asserting exit status, whereas Bash errexit/PowerShell ddb enforce success. Added status checks to the existing captured commands in delete_backlinks, search_links, link_kinds, rename, sql_lifecycle, discover, fix and cascade_delete targets. Checked raw git add/commit results in the folder fixture and sequence module fixture helper. No common.rs or application harness changes. Source row keys: `tests/smoke.sh:62`, `tests/smoke.sh:89`, `tests/smoke.sh:113`, `tests/smoke.sh:117`, `tests/smoke.sh:128`, `tests/smoke.sh:136`, `tests/smoke.sh:283`, `tests/smoke.sh:305`, `tests/smoke.sh:321`, `tests/smoke.sh:418`, `tests/smoke.sh:441`, `tests/smoke.sh:465`, `tests/smoke.sh:485`, `tests/smoke.ps1:109`, `tests/smoke.ps1:141`, `tests/smoke.ps1:168`, `tests/smoke.ps1:173`, `tests/smoke.ps1:184`, `tests/smoke.ps1:193`, `tests/smoke.ps1:382`, `tests/smoke.ps1:409`, `tests/smoke.ps1:430`, `tests/smoke.ps1:531`, `tests/smoke.ps1:554`, `tests/smoke.ps1:581`, `tests/smoke.ps1:600`.

## Complete section inventory

Covered means the source assertions are preserved by the inspected targets. S-codes identify repaired gaps; M-codes require only the parent migration-map edit. Different fixture names, row counts with equivalent count assertions, and literal fallback values were accepted where the command behavior and asserted property remain the same.

| Source range | Section | Inspected target tests | Disposition |
| --- | --- | --- | --- |
| `tests/smoke.sh:26-29` | 1 | `smoke_core::smoke_01_init` | M01 |
| `tests/smoke.sh:30-41` | 2 | `smoke_core::smoke_02_create_unique_ids`; `cli_create_unregistered_type::cli_create_unregistered_type_creates_base_doogat_with_warning` | Covered |
| `tests/smoke.sh:42-46` | 3 | `smoke_core::smoke_03_read` | Covered |
| `tests/smoke.sh:47-51` | 4 | `cli_update_app_error::cli_update_existing_prints_updated_and_changes_title`; `sync_crdt::non_overlapping_edits_merge_cleanly`; `sync_crdt::non_overlapping_edits_merge_cleanly` | M02 |
| `tests/smoke.sh:52-57` | 5 | `smoke_core::smoke_05_delete` | Covered |
| `tests/smoke.sh:58-61` | 6 | `smoke_core::smoke_06_status` | Covered |
| `tests/smoke.sh:62-72` | 6b | `delete_backlinks::delete_reports_broken_backlinks`; `delete_backlinks::status_reports_broken_backlinks_after_delete` | S20 |
| `tests/smoke.sh:73-76` | 7 | `smoke_core::smoke_07_reindex`; `sql_lifecycle::reindex_preserves_table` | Covered |
| `tests/smoke.sh:77-82` | 7b | `tags::body_hashtag_indexed_with_source` | S01 |
| `tests/smoke.sh:83-88` | 7c | `checkboxes::checkbox_filter_by_state` | S02 |
| `tests/smoke.sh:89-99` | 7d | `folder_namespace::insert_folder_typedef_creates_subdirectory_path` | S03, S20 |
| `tests/smoke.sh:100-103` | 8 | `search_links::full_text_search` | S04 |
| `tests/smoke.sh:104-107` | 8b | `search_links::paginated_search_shows_header` | Covered |
| `tests/smoke.sh:108-112` | 9 | `smoke_sql::smoke_09_sql_queries`; `tags::tag_join_filters_doogats` | S05 |
| `tests/smoke.sh:113-116` | 10 | `search_links::wikilink_indexed_in_links_table` | S20 |
| `tests/smoke.sh:117-127` | 10a | `link_kinds::all_link_kinds_indexed` | S06, S20 |
| `tests/smoke.sh:128-135` | 10b | `rename::rename_moves_file_and_rewrites_backlinks` | S03, S20 |
| `tests/smoke.sh:136-145` | 11 | `sql_lifecycle::create_table`; `sql_lifecycle::insert_and_select`; `sql_lifecycle::update_row`; `sql_lifecycle::delete_row` | S07, S20 |
| `tests/smoke.sh:146-159` | 11c | `smoke_sql::smoke_11c_ghost_row_recovery` | Covered |
| `tests/smoke.sh:160-169` | 11d | `smoke_sql::smoke_11d_join_smoke_pin` | Covered |
| `tests/smoke.sh:170-175` | 11a | `smoke_sql::smoke_11a_alter_zone_title_template` | Covered |
| `tests/smoke.sh:176-181` | 11b | `smoke_sql::smoke_11b_create_table_if_not_exists` | Covered |
| `tests/smoke.sh:182-192` | 11e | `alter_table_rename::cli_alter_table_rename_to_renames_typedef_and_data`; `alter_table_rename::cli_alter_table_rename_rejects_reserved_target` | S08 |
| `tests/smoke.sh:193-203` | 11f | `cli_create_fk_freshness::create_link_after_category_in_separate_process_succeeds_without_reindex` | S09 |
| `tests/smoke.sh:204-210` | 11g | `inline_zone::inline_zone_frontmatter_materializes_and_survives_reindex` | S10 |
| `tests/smoke.sh:211-214` | 12 | `smoke_core::smoke_12_install_bundled_type` | Covered |
| `tests/smoke.sh:215-221` | 12a | `sql_lifecycle::install_meeting_minutes_type` | S11 |
| `tests/smoke.sh:222-226` | 13 | `smoke_core::smoke_13_type_suggest` | Covered |
| `tests/smoke.sh:227-236` | 14 | `smoke_core::smoke_14_register_node_and_compact` | Covered |
| `tests/smoke.sh:237-242` | 15 | `smoke_core::smoke_15_node_list_and_retire` | Covered |
| `tests/smoke.sh:243-248` | 16 | `smoke_core::smoke_16_compact_dry_run` | Covered |
| `tests/smoke.sh:249-258` | 16a | `smoke_core::smoke_16a_compact_no_backup` | Covered |
| `tests/smoke.sh:259-266` | 16b | `smoke_core::smoke_16b_compact_backup_path` | Covered |
| `tests/smoke.sh:267-282` | 16c | `smoke_core::smoke_16c_maintenance` | Covered |
| `tests/smoke.sh:283-304` | 16d | `discover::discover_stale`; `discover::discover_orphans`; `discover::discover_mentions`; `discover::discover_similar`; `discover::discover_recent_shows_new_doogat`; `discover::discover_link_density_shows_hub` | S12, S20 |
| `tests/smoke.sh:305-320` | 16e | `fix::fix_dry_run_no_changes`; `fix::fix_commits_changes`; `fix::fix_idempotent` | S13, S20 |
| `tests/smoke.sh:321-417` | 16f | `sequence::sequence_tree_display`; `sequence::sequence_breadcrumb_display`; `sequence::sequence_broken_list`; `help::help_create_app_prints_guide`; `help::help_no_topic_lists_guides`; `help::help_unknown_topic_fails`; `smoke_core::smoke_16f_log_level_flag`; `smoke_sql::smoke_16f_app_building_flow` | S20 |
| `tests/smoke.sh:418-440` | 17 | `cascade_delete::cascade_delete_cleans_junction_table`; `cascade_delete::cascade_delete_removes_wikilink_from_referencing_file` | S20 |
| `tests/smoke.sh:441-464` | 17b | `cascade_delete::cascade_delete_cleans_owned_junction_when_parent_deleted` | S20 |
| `tests/smoke.sh:465-484` | 18 | `cascade_delete::cascade_delete_via_ddb_delete_cleans_junction_and_refs` | S20 |
| `tests/smoke.sh:485-510` | 18b | `cascade_delete::delete_rejected_by_not_null_references_sql_issue_10`; `cascade_delete::delete_rejected_by_not_null_references_cli_issue_10` | S14, S20 |
| `tests/smoke.sh:511-533` | 19 | `smoke_sql::smoke_19_boolean_consistency` | Covered |
| `tests/smoke.sh:534-541` | 20 | `smoke_sql::smoke_20_type_table_self_contained` | Covered |
| `tests/smoke.sh:542-563` | 21 | `sql_lifecycle::default_next_auto_increments` | S15 |
| `tests/smoke.sh:564-570` | 22 | `smoke_search::smoke_22_fts5_search_boost` | Covered |
| `tests/smoke.sh:571-593` | 23 | `sql_expressions_test::insert_with_coalesce`; `sql_expressions_test::update_with_ifnull`; `sql_expressions_test::insert_with_arithmetic`; `sql_expressions_test::nullif_in_insert`; `sql_expressions_test::insert_with_nested_expression`; `sql_expressions_test::insert_rejects_unlisted_function` | S16 |
| `tests/smoke.sh:594-609` | 24 | `set_flag::create_with_multiple_set_flags`; `set_flag::update_set_adds_field`; `set_flag::update_unset_removes_field`; `set_flag::update_set_empty_value_clears_field`; `set_flag::create_with_malformed_set_returns_error` | M03, S17 |
| `tests/smoke.sh:610-618` | 25 | `date_default::insert_without_date_derives_from_id`; `date_default::insert_with_explicit_date_preserves_value` | Covered |
| `tests/smoke.sh:619-632` | 26 | `smoke_search::smoke_26_fts_negation` | Covered |
| `tests/smoke.sh:633-651` | 27 | `smoke_sql::smoke_27_on_conflict_do_nothing` | Covered |
| `tests/smoke.sh:652-670` | 28 | `smoke_search::smoke_28_in_query_field_filter` | Covered |
| `tests/smoke.sh:671-682` | 29 | `alter_column_type::widens_varchar_to_text_and_accepts_longer_values` | S18 |
| `tests/smoke.sh:683-699` | 30 | `smoke_sql::smoke_30_singleton_typedef_crud` | Covered |
| `tests/smoke.sh:700-712` | 31 | `schema_apply::dry_run_prints_plan_and_creates_nothing`; `schema_apply::apply_creates_declared_type`; `schema_apply::reapply_is_idempotent_noop` | S19 |
| `tests/smoke.sh:713-742` | 32 | `poison_file_reindex::lenient_reindex_skips_poison_file_and_indexes_the_rest`; `poison_file_reindex::strict_reindex_fails_and_names_the_poison_file`; `smoke_resilience::smoke_32_cli_answers_after_strict_abort` | Covered |
| `tests/smoke.sh:743-769` | 33 | `smoke_resilience::smoke_33_reindex_report_and_create_warning` | Covered |
| `tests/smoke.ps1:66-69` | 1 | `smoke_core::smoke_01_init` | M01 |
| `tests/smoke.ps1:70-86` | 2 | `smoke_core::smoke_02_create_unique_ids`; `cli_create_unregistered_type::cli_create_unregistered_type_creates_base_doogat_with_warning` | Covered |
| `tests/smoke.ps1:87-91` | 3 | `smoke_core::smoke_03_read` | Covered |
| `tests/smoke.ps1:92-97` | 4 | `cli_update_app_error::cli_update_existing_prints_updated_and_changes_title`; `sync_crdt::non_overlapping_edits_merge_cleanly`; `sync_crdt::non_overlapping_edits_merge_cleanly` | M02 |
| `tests/smoke.ps1:98-103` | 5 | `smoke_core::smoke_05_delete` | Covered |
| `tests/smoke.ps1:104-108` | 6 | `smoke_core::smoke_06_status` | Covered |
| `tests/smoke.ps1:109-121` | 6b | `delete_backlinks::delete_reports_broken_backlinks`; `delete_backlinks::status_reports_broken_backlinks_after_delete` | S20 |
| `tests/smoke.ps1:122-126` | 7 | `smoke_core::smoke_07_reindex`; `sql_lifecycle::reindex_preserves_table` | Covered |
| `tests/smoke.ps1:127-133` | 7b | `tags::body_hashtag_indexed_with_source` | S01 |
| `tests/smoke.ps1:134-140` | 7c | `checkboxes::checkbox_filter_by_state` | S02 |
| `tests/smoke.ps1:141-150` | 7d | `folder_namespace::insert_folder_typedef_creates_subdirectory_path` | S03, S20 |
| `tests/smoke.ps1:151-155` | 8 | `search_links::full_text_search` | S04 |
| `tests/smoke.ps1:156-160` | 8b | `search_links::paginated_search_shows_header` | Covered |
| `tests/smoke.ps1:161-167` | 9 | `smoke_sql::smoke_09_sql_queries`; `tags::tag_join_filters_doogats` | S05 |
| `tests/smoke.ps1:168-172` | 10 | `search_links::wikilink_indexed_in_links_table` | S20 |
| `tests/smoke.ps1:173-183` | 10a | `link_kinds::all_link_kinds_indexed` | S06, S20 |
| `tests/smoke.ps1:184-192` | 10b | `rename::rename_moves_file_and_rewrites_backlinks` | S03, S20 |
| `tests/smoke.ps1:193-208` | 11 | `sql_lifecycle::create_table`; `sql_lifecycle::insert_and_select`; `sql_lifecycle::update_row`; `sql_lifecycle::delete_row` | S07, S20 |
| `tests/smoke.ps1:209-225` | 11c | `smoke_sql::smoke_11c_ghost_row_recovery` | Covered |
| `tests/smoke.ps1:226-240` | 11d | `smoke_sql::smoke_11d_join_smoke_pin` | Covered |
| `tests/smoke.ps1:241-249` | 11a | `smoke_sql::smoke_11a_alter_zone_title_template` | Covered |
| `tests/smoke.ps1:250-258` | 11b | `smoke_sql::smoke_11b_create_table_if_not_exists` | Covered |
| `tests/smoke.ps1:259-274` | 11e | `alter_table_rename::cli_alter_table_rename_to_renames_typedef_and_data`; `alter_table_rename::cli_alter_table_rename_rejects_reserved_target` | S08 |
| `tests/smoke.ps1:275-290` | 11f | `cli_create_fk_freshness::create_link_after_category_in_separate_process_succeeds_without_reindex` | S09 |
| `tests/smoke.ps1:291-300` | 11g | `inline_zone::inline_zone_frontmatter_materializes_and_survives_reindex` | S10 |
| `tests/smoke.ps1:301-305` | 12 | `smoke_core::smoke_12_install_bundled_type` | Covered |
| `tests/smoke.ps1:306-315` | 12a | `sql_lifecycle::install_meeting_minutes_type` | S11 |
| `tests/smoke.ps1:316-321` | 13 | `smoke_core::smoke_13_type_suggest` | Covered |
| `tests/smoke.ps1:322-333` | 14 | `smoke_core::smoke_14_register_node_and_compact` | Covered |
| `tests/smoke.ps1:334-341` | 15 | `smoke_core::smoke_15_node_list_and_retire` | Covered |
| `tests/smoke.ps1:342-347` | 16 | `smoke_core::smoke_16_compact_dry_run` | Covered |
| `tests/smoke.ps1:348-354` | 16a | `smoke_core::smoke_16a_compact_no_backup` | Covered |
| `tests/smoke.ps1:355-362` | 16b | `smoke_core::smoke_16b_compact_backup_path` | Covered |
| `tests/smoke.ps1:363-381` | 16c | `smoke_core::smoke_16c_maintenance` | Covered |
| `tests/smoke.ps1:382-408` | 16d | `discover::discover_stale`; `discover::discover_orphans`; `discover::discover_mentions`; `discover::discover_similar`; `discover::discover_recent_shows_new_doogat`; `discover::discover_link_density_shows_hub` | S12, S20 |
| `tests/smoke.ps1:409-429` | 16e | `fix::fix_dry_run_no_changes`; `fix::fix_commits_changes`; `fix::fix_idempotent` | S13, S20 |
| `tests/smoke.ps1:430-530` | 16f | `sequence::sequence_tree_display`; `sequence::sequence_breadcrumb_display`; `sequence::sequence_broken_list`; `help::help_create_app_prints_guide`; `help::help_no_topic_lists_guides`; `help::help_unknown_topic_fails`; `smoke_core::smoke_16f_log_level_flag`; `smoke_sql::smoke_16f_app_building_flow` | S20 |
| `tests/smoke.ps1:531-553` | 17 | `cascade_delete::cascade_delete_cleans_junction_table`; `cascade_delete::cascade_delete_removes_wikilink_from_referencing_file` | S20 |
| `tests/smoke.ps1:554-580` | 17b | `cascade_delete::cascade_delete_cleans_owned_junction_when_parent_deleted` | S20 |
| `tests/smoke.ps1:581-599` | 18 | `cascade_delete::cascade_delete_via_ddb_delete_cleans_junction_and_refs` | S20 |
| `tests/smoke.ps1:600-626` | 18b | `cascade_delete::delete_rejected_by_not_null_references_sql_issue_10`; `cascade_delete::delete_rejected_by_not_null_references_cli_issue_10` | S14, S20 |
| `tests/smoke.ps1:627-649` | 19 | `smoke_sql::smoke_19_boolean_consistency` | Covered |
| `tests/smoke.ps1:650-657` | 20 | `smoke_sql::smoke_20_type_table_self_contained` | Covered |
| `tests/smoke.ps1:658-679` | 21 | `sql_lifecycle::default_next_auto_increments` | S15 |
| `tests/smoke.ps1:680-687` | 22 | `smoke_search::smoke_22_fts5_search_boost` | Covered |
| `tests/smoke.ps1:688-714` | 23 | `sql_expressions_test::insert_with_coalesce`; `sql_expressions_test::update_with_ifnull`; `sql_expressions_test::insert_with_arithmetic`; `sql_expressions_test::nullif_in_insert`; `sql_expressions_test::insert_with_nested_expression`; `sql_expressions_test::insert_rejects_unlisted_function` | S16 |
| `tests/smoke.ps1:715-732` | 24 | `set_flag::create_with_multiple_set_flags`; `set_flag::update_set_adds_field`; `set_flag::update_unset_removes_field`; `set_flag::update_set_empty_value_clears_field`; `set_flag::create_with_malformed_set_returns_error` | M03, S17 |
| `tests/smoke.ps1:733-743` | 25 | `date_default::insert_without_date_derives_from_id`; `date_default::insert_with_explicit_date_preserves_value` | Covered |
| `tests/smoke.ps1:744-757` | 26 | `smoke_search::smoke_26_fts_negation` | Covered |
| `tests/smoke.ps1:758-782` | 27 | `smoke_sql::smoke_27_on_conflict_do_nothing` | Covered |
| `tests/smoke.ps1:783-801` | 28 | `smoke_search::smoke_28_in_query_field_filter` | Covered |
| `tests/smoke.ps1:802-814` | 29 | `alter_column_type::widens_varchar_to_text_and_accepts_longer_values` | S18 |
| `tests/smoke.ps1:815-834` | 30 | `smoke_sql::smoke_30_singleton_typedef_crud` | Covered |
| `tests/smoke.ps1:835-858` | 31 | `schema_apply::dry_run_prints_plan_and_creates_nothing`; `schema_apply::apply_creates_declared_type`; `schema_apply::reapply_is_idempotent_noop` | S19 |
| `tests/smoke.ps1:859-890` | 32 | `poison_file_reindex::lenient_reindex_skips_poison_file_and_indexes_the_rest`; `poison_file_reindex::strict_reindex_fails_and_names_the_poison_file`; `smoke_resilience::smoke_32_cli_answers_after_strict_abort` | Covered |
| `tests/smoke.ps1:891-932` | 33 | `smoke_resilience::smoke_33_reindex_report_and_create_warning` | Covered |

## Dialect and scaffolding treatment

- PowerShell-only strictness was inspected directly: explicit duplicate-UNIQUE/SINGLETON exit failures are preserved by smoke_sql failure assertions; RESTRICT exit failure is preserved in cascade_delete; discover orphan/similar numeric IDs and the exact url token required the repairs above. PowerShell does not assert maintenance enabled/disabled output, but Bash does; smoke_core preserves both messages.
- Bash-only explicit cleanup output checks are retained, including dropped messages in the relevant SQL tests and both one-row recovery deletes after RESTRICT. Original Bash failure-capture forms sometimes only inspect diagnostic text; the Rust failure assertions retain the stronger PowerShell exit-status contract.
- The entire §16f compound range was inspected: sequence tree/breadcrumb/broken references, the unnumbered --log-level block, Bash standalone help-list/unknown-help assertions, and both dialects' app-building ENUM/reference-zone/title-template/junction flow. Help coverage lives in help.rs; app-building value/ID/count/output checks live in smoke_sql::smoke_16f_app_building_flow.
- Build/binary-location preparation, temp-directory cleanup, pass banners, ANSI/tracing filtering in the PowerShell wrapper, and sleeps are script scaffolding, not independent application assertions. Equivalent process success is checked by assert_cmd, temporary cleanup by TempDir, and platform executable selection by the existing ddb_bin helper. No numbered row consists solely of scaffolding and none was dropped.
- §32 is CLI usability after a strict abort, not proof that the index was preserved; the existing source caveat and corresponding Rust test meaning remain intact. §33 separately binds quiet reindex stdout and the create AppWarning stderr channel.

## Validation and files

No cargo command, test runner, or commit was run by this agent. Rustfmt 1.89 parsed/formatted the changed files and scoped git diff --check passed; parent/Devon owns compilation and focused execution. The missing predicates import found during parent compilation was repaired immediately in cli_create_fk_freshness.rs. Parent review also retained the original equality JOIN before adding LIKE, and removed the redundant substring-url assertion after strengthening it to a word-token check.

Changed smoke test files:

- `tests/e2e/tags.rs`
- `tests/e2e/checkboxes.rs`
- `tests/e2e/search_links.rs`
- `tests/e2e/folder_namespace.rs`
- `tests/e2e/rename.rs`
- `tests/e2e/link_kinds.rs`
- `tests/e2e/schema_apply.rs`
- `tests/e2e/sql_lifecycle.rs`
- `tests/e2e/alter_table_rename.rs`
- `tests/e2e/cli_create_fk_freshness.rs`
- `tests/e2e/inline_zone.rs`
- `tests/e2e/discover.rs`
- `tests/e2e/fix.rs`
- `tests/e2e/cascade_delete.rs`
- `tests/e2e/set_flag.rs`
- `tests/e2e/sql_expressions_test.rs`
- `tests/e2e/alter_column_type.rs`
- `tests/e2e/delete_backlinks.rs`
- `tests/e2e/sequence.rs`

The preceding four sync authoring repairs and their diagnosis are recorded separately in `dev/local/reviews/00171-task12-partb-audit.md`.
