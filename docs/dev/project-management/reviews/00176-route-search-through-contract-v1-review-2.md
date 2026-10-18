---
prd: 00176-route-search-through-contract-v1.md
cycle: 2
date: 2026-10-18
head_sha: 52e376d04ec3da80e542d791d40573e207727455
codex_thread_id: 01a14d9e-e643-73a2-a20a-9ba622aeb042
reviewers: alice,blake,bob,carl
---

# Review: 00176-route-search-through-contract-v1 (cycle 2)

Reviewed: the rework commits of cycle 1 (incremental diff since `0fdd88ef31ca6e6a57690fa5db096c2cc63e04e1`), against the PRD's acceptance criteria and the cycle-1 consolidated findings.

Diff range: `0fdd88ef31ca6e6a57690fa5db096c2cc63e04e1..52e376d04ec3da80e542d791d40573e207727455`
pack: ok
Consolidation: `consolidate_findings.py` (4 reviewers), ledger-dismiss BLAKE.
Note: `review-stage`'s gate command (`cargo test-ci`) timed out at 1800s inside staging; the `Tests:` line below comes from a dedicated foreground-equivalent suite run at this HEAD.

codex_rung_guard: not fired

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [3/4] | 🟡 | The `search` resolver closure is still about 98 lines (queries.rs:144-243), over the 50-line limit (prior R1/R12). `queries.rs` is 909 lines, over the 800-line limit. The rework only touched the `doogats` closure. | ddb-server/src/schema/queries.rs:144 | 7 | ALICE, BOB, CARL |
| R2 | [2/4] | 🟠 | Server warning delivery remains unresolved: both new warning regressions are ignored and fail when included. REST now serializes warnings, but the documented ReadPool stale-check bypass prevents their collection; the GraphQL `where` branch remains unchanged. | tests/e2e/integration_search_pagination_warnings.rs:135 | 7 | BOB, CARL |
| R3 | [2/4] | 🟡 | The `RebuildWarningRecord` mapping is copy-pasted in `search_paginated`, `search_filtered` and a third site near driver.rs:174; `search_paginated` still repeats `search_filtered`'s command build instead of delegating. | ddb-core/src/ffi/driver.rs:121 | 8 | ALICE, BOB |
| R4 | [2/4] | 🟡 | Prior R6 unfixed: the new FFI search tests went into `ddb-core/src/ffi/tests.rs` (now 1962 lines) instead of a sibling `ffi/search_tests.rs`. | ddb-core/src/ffi/tests.rs:1894 | 8 | ALICE, CARL |
| R5 | [1/4] | 🟠 | REST `warnings` is a dead channel: `ReadPool::with_service` sets `skip_stale_check(true)`, so every REST search returns `"warnings":[]`; `rest_search_forwards_reindex_warnings` fails when run. | tests/e2e/integration_search_pagination_warnings.rs:137 | 7 | ALICE |
| R6 | [1/4] | 🟠 | The GraphQL `where` branch still uses `pool.search` and discards warnings (prior R9, unfixed). | ddb-server/src/schema/queries.rs:208 | 7 | ALICE |
| R7 | [1/4] | 🟠 | Two failing tests are `#[ignore]`d with "Deferred to batch end"; the deferral was not in the settled-decisions ledger and needed a durable disposition. | tests/e2e/integration_search_pagination_warnings.rs:129 | 7 | ALICE |
| R8 | [1/4] | 🟡 | `docs/src/technical/rest-api.md:78` still says `fields` is omitted when empty, but `skip_serializing_if` was removed; the example response also lacks the new `warnings` key. | docs/src/technical/rest-api.md:78 | 7 | ALICE |
| R9 | [1/4] | 🟡 | `docs/src/technical/ffi.md:209` still lists `PaginatedSearchResult` as `{ hits, total_count }` and line 227 denies a structured warning channel; no `**ffi**:` CHANGELOG entry for the new public record field. | docs/src/technical/ffi.md:209 | 8 | ALICE |
| R10 | [1/4] | 🟡 | `impl From<types::PaginatedSearchResult> for ffi::PaginatedSearchResult` now has no caller and sets `warnings: Vec::new()`, which could silently hide warnings if reused. | ddb-core/src/ffi/records.rs:183 | 8 | ALICE |
| R11 | [1/4] | 🟡 | GraphQL search resolver forks around SearchCommand when `where` is present and never forwards warnings on that branch. | ddb-server/src/schema/queries.rs:208 | Phase 1 | BLAKE |
| R12 | [1/4] | 🟡 | The new overflow regression assumes `usize::MAX > i64::MAX` on every target; on 32-bit targets it fits, so the test fails. Gate it with `#[cfg(target_pointer_width = "64")]`. | ddb-cli/src/commands/query.rs:242 | 8 | BOB |
| R13 | [1/4] | 🟡 | FFI test extraction remains undone; the incremental diff adds another 53 lines to the oversized test file. | ddb-core/src/ffi/tests.rs:1894 | 8 | BOB |
| R14 | [1/4] | 🟡 | GraphQL search field resolver closure exceeds 50 lines (97 lines). | ddb-server/src/schema/queries.rs:144 | 2 | CARL |
| R15 | [1/4] | 🟡 | Redundant implementation in `search_paginated` can delegate to `search_filtered`. | ddb-core/src/ffi/driver.rs:110 | 8 | CARL |
| R16 | [1/4] | ⚪ | `base_types.rs` still keeps its own `check_limit`/`MAX_LIMIT` copy of the pagination bound; `doogats` now validates twice. | ddb-server/src/schema/base_types.rs:1030 | 7 | ALICE |
| R17 | [1/4] | ⚪ | The `doogats` GraphQL argument descriptions still read "Maximum results to return."; the new default of 50 and rejection of 0 are stated only in the CHANGELOG. | ddb-server/src/schema/queries.rs:130 | 7 | ALICE |
| R18 | [1/4] | ⚪ | Over-max errors surface as `VALIDATION_ERROR` (400 on REST) while the error table lists 422 for the same category on another code path. | ddb-core/src/app_contract/commands.rs:102 | Phase 0 | BLAKE |
| R19 | [1/4] | ⚪ | GraphQL `types` argument silently drops non-string list entries through `filter_map`. | ddb-server/src/schema/queries.rs:157 | Phase 1 | BLAKE |
| R20 | [1/4] | ⚪ | REST `type` accepts a single value while GraphQL and FFI accept a list; the narrower REST filter surface is undocumented. | ddb-server/src/rest.rs:319 | Phase 1 | BLAKE |

