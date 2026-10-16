---
prd: dev/local/prds/wip/00171-migrate-shell-scenarios-to-e2e-v1.md
review: 1
date: 2026-09-06
head_sha: c8b72bd9126c83d054264ce6ac3738e8f6d0abda
reviewers: alice,blake,bob,eve
agents:
  alice: available
  blake: available
  bob: available (Claude fallback)
  carl: unavailable (nested dispatch guard, exit 3 twice)
  eve: available
---

# Review: 00171-migrate-shell-scenarios-to-e2e-v1

Diff range: `b1e785395fb1e5d7043fbc725cc5786f62a6417e..c8b72bd9126c83d054264ce6ac3738e8f6d0abda`

codex_rung_guard: fired (4 codex-implemented task(s))

All required lenses ran in isolated sessions. This Codex host has no native Claude Agent tool: Alice and Blake ran through the Sonnet CLI adapter; Bob's Codex runner refused nested dispatch (exit 3), so the exact assembled doubt/de-slop prompt ran on Claude Sonnet. Eve ran Claude Fable (`claude-fable-5-1`, session `72d89b18-fa57-415d-b718-7405d065cc38`). Carl's optional Gemini runner refused nested dispatch on both permitted attempts; no Carl output is counted. No permission bypass was used. The installed rubric calls the five doubt rules D1–D5 (the handoff called them R1–R5).

Context pack: first attempt lacked a staged diff; retry was sandbox-blocked on localhost; the escalated retry succeeded after reviewer dispatch. Prompts used the documented no-pack sentinel, so that later pack is not claimed as reviewer input. Python-only mechanical checks were run and explicitly skipped Rust test discovery/replay; no automated Rust tautology coverage is claimed. The normal gather script chose master and yielded an empty diff, corrected to the preserved full work range before any reviewer dispatch.

Parent verification: all 391 original file:line source keys match the mapping, zero duplicates/missing/extra rows; 565 tests registered; 63 existing qualified smoke targets are absent from the push filters. The built runner confirms the current filters select 35 tests. Evidence is in `00171-review-1-parent-evidence.json` and `00171-review-1-selection-proof.txt`. GitHub live reads confirmed Full Validation 33991344845 (all six jobs) green at 5cf4c36 before deletion, and Test 34016441899 green at c8b72bd after deletion.

## Consolidated findings

Eve's buckets were converted to issue lines for the installed consolidator; raw bucket text is retained verbatim below and in the durable raw files. No bucket finding is silently dropped. Severity is parent-classified: per-push coverage HIGH; six fidelity gaps MEDIUM; helper duplication and known/verification observations LOW.

