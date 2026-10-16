# Task 12: integration Part A reconciliation

The audit compared both shell dialects' assertion bodies with the registered Rust test bodies. Runner discovery was checked separately: all four source inventories match the mapping keys (56 + 56 + 140 + 139 = 391). Registration alone was not treated as assertion equivalence. Validation results are recorded in the task verification report after the final edits.

## Repairs to previously claimed coverage

| Source sections | Registered targets and preserved assertions |
| --- | --- |
| 18d3 | `search_in_filter::search_where_in_tag` now includes single-value tag membership, exact count and matching title. Existing materialized-column and empty-array cases remain. |
| 18g | `integration_graphql_server::integration_18g_normalize_query_implicit_and_lowercase` also checks standalone `B AND A` normalization. Existing field-order and search-result normalization tests remain mapped. |
| 18z3 | Tag-clear test verifies the initial three tags before clearing. |
| 19 | REST tag filtering checks the created ID, field filtering checks the returned custom field value, and missing-auth POST explicitly returns 401. Existing lifecycle/status, no-match and sort assertions remain. |
| 20 | PgWire catalog filtering rejects every `_ddb_` name; direct internal access requires a numeric count. Compact calls request the original statistics fields, and force/noBackup variants require the gcSuccess and backupPath keys, including explicit null for noBackup. Existing pure-Rust PgWire, NoSQL, SQL-error, backup-path and maintenance ports cover the remaining assertions unconditionally across platforms. |
| 38b2b | `reference_raw_id::plural_resolver_with_order_by_and_limit` checks the combined DESC + limit:2 result as cherry then banana. Separate ASC, DESC, limit and raw-ID/object tests are retained. |
| 38c/c2 | SQL array response checks exact id/title column names. Boolean port checks nonempty INSERT ID and SELECT of the core title, in addition to true/false conversion. |
| 38i | Typed mutation setup checks CREATE message and 14-digit ID; unset returns the same ID. Hyphenated typed query checks totalCount as well as field values. |
| 42b | Mapping now names the actual registered `integration_orderby_base_fields::integration_42b_orderby_base_fields_deterministic_pagination`, which checks every source ordering and both pages. |
| 44J | Both category seed IDs are nonempty and distinct before the junction assertions. |
| 45G12 / 45A2 | Intra-batch IGNORE survivor and successful post-restart INSERT IDs must be nonempty. Existing duplicate, restart, durable row-count and recovery assertions remain. |
| 50a | New-name query must have a data key as required by the source response helper. |
| 51A | Empty singular query has a data key; first upsert returns a nonempty ID. The test covers the full flow, replacing stale “uncovered halves only” map wording. |
| 52B | CLI creates/inserts, validates multi-row IDs, rejects SET SINGLETON with the original row-count diagnostic, and deletes one row with the expected message before the GraphQL SET/DROP introspection flow. |
| 53C | DEFAULT VALUES auto-seeding now follows the original CLI create/count/default-field/schema_version/drop workflow. |
| 56 banner | The warnings fixture asserts both CLI CREATE and SET TITLE TEMPLATE output messages before starting the server; this row is executable setup coverage, not an assertion-free banner. |
| 57A/B | Both GraphQL warning responses require the source helper's data-key presence. Warning code/message/placement and empty-array checks remain. |

The older PowerShell 17J/18/19/20 TODO rows were read against their own source bodies and corresponding Rust assertions. They were not inferred complete from the Bash status. Differences retained include Windows' original unconditional PgWire skip being replaced by unconditional tokio_postgres coverage; custom backup paths flow through serde JSON serialization rather than shell escaping. PowerShell's help-only integration section keeps its distinct mapping to the existing help tests.

## Race coverage

`integration_singleton_parity` and `integration_write_lock_races` existed but were absent from `main.rs`; both are now registered. New `integration_singleton_races` implements 55B/C/D. GraphQL races require one success and one structured SINGLETON_VIOLATION with the correct table. CLI versus PgWire accepts only the source's singleton or transient contention diagnostics and rejects raw UNIQUE leaks. Every protocol race checks exact final count one; the new cases also bind the survivor to committed HEAD and the winning payload. Existing CLI-only 55A and concurrent-writer 61A remain reused. Section 55E is the union of these exact count assertions, not a synthetic test-count claim.

The six focused race/parity/write checks passed twice before adversarial validation. One earlier failure was an added affected=1 expectation, which the public INSERT response does not promise; it was removed while all original source assertions remained. An independent validator is testing wrong CLI facades against the risk tests. Its final evidence and any resulting strengthening are recorded separately.

## Boundaries

Pure banners and a mid-sentence comment that matches the header regex retain explicit non-scenario dispositions. No shell survivor is planned. Historical mapping-inventory statements about NEW/no existing coverage describe the initial inventory; final status and the reconciliation reports describe the delivered assertions. Production behavior, workflows and shell files have not been changed by this task.

Task commit: `faede1d`. While task 12 was in progress, the separate 00202 session landed `75cde1c` (Clippy closure-borrow correction in git_ops/read.rs). It is preserved and excluded from task 12 review partitions, which use `75cde1c..faede1d`. The original PRD work_start_sha is unchanged.

Per-task Pat review found that porting the CLI 53C source had replaced pre-existing GraphQL auto-seed coverage. Commit `32aa884` restores that GraphQL CREATE/default/count/schema_version block verbatim after the CLI phase; all CLI checks remain. The integration suite re-ran green (83/83, 15.95s).