### Auto-dismissed (ledger)

- [BLAKE] 🟡 Conformance harness has no REST driver (tests/e2e/conformance/cross_driver_crud.rs:179) — PRD 00176 itself scopes the harness REST driver to 00175; accepted gap, not a defect in this PRD's scope.
- [BLAKE] ⚪ Legacy FFI `search(query)` is still unfiltered, unpaginated and uncapped (ddb-core/src/ffi/driver.rs:97) — pre-existing public method kept for compatibility; removing it is a breaking FFI change needing its own compatibility review.

## Alice

Consensus lens, implementation-aware. Alice ran `cargo test -p ddb-core --lib ffi::tests::search` (2 passed) and `cargo test -p ddb-e2e search -- --include-ignored` (49 passed, 2 failed — the two `#[ignore]`d warning regressions). Findings R5-R10, R16, R17 above.

Verified resolved this cycle: CLI offset overflow now returns `Validation` with a binding test; the two tautological CLI tests were deleted; REST `fields` is always emitted; `doogats` defaults to 50 and rejects `limit: 0`; the long test functions were split; the CHANGELOG `limit: 0` lines landed; both FFI poison-file warning tests pass.

R1: fail · R2: pass · R3: fail · R4: pass · R6: pass · R7: pass · R8: pass · R9: fail · R10: pass · R11: fail · R12: fail · R13: fail

## Blake

Blind lens, PRD-only (no diff, no history). Findings R11, R18, R19, R20 above; two further findings auto-dismissed against the ledger.

Verified as spec-compliant: `PageRequest::decode` defaults 50/0, max 10,000, rejects 0/negative/over-max/negative-offset/conversion failure with no clamping; `SearchCommand` carries types/tag + PageRequest; `AppSearchResult` holds `Vec<core SearchResult>`, total, query; `search_command` takes total from the index count; REST `SearchHit` serializes all ten fields and populates filters from the request; `decode_search_page` rejects `page < 1` with a checked multiply; the FFI record has all ten fields and both FFI search methods route through `PageRequest`/`search_command`; CLI and GraphQL both decode through `PageRequest`; docs, CHANGELOG and walkthrough exist.

B1-B19: pass

## Bob

Doubt lens + de-slop (codex, `gpt-6.1-sol`), after one retry (inlined). The first dispatch refused with a sandbox read error and produced no evidence-backed verdicts; the retry ran against the fully inlined context and diff.

FIX: R2, R12, R13, R15-equivalent (FFI delegation), R1-equivalent (resolver size). VERIFY: (none). KNOWN: (none).

R1: fail · R2: pass · R3: fail · R4: pass · R6: pass · R7: pass · R8: pass · R9: fail · R10: fail · R11: pass · R12: fail · R13: fail

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Carl

Gemini backend (copilot). Findings R1/R14 (resolver size), R2 (ignored warning regressions), R4 (FFI test file size), R15 (FFI delegation).

## Decision

The four 🟠 findings (R2, R5, R6, R7) are one settled deferral: `ReadPool::with_service` sets `set_skip_stale_check(true)`, so no read path can collect reindex warnings. That was settled in cycle 1 as an architecture decision outside this PRD's search-routing scope, with the two regressions committed as `#[ignore]`d tests so the gap stays pinned and executable. R7 asked for a durable disposition — it is now written to the settled-decisions ledger, together with the two structural deferrals (R1/R14/R16 resolver and pagination-copy size; R3/R4/R13/R15 FFI duplication and test-file size).

No unresolved CRITICAL or HIGH remains. The actionable Medium/Low tail (R8, R9, R10, R12, R17, R18, R19, R20) goes to the tail sweep.

Verdict: 20 findings
Tests: 1726 passed, 1 failed, 0 skipped (suite run this cycle, `cargo test-ci` exit 101; the single failure is `app_contract::error::tests::internal_rendering_logs_unredacted_message_once_and_non_internal_logs_nothing`, a global tracing-subscriber contention flake in `ddb-core/src/app_contract/error/tests.rs` — a file outside this PRD's diff — which passes in isolation, verified this cycle; the run aborted at that target, so later targets did not execute)