| Consensus | Severity | Issue | File | Task | Found By |
|-----------|----------|-------|------|------|----------|
| [3/4] | ⚪ | Review diff/commit range for PRD 00171 bundles unrelated PRD 00202 HLC-logic commits (git_ops/read.rs, tests.rs) — a process/traceability issue, not a defect in 00171's own work | ddb-core/src/git_ops/read.rs | general | ALICE, BOB, EVE |
| [1/4] | 🟠 | [FIX] Per-push CI coverage dropped: `test.yml`'s new `Smoke test (e2e)` step runs only the 30 `smoke_*` tests (of 565), yet ~26 of smoke.sh's 56 sections were mapped to pre-existing modules outside that filter (delete_backlinks, tags, checkboxes, folder_namespace, search_links, link_kinds, rename, sql_lifecycle, alter_table_rename, cli_create_fk_freshness, inline_zone, discover, fix, sequence, help, cascade_delete, sql_expressions_test, set_flag, date_default, alter_column_type, schema_apply, poison_file_reindex, cli_create_unregistered_type, cli_update_app_error, sync_crdt); smoke.sh ran those on every push, now they run nightly only, contradicting the PRD's "CI keeps identical coverage" — .github/workflows/test.yml:45-48 — replace the `smoke_` step and the five `--exact` steps with one `cargo test -p ddb-e2e` step (the full suite took 223 s in the 5cf4c36 linux-e2e job), drop the `DDB_BIN` env (nothing under tests/ reads it), and fix the test.yml sentence at AGENTS.md:116. | .github/workflows/test.yml | general | EVE |
| [1/4] | 🟡 | Multi-process race tests not marked #[serial] as PRD explicitly requires (mitigated by per-test port/tempdir isolation) | tests/e2e/integration_singleton_races.rs | 5 | BLAKE |
| [1/4] | 🟡 | [FIX] §42 totalCount assertions dropped: bash 1505/1517 asserted `totalCount == 1`; the cited "ALREADY COVERED" tests request the field but never read it — tests/e2e/serve.rs:1408-1410 (`filter_base_field_title_eq`) and :1443-1445 (`filter_base_field_title_contains`) — add `assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 1)` to each. | tests/e2e/serve.rs | general | EVE |
| [1/4] | 🟡 | [FIX] §44.E1 JOIN pin degraded to single-character containment: `row_str.contains('a')`/`contains('1')` also pass on `["abc","10"]`; bash 1699-1700 grepped the quoted tokens — tests/e2e/integration_junction_sync.rs:35-43 — parse `rows[0]` as JSON and `assert_eq!(row, json!(["a","1"]))`. | tests/e2e/integration_junction_sync.rs | general | EVE |
| [1/4] | 🟡 | [FIX] §59.A cross-process read-back dropped: bash 2667-2677 read the typed column back through a separate `ddb query` process (its comment names that as the point); the Rust target reads through the same server's executeSql — tests/e2e/rest.rs:153-154 — add `repo.ddb().args(["query", "SELECT category FROM item WHERE id = '<id>'"]).assert().success().stdout(contains("books"))`. | tests/e2e/rest.rs | general | EVE |
| [1/4] | 🟡 | [FIX] §18d2 eq-exactness fixture lost: bash 341-346 seeded `https://example.com/page` so a prefix/LIKE `eq` would fail; the "ALREADY COVERED" test seeds two identical `https://example.com` plus `https://other.org`, so a prefix match still passes — tests/e2e/serve.rs:1830-1857 (`search_where_filter_materialized_column_eq`) — change Link B's url to `https://example.com/page` and expect 1 hit / totalCount 1. | tests/e2e/serve.rs | general | EVE |
| [1/4] | 🟡 | [FIX] §38b2b raw-scalar pin with multiple junction rows lost: bash 1260-1265 linked three categories and asserted `rlcat_id == <first-linked>`; the cited test links one — tests/e2e/reference_raw_id.rs:112-143 — add a three-category variant asserting `category_id` equals the first-linked id. | tests/e2e/reference_raw_id.rs | general | EVE |
| [1/4] | 🟡 | [FIX] §17.J2 numeric-id assertion weakened: bash 217 required `"message":"[0-9]+"`, Rust asserts `!config_id.is_empty()` — tests/e2e/integration_graphql_jink.rs:158 — assert a 14-digit numeric id. | tests/e2e/integration_graphql_jink.rs | general | EVE |
| [1/4] | ⚪ | PRD checklist unchecked and file still in wip/ despite verifiably complete, CI-green implementation | dev/local/prds/wip/00171-migrate-shell-scenarios-to-e2e-v1.md | general | BLAKE |
| [1/4] | ⚪ | Several newly-ported multi-process race test functions run well past the 50-line guideline (integration_61_b_concurrent_cli_and_graphql_write_lock ≈145 lines, integration_55_d_cli_vs_pgwire_singleton_insert_race ≈152 lines) | tests/e2e/integration_write_lock_races.rs | general | BOB |
| [1/4] | ⚪ | The mechanical fail-first/tautology finder for this cycle reports "Checked 0 test function(s)" and "replay: skipped (the diff touches no test function)" despite ~60 new/changed e2e test files — the automated tautology net did not fire for this diff | N/A | general | BOB |
| [1/4] | ⚪ | [FIX] Slop: `stdout()` copy-pasted in smoke_core.rs:6, smoke_sql.rs:4, smoke_search.rs:4; the 14-digit-id check re-implemented as `assert_doogat_id` (smoke_sql.rs:12), `is_14_digit_id` (integration_id_poison.rs:40) and inline at integration_nosql_cli.rs:40,88, integration_singleton_parity.rs:61, integration_singleton_races.rs:87,111 — tests/e2e/common.rs — move one `stdout()` and one `assert_doogat_id()` there and call them (also closes the §17.J2 item). | tests/e2e/common.rs | general | EVE |
| [1/4] | ⚪ | [VERIFY] Tier 2 has not run on the deletion commit: the green manual dispatch (run 33991344845, 2026-09-05 20:51-21:48, `565 passed; 0 failed; 0 ignored` on linux-e2e and windows-full) is on 5cf4c36, one commit before c8b72bd; only test.yml (run 34016441899, 21 min, green) has run on c8b72bd — after the next nightly run `gh run list --repo doogat/ddb --workflow=full-validation.yml --limit 1 --json headSha,conclusion,createdAt,updatedAt`; it must show headSha c8b72bd or later, conclusion success, a real duration (≥45 min), and `gh run view --job <linux-e2e id> --log` must contain `test result: ok. 565 passed; 0 failed; 0 ignored`. | N/A | general | EVE |
| [1/4] | ⚪ | [KNOWN] PRD says the §55/§61 race sections are "marked serial"; no serial marker exists (no serial_test, no shared mutex) — design doc lines 92-100 and 289 supersede this with per-test tempdir + unique-port isolation, and every race outcome is order-independent (SINGLETON forces exactly one winner; 61.B requires every write to land), so a serial marker would only slow the suite. | tests/e2e/integration_singleton_races.rs | general | EVE |
| [1/4] | ⚪ | [KNOWN] PRD spawn helper "polls /health"; `ServerGuard::start` (tests/e2e/common.rs:81-102) waits on the stderr "listening on" lines plus a 50 ms sleep, no /health poll — design lines 84-89 reused the existing guard unchanged, the health round trip is pinned by `serve::health_returns_ok`, and the internals are 00188's to change. | tests/e2e/common.rs | general | EVE |
| [1/4] | ⚪ | [KNOWN] §54.D/§55.D psql-absent skip semantic not ported — moot: the PgWire legs use tokio-postgres (`pgwire_singleton`, `integration_55_d_cli_vs_pgwire_singleton_insert_race`), so no client-absent path exists to degrade; recorded so nobody re-adds a skip. | tests/e2e/pgwire_singleton.rs | general | EVE |
| [1/4] | ⚪ | [KNOWN] The mapping table cites three reserved slugs that resolve to no test (`integration_49_structured_error_propagation`, `integration_50_alter_table_rename`, `integration_50_cross_process_fk_freshness`; mapping.md:230,235,340,465,538) — each row documents the slug as intentionally unused for a banner-only section whose assertions map to sub-rows; every other cited target (179 qualified, 112 bare) resolves against `cargo test -p ddb-e2e -- --list` (565 tests). | dev/local/designs/00171-migrate-shell-scenarios-to-e2e-v1-mapping.md | general | EVE |


