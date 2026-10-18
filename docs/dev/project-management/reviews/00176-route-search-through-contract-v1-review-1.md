---
prd: docs/dev/project-management/prds/wip/00176-route-search-through-contract-v1.md
review: 1
date: 2026-10-18
head_sha: 0fdd88ef31ca6e6a57690fa5db096c2cc63e04e1
codex_thread_id: 01a14ceb-44c3-72e0-aa00-f6a9b5be9d44
reviewers: alice,blake,bob,carl
agents:
  alice: available
  blake: available
  bob: available
  carl: available
dispatch_rows:
  bob: 36b7ee5e
  carl: 22b34304
---

# Review: 00176-route-search-through-contract-v1

Diff range: `21c283d10c16a873ed0e044e07703d729bf419fa..0fdd88ef31ca6e6a57690fa5db096c2cc63e04e1`

codex_rung_guard: not fired

## Review Summary

Reviewed: 6 completed tasks
PRDs checked: 00176-route-search-through-contract-v1

### Agent Status

- Alice: ✅ Available (consensus lens, legacy engine)
- Blake: ✅ Available (blind lens, PRD-only)
- Bob: ✅ Available (doubt + de-slop lens, codex) — after one retry (inlined)
- Carl: ✅ Available (gemini via copilot backend, model `gemini-3.8-flash`)

### Run notes (fail loud)

