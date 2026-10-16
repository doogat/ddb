# Decision Audit Log: 00171-migrate-shell-scenarios-to-e2e-v1

PRD: `00171-migrate-shell-scenarios-to-e2e-v1.md`
Started: 2026-09-07T16:30:13Z
Completed: 2026-09-07T16:30:13Z
Autonomous: 17  |  Deferred: 16  |  Doubts: 0

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: Review diff/commit range for PRD 00171 bundles unrelated PRD 00202 HLC-logic commits (git_ops/read.rs, tests.rs) — a process/traceability issue, not a defect in 00171's own work

**Choice**: discard

**Rationale**: Git history identifies interleaved, separately authored PRD 00202 commits; not scope creep by 00171. Full work_start_sha is intentionally preserved.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: [FIX] Per-push CI coverage dropped: `test.yml`'s new `Smoke test (e2e)` step runs only the 30 `smoke_*` tests (of 565), yet ~26 of smoke.sh's 56 sections were mapped to pre-existing modules outside that filter (delete_backlinks, tags, checkboxes, folder_namespace, search_links, link_kinds, rename, sql_lifecycle, alter_table_rename, cli_create_fk_freshness, inline_zone, discover, fix, sequence, help, cascade_delete, sql_expressions_test, set_flag, date_default, alter_column_type, schema_apply, poison_file_reindex, cli_create_unregistered_type, cli_update_app_error, sync_crdt); smoke.sh ran those on every push, now they run nightly only, contradicting the PRD's "CI keeps identical coverage" — .github/workflows/test.yml:45-48 — replace the `smoke_` step and the five `--exact` steps with one `cargo test -p ddb-e2e` step (the full suite took 223 s in the 5cf4c36 linux-e2e job), drop the `DDB_BIN` env (nothing under tests/ reads it), and fix the test.yml sentence at AGENTS.md:116.

**Choice**: auto-fix

**Rationale**: Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: [FIX] §42 totalCount assertions dropped: bash 1505/1517 asserted `totalCount == 1`; the cited "ALREADY COVERED" tests request the field but never read it — tests/e2e/serve.rs:1408-1410 (`filter_base_field_title_eq`) and :1443-1445 (`filter_base_field_title_contains`) — add `assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 1)` to each.

**Choice**: auto-fix

**Rationale**: Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: [FIX] §44.E1 JOIN pin degraded to single-character containment: `row_str.contains('a')`/`contains('1')` also pass on `["abc","10"]`; bash 1699-1700 grepped the quoted tokens — tests/e2e/integration_junction_sync.rs:35-43 — parse `rows[0]` as JSON and `assert_eq!(row, json!(["a","1"]))`.

**Choice**: auto-fix

**Rationale**: Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: [FIX] §59.A cross-process read-back dropped: bash 2667-2677 read the typed column back through a separate `ddb query` process (its comment names that as the point); the Rust target reads through the same server's executeSql — tests/e2e/rest.rs:153-154 — add `repo.ddb().args(["query", "SELECT category FROM item WHERE id = '<id>'"]).assert().success().stdout(contains("books"))`.

**Choice**: auto-fix

**Rationale**: Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: [FIX] §18d2 eq-exactness fixture lost: bash 341-346 seeded `https://example.com/page` so a prefix/LIKE `eq` would fail; the "ALREADY COVERED" test seeds two identical `https://example.com` plus `https://other.org`, so a prefix match still passes — tests/e2e/serve.rs:1830-1857 (`search_where_filter_materialized_column_eq`) — change Link B's url to `https://example.com/page` and expect 1 hit / totalCount 1.

**Choice**: auto-fix

**Rationale**: Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: [FIX] §38b2b raw-scalar pin with multiple junction rows lost: bash 1260-1265 linked three categories and asserted `rlcat_id == <first-linked>`; the cited test links one — tests/e2e/reference_raw_id.rs:112-143 — add a three-category variant asserting `category_id` equals the first-linked id.

**Choice**: auto-fix

**Rationale**: Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: [FIX] §17.J2 numeric-id assertion weakened: bash 217 required `"message":"[0-9]+"`, Rust asserts `!config_id.is_empty()` — tests/e2e/integration_graphql_jink.rs:158 — assert a 14-digit numeric id.

**Choice**: auto-fix

**Rationale**: Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: PRD checklist unchecked and file still in wip/ despite verifiably complete, CI-green implementation

**Choice**: discard

**Rationale**: PRD is intentionally wip until review converges and a fresh finalize session accepts it.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: [FIX] Slop: `stdout()` copy-pasted in smoke_core.rs:6, smoke_sql.rs:4, smoke_search.rs:4; the 14-digit-id check re-implemented as `assert_doogat_id` (smoke_sql.rs:12), `is_14_digit_id` (integration_id_poison.rs:40) and inline at integration_nosql_cli.rs:40,88, integration_singleton_parity.rs:61, integration_singleton_races.rs:87,111 — tests/e2e/common.rs — move one `stdout()` and one `assert_doogat_id()` there and call them (also closes the §17.J2 item).

**Choice**: auto-fix