## Decision gate

One unresolved HIGH requires rework, cycle 1 of cap 2. Eight actionable FIX findings become two bounded D1 tasks: restore CI selection, then restore six assertion gaps and consolidate their duplicated helpers. Both are additive coverage repairs with no public product contract change. No scope alarm (two follow-up tasks).

The five known Eve observations remain visible. Serial and psql-skip wording is settled by the existing design's isolated per-test repos/ports and always-available Rust PgWire client; helper readiness was deliberately reused and its health round trip is tested. The three reserved mapping slugs are explicitly banner-only and point at executable subrows. The interleaved 00202 commits are independently identified by git history and are not 00171 scope.

Blake's unchecked PRD/wip observation is discarded: acceptance/finalize has intentionally not occurred; prematurely moving it would violate the fresh finalize-session gate. Bob's zero mechanical Rust coverage is a real tool limitation (the installed script only accepts .py); this review used static Rust inspection and recorded runtime evidence. Bob's long race scenarios are retained as a documented quality tradeoff: phases operate on the same process handles and durable HEAD assertions, existing shared operations are extracted, and further fragmentation is not required by project Rust instructions. No full-function refactor is commissioned from that LOW observation.

Eve's VERIFY item is not queued: it is a future GitHub observation with a placeholder job id, a compound check, and an unnecessary fixed-duration/test-count threshold, not an eligible project verification command. The required before-deletion all-platform run and after-deletion push run are independently verified green. New test rework will be pushed for fresh CI, whose real result is authoritative; no minimum duration is treated as correctness evidence.