- **Bob's first run was a lack-of-input refusal**: codex's sandbox could not read the staged files (`sandbox-exec: sandbox_apply: Operation not permitted`), so it emitted a single "Cannot statically verify" line and all-`fail` verdicts. The one-retry budget was spent on the inlined retry prompt (`docs/dev/tmp/bob-prompt-20261018-00176-c1-retry.md`, context + full diff inlined, 314 KB); that run produced a usable review, which is what is consolidated here.
- **Bob's findings needed model-side normalization for consolidation**: his doubt-lens buckets emit `- [BOB] …` bullet lines, which `consolidate_findings.py` does not parse. Without normalization his four FIX findings (including a 🟠) were silently dropped from the table. A normalized copy (`bob-output-20261018-00176-c1-normalized.txt`, bullets stripped, text otherwise verbatim) was passed to the consolidator. Raw output is retained unchanged.
- **Citation merge on row R1**: the consolidator merged three distinct findings that cite `ddb-server/src/schema/queries.rs` at different lines (`:148` Alice/Bob — missing GraphQL default-50 / resolver tests; `:97` Blake — the `doogats` list query still has an unbounded default and never decodes through `PageRequest`; `:144` Carl — the 98-line resolver closure). The `[4/4]` consensus is therefore a merge artifact, not four reviewers on one defect. All three are carried into the rework task.
- **Mechanical test checks contributed nothing**: `detect_tautological_tests.py` and the fail-first replay are python-only, so on this Rust diff they report `Checked 0 test function(s)` / `skipped (non-python)`. Tautological-test concerns were raised manually instead (row R3).
- `pack: ok` (engram pack built). `review-stage` errors: none.
- No prior cycle, so no carried-forward verification checks. No VERIFY item named an exact runnable command (Bob's names a Swift/Kotlin binding smoke, not a command), so no `-checks-1.json` queue was written.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [4/4] | 🟠 | GraphQL `search` default-limit change (20 -> 50) and the new SearchCommand/`forward_warnings` branch have no test (Alice/Bob: add a resolver test searching >50 matching doogats with no `limit`, expecting 50; `integration_graphql_server.rs:550` pins only 0/10000/10001/-1). Merge note — this row also carries Blake's finding that the `doogats` list query keeps an unbounded default and never decodes through `PageRequest` (`validate_limit` returns `None` when absent, `limit: 0` accepted, queries.rs:97), and Carl's 98-line search-resolver closure (queries.rs:144). | ddb-server/src/schema/queries.rs:148 | 2 | ALICE, BLAKE, BOB, CARL |
| R2 | [1/4] | 🟠 | REST discards output.warnings, hiding REINDEX_SKIPPED_FILES when search returns an incomplete index. Include structured warnings in SearchResponse and add a malformed-file regression test. The design exempts CLI/FFI warning forwarding, not REST. | ddb-server/src/rest.rs:370 | 3 | BOB |
| R3 | [1/4] | 🟡 | Two CLI unit tests cannot fail if the filters or defaults are ignored. `cli_search_with_type_and_tag_filters_succeeds` and `cli_search_with_omitted_limit_and_offset_succeeds` only call `.expect(..)` on `Ok(())`, so a `search` that dropped `--type`/`--tag` would still pass. The `outln!` output is not captured. Filtering is only proven by the separate e2e `smoke_search_type_and_tag_flags_filter_hits`. Either assert on captured output or delete these two and rely on the e2e test. | ddb-cli/src/commands/query.rs:207 | 2 | ALICE |
| R4 | [1/4] | 🟡 | Simplification: `search_paginated` and `search_filtered` repeat the same `SearchCommand` build, `search_command` call and `PaginatedSearchResult` mapping. `search_paginated(query, limit, offset)` can be `self.search_filtered(query, Some(limit), Some(offset), None, None)`. The decode and filter behaviour stays identical. | ddb-core/src/ffi/driver.rs:101 | 4 | ALICE |
| R5 | [1/4] | 🟡 | Functions over 50 lines were added. `filtered_search_returns_same_hits_and_full_field_set_on_cli_and_graphql` is about 77 lines (tests/e2e/conformance/cross_driver_crud.rs:1). `rest_search_type_filter_returns_only_matching_hits_with_full_shape` is about 75 lines (ddb-server/src/rest/search_tests.rs:1881). `parse_cli_search_hits` is about 55 lines (tests/e2e/conformance/comparator.rs:1). Split out helpers: per-hit CLI block parsing, the seed-success check, and the shape assertions. | tests/e2e/conformance/cross_driver_crud.rs:1 | 5 | ALICE |
| R6 | [1/4] | 🟡 | This diff grows files that are already over the 800-line limit. ddb-core/src/ffi/tests.rs went from about 1668 to 1909 lines (+241 search tests). ddb-server/src/schema/queries.rs is at 909. tests/e2e/conformance/comparator.rs is now 768, close to the limit. Move the new FFI search tests into a sibling module such as ffi/search_tests.rs, as was done for rest/search_tests.rs. | ddb-core/src/ffi/tests.rs:1653 | 4 | ALICE |
| R7 | [1/4] | 🟡 | REST search discards `output.warnings`. The PRD requires transports to surface warnings, and `REINDEX_SKIPPED_FILES` from the freshness reindex is the intended warning. `SearchResponse` has no warnings field, and `handle_search` drops them. CLI and GraphQL (non-`where` branch) do forward them. | ddb-server/src/rest.rs:365 | 2 | BLAKE |
| R8 | [1/4] | 🟡 | FFI `search_paginated` and `search_filtered` drop `output.warnings`. The FFI `PaginatedSearchResult` has no slot for them. The PRD says warnings are surfaced. | ddb-core/src/ffi/driver.rs:117 | 3 | BLAKE |
| R9 | [1/4] | 🟡 | The GraphQL `where` branch bypasses `SearchCommand`. It calls the older `pool.search` and discards the reindex warnings. The PRD allows `where` to stay GraphQL-specialized, but the `SearchCommand` entrypoint is then not universal. | ddb-server/src/schema/queries.rs:208 | 1 | BLAKE |
| R10 | [1/4] | 🟡 | CLI silently replaces offsets above i64::MAX with i64::MAX, violating the required overflow rejection. Replace the saturating conversion with a checked conversion returning Validation. | ddb-cli/src/commands/query.rs:53 | 2 | BOB |
| R11 | [1/4] | 🟡 | parse_cli_search_hits exceeds the 50-line function limit and mixes block traversal with metadata decoding. Extract a helper that decodes one hit block; retain traversal in the existing function. | tests/e2e/conformance/comparator.rs:128 | 5 | BOB |
| R12 | [1/4] | 🟡 | Conformance helper parse_cli_search_hits is 58 lines; extract single hit parser to helper | tests/e2e/conformance/comparator.rs:125 | 6 | CARL |
| R13 | [1/4] | ⚪ | The GraphQL and CLI `limit: 0` / `--limit 0` rejection is a behaviour change that is not noted in the CHANGELOG or in the compatibility notes. GraphQL `check_limit` accepted 0, and the old CLI accepted `--limit 0`. Only the 20 -> 50 default change and the REST/FFI changes are listed. Add a line under Changed for graphql/cli. | CHANGELOG.md:24 | 6 | ALICE |
| R14 | [1/4] | ⚪ | The pagination bound is duplicated. `base_types.rs` keeps its own `MAX_LIMIT` const and `check_limit`, which accepts 0 and rejects only negatives. This is a second copy of the policy, although `PageRequest::decode` re-validates on the search path. | ddb-server/src/schema/base_types.rs:1030 | 1 | BLAKE |
| R15 | [1/4] | ⚪ | The REST hit omits the `fields` key when the hit has no fields (`skip_serializing_if`). FFI and GraphQL always emit it. The REST field set therefore varies per hit, and the PRD asks for one hit shape. | ddb-server/src/rest.rs:142 | 2 | BLAKE |
| R16 | [1/4] | ⚪ | The conformance `StepOp::Search` parity test runs CLI and GraphQL only. REST parity is covered separately in `integration_rest_search_filtered.rs`, not in the harness. The PRD's REST harness driver depends on 00175, so this is an accepted gap. | tests/e2e/conformance/cross_driver_crud.rs:179 | 5 | BLAKE |
| R17 | [1/4] | ⚪ | The unbounded, unfiltered FFI `search(query)` still exists beside the policy-bound methods. It is not a regression, but one uncapped FFI search path remains. | ddb-core/src/ffi/driver.rs:97 | 3 | BLAKE |
| R18 | [1/4] | ⚪ | Required binding regeneration and Swift/Kotlin smoke results are not supplied. Regenerate both bindings and compile/run calls to search_filtered that read all ten SearchResult fields. Rust build/clippy results do not establish this acceptance criterion. | ddb-core/src/ffi/records.rs:132 | 4 | BOB |
| R19 | [1/4] | ⚪ | Modified queries.rs exceeds 800-line file limit (909 lines) | ddb-server/src/schema/queries.rs:1 | 4 | CARL |
| R20 | [1/4] | ⚪ | Modified main.rs exceeds 800-line file limit (836 lines) | ddb-cli/src/main.rs:1 | 5 | CARL |
| R21 | [1/4] | ⚪ | Parallel cargo test-ci times out on shared_actor_preserves_schema_applys_owned_transaction under thread contention | ddb-server/src/actor/tests/mod.rs:14 | general | CARL |

## Alice

Consensus lens, implementation-aware (context + full diff + engram pack). Seven findings: five 🟡 (untested GraphQL default-50 / SearchCommand branch, two tautology-shaped CLI unit tests, FFI `search_paginated`/`search_filtered` duplication, >50-line test functions, growth of already-oversized files) and two ⚪ (undocumented `limit: 0` rejection, double validation in the GraphQL resolver). She re-ran targeted suites herself: `cargo clippy -p ddb-core -p ddb-server -p ddb-cli -p ddb-e2e --all-targets` clean, and the search-related unit/e2e tests across the four crates passing (62 tests). She also reported that a full `cargo clippy --workspace --all-targets` exited 255 with no compiler output in her environment — unexplained, and not reproduced by the per-crate runs.

Rubric verdicts: R1 fail, R2 fail, R3 pass, R4 pass, R6 pass, R7 pass, R8 pass, R9 pass, R10 pass, R11 pass, R12 fail, R13 fail.

## Blake

Blind lens, PRD-only prompt (no diff, no file list, no history); located the code himself. One 🟠 — the `doogats` list query still carries an unbounded default and never decodes through `PageRequest`, while the PRD's own Risks section says that default becomes 50 — plus three 🟡 (REST drops `output.warnings`; FFI drops them too; the GraphQL `where` branch bypasses `SearchCommand`) and four ⚪ (duplicated pagination bound in `base_types.rs`, REST's per-hit `fields` omission, CLI+GraphQL-only conformance parity, the surviving uncapped FFI `search`).