**Rationale**: Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: [VERIFY] Tier 2 has not run on the deletion commit: the green manual dispatch (run 33991344845, 2026-09-05 20:51-21:48, `565 passed; 0 failed; 0 ignored` on linux-e2e and windows-full) is on 5cf4c36, one commit before c8b72bd; only test.yml (run 34016441899, 21 min, green) has run on c8b72bd — after the next nightly run `gh run list --repo doogat/ddb --workflow=full-validation.yml --limit 1 --json headSha,conclusion,createdAt,updatedAt`; it must show headSha c8b72bd or later, conclusion success, a real duration (≥45 min), and `gh run view --job <linux-e2e id> --log` must contain `test result: ok. 565 passed; 0 failed; 0 ignored`.

**Choice**: discard

**Rationale**: Live GitHub confirmed the contract-required full validation before deletion and Test after deletion. A fixed duration and old exact test count are not validity gates. Rework will be pushed for fresh CI. Not queued: command shape.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: [KNOWN] §54.D/§55.D psql-absent skip semantic not ported — moot: the PgWire legs use tokio-postgres (`pgwire_singleton`, `integration_55_d_cli_vs_pgwire_singleton_insert_race`), so no client-absent path exists to degrade; recorded so nobody re-adds a skip.

**Choice**: discard

**Rationale**: Rust PgWire client is a declared dependency; every platform runs the assertions, so no missing-client branch remains.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: [KNOWN] The mapping table cites three reserved slugs that resolve to no test (`integration_49_structured_error_propagation`, `integration_50_alter_table_rename`, `integration_50_cross_process_fk_freshness`; mapping.md:230,235,340,465,538) — each row documents the slug as intentionally unused for a banner-only section whose assertions map to sub-rows; every other cited target (179 qualified, 112 bare) resolves against `cargo test -p ddb-e2e -- --list` (565 tests).

**Choice**: discard