## Alice

[ALICE] ⚪ Review diff/commit range for PRD 00171 bundles unrelated PRD 00202 HLC-logic commits (git_ops/read.rs, tests.rs) — a process/traceability issue, not a defect in 00171's own work | File: ddb-core/src/git_ops/read.rs | Task: general
[ALICE] ✅ No issues found in PRD 00171's own scope: CI workflow diffs (test.yml, full-validation.yml) match the design doc's contract byte-for-byte; both task 13 and task 14 CI-green evidence JSONs check out against their claimed SHAs and job names; the mapping table (585 lines, 4 tables) was built and independently reconciled/audited at task 12 with separate audit reports; AGENTS.md/CONTRIBUTING.md/README.md/docs rewrites correctly drop the 4-file shell mandate while preserving the unrelated jink-feedback sentence; `rg` finds zero live references to the deleted shell files; spot-checked race tests (write-lock, SINGLETON insert races) assert against committed `HEAD` state with structured, non-tautological checks; new e2e test files are all under the 800-line file cap; `tests/e2e/main.rs` module list was inserted alphabetically; no TODO/debug/`#[ignore]` markers found in the new test files; incidental core-code comment updates (search_query.rs, indexer/tests/mod.rs) correctly repoint stale shell-section references to the new Rust test names rather than being scope creep.

R1: pass
R2: pass
R3: pass
R4: pass
R6: pass
R7: pass
R8: pass
R9: pass
R10: pass
R11: pass
R12: pass
R13: pass


## Blake

```
[BLAKE] 🟡 Multi-process race tests not marked #[serial] as PRD explicitly requires (mitigated by per-test port/tempdir isolation) | File: tests/e2e/integration_singleton_races.rs | Task: 5
[BLAKE] ⚪ PRD checklist unchecked and file still in wip/ despite verifiably complete, CI-green implementation | File: dev/local/prds/wip/00171-migrate-shell-scenarios-to-e2e-v1.md | Task: general
```

B1: pass
B2: pass
B3: pass
B4: pass
B5: pass
B6: pass
B7: pass
B8: pass
B9: pass
B10: pass
B11: pass
B12: pass
B13: pass
B14: pass
B15: pass
B16: pass
B17: fail
B18: pass
B19: pass


## Bob