Rubric verdicts: B1 fail; B2-B19 pass.

## Bob

Doubt + de-slop lens, codex (static read-only sandbox), **after one retry (inlined)** — see Run notes for why the first dispatch refused.

FIX:
- 🟡 CLI silently replaces offsets above i64::MAX with i64::MAX, violating the required overflow rejection (`ddb-cli/src/commands/query.rs:53`).
- 🟠 REST discards `output.warnings`, hiding `REINDEX_SKIPPED_FILES` on an incomplete index; the design exempts CLI/FFI warning forwarding, not REST (`ddb-server/src/rest.rs:370`).
- 🟡 Required GraphQL resolver regressions absent: omitted-limit default, argument decoding, preserved `where` branch (`ddb-server/src/schema/queries.rs:148`).
- 🟡 `parse_cli_search_hits` exceeds the 50-line limit and mixes traversal with metadata decoding (`tests/e2e/conformance/comparator.rs:128`).

VERIFY:
- ⚪ Binding regeneration plus Swift/Kotlin smoke results not supplied (`ddb-core/src/ffi/records.rs:132`). **Not queued: names no exact runnable command** (and the mobile toolchain is absent on this host — deferred, see below).

KNOWN:
- (none)

Rubric verdicts: R1 fail, R2 pass, R3 pass, R4 pass, R6 pass, R7 fail, R8 pass, R9 fail, R10 fail, R11 pass, R12 fail, R13 fail.
Doubt rubric: D1 pass, D2 pass, D3 pass, D4 pass, D5 pass.

