# Task 12 Part B assertion audit

Scope: read-only reconciliation of the integration Bash and PowerShell source scenarios against their mapped Rust e2e tests. This records the concrete gaps found in the bounded Part B audit and the test changes used to close them. Production code and the retired scripts were not edited. Parent owns the migration map, cargo validation, commits, and final completion decisions.

The source row key identifies the numbered scenario, because integration section numbers repeat and their order is not numeric. Locations refer to the audited source files before script removal.

## Prior bounded fixes

| Source row | Bash / PowerShell location | Rust target | Gap and fix |
| --- | --- | --- | --- |
| §23, frontmatter conflict | `tests/integration.sh:2918` / `tests/integration.ps1:2842` | `sync_crdt::frontmatter_conflict_creates_fm_crdt_file` | Extract the actual title line and require Laptop Title or Desktop Title. Preserve the existing nonempty frontmatter CRDT file and compaction assertions. |
| §27b, delete versus edit | `tests/integration.sh:3028` / `tests/integration.ps1:2953` | `sync_crdt::sync_output_reports_resurrected_count` | Check the edited content after resurrection and the `resurrected` status diagnostic. The current continuation also checks the initial title before deletion. |
| §41, node auto-registration | `tests/integration.sh:3464` / `tests/integration.ps1:3438` | `sync_crdt::sync_auto_registers_node_when_none_exists` | Compare node UUID file bytes before and after a second sync. |
| §47, VARCHAR alteration | `tests/integration.sh:3516` / `tests/integration.ps1:3498` | `alter_column_type::widens_varchar_to_text_and_accepts_longer_values` | Retain the prior VARCHAR(255) regression and add the source VARCHAR(32) pre-ALTER rejection, VARCHAR(100) widening, failed narrowing with two existing overlength rows, and TEXT widening/fresh-process writes. |

## Remaining findings and fixes

| Source row | Bash / PowerShell location | Rust target | Gap and fix |
| --- | --- | --- | --- |
| §26b, convergence | `tests/integration.sh:3002` / `tests/integration.ps1:2926` | `multi_device::concurrent_edits_same_doogat` | Require a real sync reporting a nonzero resolved-conflict count, then retain three-node byte equality and require an actual title line equal to one of the concurrent writes. |
| §27b, delete versus edit | `tests/integration.sh:3028` / `tests/integration.ps1:2953` | `sync_crdt::sync_output_reports_resurrected_count` | Read and check `title: Doomed` before deleting the shared row. |
| §27, full bundle bootstrap | `tests/integration.sh:3168` / `tests/integration.ps1:3096` | `multi_device::bundle_full_bootstrap` | Build a real merged reference fixture before export; require the import message and both laptop/desktop references on the fresh imported node, preserving the existing title assertion. |
| §28a, bundle conflict recovery | `tests/integration.sh:3193` / `tests/integration.ps1:3125` | `bundle_import_conflict_recovery::bundle_import_recovers_real_conflict_and_reimport_is_noop` | Require the `imported:` prefix in both resolution/no-op messages and check the winner using the actual title line. |
| §32, NoSQL CLI | `tests/integration.sh:3284` / `tests/integration.ps1:3235` | `integration_nosql_cli::integration_32_nosql_cli_commands` | Update the fixture before `get`, require the updated title, validate the typed fixture ID, and require a 14-digit line from typed scan. |
| §33, compaction resync | `tests/integration.sh:3298` / `tests/integration.ps1:3252` | `multi_device::compacted_node_resolves_second_conflict`; `multi_device::stale_node_resync_after_compaction`; existing `integration_compact_stats::integration_33_compact_byte_stats_report` | Add a second conflict resolved on the same node that performed compaction, including byte-stat output and a title line after resolution. Preserve the original three-node stale-node regression and strengthen its title-line assertion. |
| §34, multi-row INSERT | `tests/integration.sh:3347` / `tests/integration.ps1:3301` | `sql_lifecycle::multi_row_insert` | Require each of the three comma-separated IDs to contain exactly 14 ASCII digits and require exact row count 3. Preserve value/score assertions. |
| §35, transactions | `tests/integration.sh:3355` / `tests/integration.ps1:3310` | `sql_lifecycle::transaction_rollback_via_cli` and existing commit scenario | Retain rollback-on-empty coverage; add a committed row, roll back another insert, then require exact count 1 and the previously committed value. |
| §37, junction round trip | `tests/integration.sh:3374` / `tests/integration.ps1:3333` | `junction_tables::junction_table_round_trip` | Require the source one-row messages for junction INSERT and DELETE. |
| §39, migration fix | `tests/integration.sh:3398` / `tests/integration.ps1:3363` | `fix::fix_migrate_rewrites_zones` | Invoke `fix --migrate --verbose`, retaining the existing migration assertions. |
| §48, typed writes/cascade | `tests/integration.sh:3563` / `tests/integration.ps1:3538` | `integration_typed_write_cli::integration_48_typed_write_blockers_and_cascade`; `cascade_delete::delete_rejected_by_not_null_references_cli_issue_10` | Require both post-cascade SELECT processes to succeed before checking absent IDs, and require RESTRICT stderr to identify `category-membership.link_id`. |
| §49, typed write paths | `tests/integration.sh:3626` / `tests/integration.ps1:3595` | `integration_typed_write_cli::integration_49_unify_typed_write_paths` | Require both category seed IDs to be nonempty and the wrong-type foreign-key command to exit unsuccessfully. |
| §50, fresh-process FK | `tests/integration.sh:3697` / `tests/integration.ps1:3654` | `cli_create_fk_freshness::create_link_after_category_in_separate_process_succeeds_without_reindex` | Require both successful creation IDs to contain exactly 14 ASCII digits, retaining distinctness and persistence assertions. |
| §63, poison-file GraphQL | `tests/integration.sh:3746` / `tests/integration.ps1:3703` | `integration_id_poison::integration_63_poison_file_graphql_warning` | Require the response `data` key to exist, in addition to errors being absent. The source helper requires key presence, not non-null data. |