**Rationale**: Mapping explicitly labels these as non-executable banner sections and maps their executable subrows to registered tests.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: Blake re-raised four cycle-1 settled decisions (PRD still in wip/ with unchecked boxes; §54.D psql-absent skip not ported; no #[serial] markers) plus Bob raised three of the same settled class (serve.rs 2011 lines; rest.rs either-or hedge outside the diff)

**Choice**: discarded or settled-deferral per the ledger

**Rationale**: Cycle 2 decision gate: --ledger-dismiss BLAKE matched none of them because his wording changed, so they were dismissed by gate judgment on issue text plus file and appended to 00171-migrate-shell-scenarios-to-e2e-v1-ledger.json with reasons. Blake is blind to the ledger by construction.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: Bob: obsolete DDB_BIN env survives on the linux-e2e step of full-validation.yml; and Bob VERIFY: 14-digit-id consolidation completeness unverified outside the touched modules

**Choice**: discarded - contradicts computed facts / answered by gate-run evidence

**Rationale**: Cycle 2 decision gate ran both checks itself. rg -n DDB_BIN over the repo excluding dev/local returns zero hits, so the DDB_BIN claim is refuted. rg -n over tests/e2e for the 14-digit shapes shows survivors only in modules PRD 00171 never touched, matching Eve KNOWN 2 and the surgical-change rule; every 00171-authored site named in cycle 1 is consolidated. Bob had no grep in his sandbox and said so.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: DDB_BIN cycle-2 dismissal superseded after hidden-directory search correction; Eve input isolation repaired in a fresh native Fable child

**Choice**: restored finding; review input corrected

**Rationale**: Direct rg of .github/workflows/full-validation.yml finds DDB_BIN at line 61. The old search omitted hidden directories. False discard removed from active ledger, full old ledger archived. Corrected Eve raw response and re-consolidation retained; original findings preserved. No code edits, acceptance or state resume; maintainer decision pending.

### [autonomous] 2026-09-07T16:30:13Z

**Decision**: Explicit maintainer acceptance of all 11 remaining 00171 findings and waiver of the remaining tail sweep

**Choice**: Accept 00171 at 876a153 with deferred exceptions; run fresh finalization and stop before 00172

**Rationale**: I accept 00171 at `876a153` with the remaining assertion gaps, cleanup/documentation findings, and unreproduced macOS flake explicitly deferred. I waive the remaining tail sweep for this PRD. Record those acceptance exceptions, run the fresh finalize gate, mark 00171 done, and stop. No further implementation or review cycles. Explicit reply: yes, that is what I want to do, proceed

### [deferred] 2026-09-07T16:30:13Z

**Decision**: Multi-process race tests not marked #[serial] as PRD explicitly requires (mitigated by per-test port/tempdir isolation)

**Choice**: deferred

**Rationale**: Existing reviewed design deliberately uses independent TempDirs/ports and order-independent race results instead of serial markers. Retain parallel execution; record the stale PRD wording.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: Several newly-ported multi-process race test functions run well past the 50-line guideline (integration_61_b_concurrent_cli_and_graphql_write_lock ≈145 lines, integration_55_d_cli_vs_pgwire_singleton_insert_race ≈152 lines)

**Choice**: deferred

**Rationale**: Keep cohesive multi-process scenario orchestration and HEAD durability checks; shared operations are already helpers. The project Rust instructions have no hard function-size limit, and broad test fragmentation is outside this coverage repair.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: The mechanical fail-first/tautology finder for this cycle reports "Checked 0 test function(s)" and "replay: skipped (the diff touches no test function)" despite ~60 new/changed e2e test files — the automated tautology net did not fire for this diff

**Choice**: deferred

**Rationale**: Installed mechanical checks are explicitly Python-only. Extending shared tooling is outside 00171; this cycle used Rust source audits and same-HEAD runtime evidence.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: [KNOWN] PRD says the §55/§61 race sections are "marked serial"; no serial marker exists (no serial_test, no shared mutex) — design doc lines 92-100 and 289 supersede this with per-test tempdir + unique-port isolation, and every race outcome is order-independent (SINGLETON forces exactly one winner; 61.B requires every write to land), so a serial marker would only slow the suite.

**Choice**: deferred

**Rationale**: Existing reviewed design deliberately uses independent TempDirs/ports and order-independent race results instead of serial markers. Retain parallel execution; record the stale PRD wording.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: [KNOWN] PRD spawn helper "polls /health"; `ServerGuard::start` (tests/e2e/common.rs:81-102) waits on the stderr "listening on" lines plus a 50 ms sleep, no /health poll — design lines 84-89 reused the existing guard unchanged, the health round trip is pinned by `serve::health_returns_ok`, and the internals are 00188's to change.

**Choice**: deferred

**Rationale**: Design deliberately reuses existing ServerGuard readiness; health_returns_ok tests its lifecycle. Internals are reserved for 00188, outside this test migration.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: Nightly macOS ConnectionReset in a migrated e2e test, cause unconfirmed. integration_warnings_update::integration_58_a_graphql_update_warnings_empty failed once at c8b72bd in Nightly 34020494801 at tests/e2e/common.rs:186. At the reviewed SHA it passed on all three platforms with no code change, no retry, no skip. Eve traced the readiness path and found it sound for HTTP; cause remains unconfirmed. Alice established that task 15 moved the full e2e suite from nightly-only to every push, so this flake exposure increased as a direct result of this PRD rework, with no resilience added. Severity contested: Blake (blind) 1/4 rates it high; Alice, Bob and Eve rate it low with PRD 00188 named as the durable home.

**Choice**: deferred

**Rationale**: Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: Section 38b2b link order does not exclude a MAX(id) resolver. reference_raw_id.rs:164 mints alpha, beta, gamma ascending so category_ids[2] is the largest id; :195 links [2],[0],[1] and :215 asserts category_ids[2]. Confirmed by reading. Fix: link the middle id first and assert category_ids[1], which excludes min-id, max-id and last-linked at once.

**Choice**: deferred

**Rationale**: Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: Section 38b2b object resolver unpinned on multiple junction rows. The deleted bash asserted BOTH the scalar and the object resolver on three junction rows; the new test queries only category_id, so an object resolver drifting from the scalar row is unpinned. Bears on the PRD No lost semantics success metric.

**Choice**: deferred

**Rationale**: Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: The new 73-line test duplicates the seed loop at reference_raw_id.rs:320-338 verbatim apart from table and variable names, and exceeds the 50-line guideline. Confirmed by reading. One seed_labeled_rows helper closes both.

**Choice**: deferred

**Rationale**: Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: Section 18d2 pins the hit count but never which row survived. serve.rs:1854-1860 asserts hits.len()==1 and totalCount==1 and never reads hits[0][title], so an eq returning the wrong single row passes. Confirmed by reading.

**Choice**: deferred

**Rationale**: Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: common::stdout is a noun name for a newly crate-wide action helper that runs ddb and asserts success; it reads as std::io::stdout or the assert_cmd stdout assertion. Rename to run_stdout or capture_stdout and update the three importers.

**Choice**: deferred

**Rationale**: Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: Redundant predicate in integration_nosql_cli.rs: the contains predicate is strictly implied by the new line-equality check on the same captured output, and the clone exists only to feed it.

**Choice**: deferred

**Rationale**: Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: Unused DDB_BIN remains at .github/workflows/full-validation.yml:61; the initial zero-hit search skipped hidden directories. The cycle-2 discard is superseded; restore this low-severity cleanup.

**Choice**: deferred

**Rationale**: Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: Corrected Eve: strengthen the two sibling section 18z8 nonempty-ID assertions with the existing assert_doogat_id helper.

**Choice**: deferred

**Rationale**: Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: Corrected Eve: assess redundant seeding sleeps; preserve the known repo-aware ID throughput ceiling. Claimed three-second improvement is unverified; no minting-code change is authorized.

**Choice**: deferred

**Rationale**: Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md.

### [deferred] 2026-09-07T16:30:13Z

**Decision**: Corrected Eve first-reference-order documentation check ran. data-model.md and indexer.md lack the explicit rule; server.md does not state multi-reference singular scalar/object selection order. Clarify existing behavior without changing policy.

**Choice**: deferred

**Rationale**: Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md.