## Carl

Gemini via the copilot backend, model `gemini-3.8-flash`. Two 🟡 (98-line GraphQL search-resolver closure; 58-line `parse_cli_search_hits`) and three ⚪ (`queries.rs` at 909 lines, `main.rs` at 836 lines, the parallel-contention timeout of `shared_actor_preserves_schema_applys_owned_transaction`). He independently reproduced the suite failure and confirmed it passes in isolation, and that `apply_schema` does not appear in this diff.

Rubric verdicts: R1 pass, R2 pass, R3 pass, R4 pass, R6 pass, R7 pass, R8 pass, R9 pass, R10 pass, R11 pass, R12 fail, R13 fail.

## Decision gate (cycle 1)

Two unresolved 🟠 High findings (R1, R2+R7) remain, no 🔴 Critical. `state.cycle` 1 < `rework_cap` 2, so this cycle does not converge and goes to rework.

Classified **fix** (rework tasks): R1, R2, R3, R4, R5, R6, R7, R8, R9, R10, R11, R12, R13, R14, R15, R19.
Classified **settled deferral** (recorded in `00176-route-search-through-contract-v1-ledger.json` and `deferred_decisions`): R16 (REST harness driver is scoped to 00175 by the PRD itself), R17 (pre-existing public FFI method; removing it is a breaking change needing its own compatibility review), R18 (Swift/Kotlin toolchain absent on this host; PRD 00191 was rescoped to bindgen for the same reason), R20 (`ddb-cli/src/main.rs` is a pre-existing oversized dispatch file this PRD only appended flags to), R21 (pre-existing flake in `ddb-server/src/actor/tests/mod.rs`, a file outside this diff).

Verdict: 21 findings
Tests: 2019 passed, 1 failed, 0 skipped (suite run this cycle, `cargo test-ci` exit 101; the single failure is the pre-existing `actor::tests::shared_actor_preserves_schema_applys_owned_transaction` timeout in an untouched file, which passes in isolation — the run aborted at that target, so the remaining targets did not execute)