Mapping-only follow-up: §30 (`tests/integration.sh:3225` / `tests/integration.ps1:3155`) also needs to cite existing `sql_lifecycle::bulk_update` and `sql_lifecycle::bulk_delete` coverage. Parent owns that map edit.

## Parent validation correction

During this continuation, parent validation identified that the extra `affected == 1` assertion in §55B did not match the existing INSERT contract: the response returned the winner ID in `message` and `affected: null`. Removed `affected` from the GraphQL selection and assertion in `integration_singleton_races::integration_55_b_graphql_execute_sql_singleton_insert_race`. The exact durable count, winner ID, HEAD payload, readback, and structured loser checks remain.

## Validation ownership

Parent's focused pre-commit run passed 140 of 144 tests and exposed four fixture-order failures. `MultiNodeSetup::push` delegates to raw `git push` without checking its exit status (`tests/e2e/common.rs:602`). A peer's initial sync advances remote registration/sync metadata, so later raw pushes from node0 can silently fail non-fast-forward. Updated node0 publication in `bundle_full_bootstrap`, `compacted_node_resolves_second_conflict`, and `concurrent_edits_same_doogat` to use the existing checked CLI sync helper. Peer edits remain unpublished until the asserted conflict-resolution call; all conflict-count and content assertions remain intact.

The fourth failure came from reading the merged title through the CLI before inspecting `_fm.crdt`. Parent traced repository reopening to `cleanup_orphaned_crdt_temp`, which removes those temporary files. Moved the CLI title read after the existing file-existence/data assertions and before compaction in `frontmatter_conflict_creates_fm_crdt_file`. No assertion was removed or weakened. Parent owns the rerun of these four repaired tests.

No cargo command, test runner, or commit was run by this agent. Parent should validate the changed targets above, including the new compacted-node scenario and the modified bundle bootstrap. Repo-pinned Rust 1.89 rustfmt completed successfully for the 12 changed test files, and scoped `git diff --check` passed. These are static checks only. This audit records concrete assertion reconciliation, not a claim that test execution has passed.