```
[BOB] ⚪ Diff bundles already-landed PRD 00202 commits (git_ops/read.rs merge-attribution rewrite, its new tests, and the matching CHANGELOG entry) that are out of scope for 00171's shell-migration mandate — those commits (b1a047f, a7e5179, 75cde1c) are already on master per `git log`, so this is almost certainly a diff-base artifact (c1's base predates them), not new work done under 00171 | File: ddb-core/src/git_ops/read.rs | Task: general
[BOB] ⚪ Several newly-ported multi-process race test functions run well past the 50-line guideline (integration_61_b_concurrent_cli_and_graphql_write_lock ≈145 lines, integration_55_d_cli_vs_pgwire_singleton_insert_race ≈152 lines) | File: tests/e2e/integration_write_lock_races.rs | Task: general
[BOB] ⚪ The mechanical fail-first/tautology finder for this cycle reports "Checked 0 test function(s)" and "replay: skipped (the diff touches no test function)" despite ~60 new/changed e2e test files — the automated tautology net did not fire for this diff | File: N/A | Task: general

R1: pass
R2: pass
R3: pass
R4: pass
R6: pass
R7: pass
R8: pass
R9: pass
R10: pass
R11: pass
R12: fail
R13: pass

FIX:
- (none)

VERIFY:
- git_ops/read.rs + tests.rs + CHANGELOG 00202 content appearing in this diff — run `git log --oneline -1 <c1-base-sha> -- ddb-core/src/git_ops/read.rs` and confirm it already contains 75cde1c; if it does, this is pure diff-base noise from cutting the review diff against a stale base, not scope creep by this cycle's implementor.
- Mech-check tool reporting zero touched test functions for a diff that adds/changes ~60 e2e test files — check the mech-check finder's file-glob/path filter to see why `tests/e2e/**` was excluded from this cycle's scan, since future review cycles on this crate need the automated tautology/fail-first net working rather than relying on manual sampling.

KNOWN:
- Race-scenario test functions exceeding 50 lines (R12) — out of scope to split: these are atomic multi-process race scenarios (spawn every writer, then assert convergence + HEAD durability as one indivisible check), the same shape as the pre-existing `singleton_cross_process_upsert.rs` tests this PRD explicitly cross-references, and the diff already extracts the reusable pieces into helpers (`race_graphql`, `graphql_winner`, `assert_survivor`, `spawn_ddb`, `wait_ok`, `head_content`) — further splitting would fragment one scenario into pieces that still have to run together.

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass
```

Scope note: this diff is large (84 files) with an extensive pre-existing per-file Pat review trail in `dev/local/reviews/00171-task14-pat-*`. I read the mapping table, both CI workflows in full, `common.rs`/`main.rs` wiring, all four deleted shell files' headers (confirmed clean full deletions), and did deep spot-checks on the highest-risk ported scenarios (SINGLETON races, CLI-vs-PgWire race, cross-process write-lock race) rather than reading all ~60 e2e files line-by-line — those sampled files show correct, non-tautological, HEAD-durability-based assertions.


## Eve

FIX:
- Per-push CI coverage dropped: `test.yml`'s new `Smoke test (e2e)` step runs only the 30 `smoke_*` tests (of 565), yet ~26 of smoke.sh's 56 sections were mapped to pre-existing modules outside that filter (delete_backlinks, tags, checkboxes, folder_namespace, search_links, link_kinds, rename, sql_lifecycle, alter_table_rename, cli_create_fk_freshness, inline_zone, discover, fix, sequence, help, cascade_delete, sql_expressions_test, set_flag, date_default, alter_column_type, schema_apply, poison_file_reindex, cli_create_unregistered_type, cli_update_app_error, sync_crdt); smoke.sh ran those on every push, now they run nightly only, contradicting the PRD's "CI keeps identical coverage" — .github/workflows/test.yml:45-48 — replace the `smoke_` step and the five `--exact` steps with one `cargo test -p ddb-e2e` step (the full suite took 223 s in the 5cf4c36 linux-e2e job), drop the `DDB_BIN` env (nothing under tests/ reads it), and fix the test.yml sentence at AGENTS.md:116.
- §42 totalCount assertions dropped: bash 1505/1517 asserted `totalCount == 1`; the cited "ALREADY COVERED" tests request the field but never read it — tests/e2e/serve.rs:1408-1410 (`filter_base_field_title_eq`) and :1443-1445 (`filter_base_field_title_contains`) — add `assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 1)` to each.
- §44.E1 JOIN pin degraded to single-character containment: `row_str.contains('a')`/`contains('1')` also pass on `["abc","10"]`; bash 1699-1700 grepped the quoted tokens — tests/e2e/integration_junction_sync.rs:35-43 — parse `rows[0]` as JSON and `assert_eq!(row, json!(["a","1"]))`.
- §59.A cross-process read-back dropped: bash 2667-2677 read the typed column back through a separate `ddb query` process (its comment names that as the point); the Rust target reads through the same server's executeSql — tests/e2e/rest.rs:153-154 — add `repo.ddb().args(["query", "SELECT category FROM item WHERE id = '<id>'"]).assert().success().stdout(contains("books"))`.
- §18d2 eq-exactness fixture lost: bash 341-346 seeded `https://example.com/page` so a prefix/LIKE `eq` would fail; the "ALREADY COVERED" test seeds two identical `https://example.com` plus `https://other.org`, so a prefix match still passes — tests/e2e/serve.rs:1830-1857 (`search_where_filter_materialized_column_eq`) — change Link B's url to `https://example.com/page` and expect 1 hit / totalCount 1.
- §38b2b raw-scalar pin with multiple junction rows lost: bash 1260-1265 linked three categories and asserted `rlcat_id == <first-linked>`; the cited test links one — tests/e2e/reference_raw_id.rs:112-143 — add a three-category variant asserting `category_id` equals the first-linked id.
- §17.J2 numeric-id assertion weakened: bash 217 required `"message":"[0-9]+"`, Rust asserts `!config_id.is_empty()` — tests/e2e/integration_graphql_jink.rs:158 — assert a 14-digit numeric id.
- Slop: `stdout()` copy-pasted in smoke_core.rs:6, smoke_sql.rs:4, smoke_search.rs:4; the 14-digit-id check re-implemented as `assert_doogat_id` (smoke_sql.rs:12), `is_14_digit_id` (integration_id_poison.rs:40) and inline at integration_nosql_cli.rs:40,88, integration_singleton_parity.rs:61, integration_singleton_races.rs:87,111 — tests/e2e/common.rs — move one `stdout()` and one `assert_doogat_id()` there and call them (also closes the §17.J2 item).

VERIFY:
- Tier 2 has not run on the deletion commit: the green manual dispatch (run 33991344845, 2026-09-05 20:51-21:48, `565 passed; 0 failed; 0 ignored` on linux-e2e and windows-full) is on 5cf4c36, one commit before c8b72bd; only test.yml (run 34016441899, 21 min, green) has run on c8b72bd — after the next nightly run `gh run list --repo doogat/ddb --workflow=full-validation.yml --limit 1 --json headSha,conclusion,createdAt,updatedAt`; it must show headSha c8b72bd or later, conclusion success, a real duration (≥45 min), and `gh run view --job <linux-e2e id> --log` must contain `test result: ok. 565 passed; 0 failed; 0 ignored`.

KNOWN:
- PRD says the §55/§61 race sections are "marked serial"; no serial marker exists (no serial_test, no shared mutex) — design doc lines 92-100 and 289 supersede this with per-test tempdir + unique-port isolation, and every race outcome is order-independent (SINGLETON forces exactly one winner; 61.B requires every write to land), so a serial marker would only slow the suite.
- PRD spawn helper "polls /health"; `ServerGuard::start` (tests/e2e/common.rs:81-102) waits on the stderr "listening on" lines plus a 50 ms sleep, no /health poll — design lines 84-89 reused the existing guard unchanged, the health round trip is pinned by `serve::health_returns_ok`, and the internals are 00188's to change.
- §54.D/§55.D psql-absent skip semantic not ported — moot: the PgWire legs use tokio-postgres (`pgwire_singleton`, `integration_55_d_cli_vs_pgwire_singleton_insert_race`), so no client-absent path exists to degrade; recorded so nobody re-adds a skip.
- The mapping table cites three reserved slugs that resolve to no test (`integration_49_structured_error_propagation`, `integration_50_alter_table_rename`, `integration_50_cross_process_fk_freshness`; mapping.md:230,235,340,465,538) — each row documents the slug as intentionally unused for a banner-only section whose assertions map to sub-rows; every other cited target (179 qualified, 112 bare) resolves against `cargo test -p ddb-e2e -- --list` (565 tests).
- The diff range carries PRD 00202's commits (ddb-core/src/git_ops/read.rs, git_ops/tests.rs, the CHANGELOG `sync` entry, crdt-resolver.md) — not this PRD's work; landed attended 2026-09-05 and reviewed there.

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

**Status**: Done.
**Changed**: nothing (review only). Evidence: mapping cross-check 565 registered tests, 0 unresolved qualified targets; agent B audit of 40 sync/CRDT/typed-write sections clean; agent A audit of 70 GraphQL/REST sections surfaced the 6 fidelity gaps above, each re-verified against bash source and Rust target.
```
registered tests: 565 | qualified refs: 179 missing: 0 | smoke_ filter selects: 30
linux-e2e@5cf4c36: 565 passed; 0 failed; 0 ignored (223.56s) | windows-full: 565 passed (438.14s)
```
**Next**: Task complete. Suggested next: apply FIX 1 (full e2e in test.yml) first, since it restores the per-push coverage the other seven fixes land in.


Verdict: 18 findings
Tests: 2384 passed, 0 failed, 0 skipped (reused from last-verification.json at c8b72bd9126c83d054264ce6ac3738e8f6d0abda)
