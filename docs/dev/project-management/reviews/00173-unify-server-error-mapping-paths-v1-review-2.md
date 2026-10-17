---
prd: docs/dev/project-management/prds/wip/00173-unify-server-error-mapping-paths-v1.md
review: 2
date: 2026-10-17
head_sha: cbd9166e85a24bf8626cb11e055caf34fc95f3de
reviewers: alice,blake,bob,carl
agents:
  alice: available
  blake: available
  bob: available
  carl: available
dispatch_rows:
  bob: 8e84c0c2
  carl: b8df2921
---

# Review: 00173-unify-server-error-mapping-paths-v1

Diff range: `7b2923693033ad680411b15d7612bc11e3f36af5..cbd9166e85a24bf8626cb11e055caf34fc95f3de`

codex_rung_guard: not fired

pack: ok

## Review Summary

Reviewed: 11 completed tasks
PRDs checked: 00173-unify-server-error-mapping-paths-v1.md
Cycle: 2 of cap 2 (incremental review, `--since 7b29236` — the cycle-1 head_sha)

### Agent Status
- Alice: ✅ Available (consensus lens, legacy engine). Her verdict block omits `R5`; the remaining twelve rules are answered.
- Blake: ✅ Available (blind lens, PRD-only)
- Bob: ✅ Available (doubt lens, codex) — after one retry (inlined). The first dispatch was again a lack-of-input refusal: codex could not open any file (`sandbox-exec: sandbox_apply: Operation not permitted`) and failed every rubric rule. The retry inlined the review context and the full diff verbatim and produced a complete review.
- Carl: ✅ Available (UI/design lens, gemini via copilot backend) — no issues found.

Mechanical test checks (tautological-test detector, fail-first replay): no `[MECH]` findings.

No prior-cycle verification-check queue exists for this PRD, so nothing was carried forward.

Consolidation note: `consolidate_findings.py` merged three Blake citations in `ddb-core/src/app_contract/error.rs` after suffix stripping (`:77 ~ :310 ~ :305`). The row below (R3) therefore carries the Parse-status text; Blake's two other points on that file — the unregistered-code 422→500 change and the `Display`-prefix stripping edge case — are recorded verbatim in his section and are judged alongside R3.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [1/4] | 🟡 | Tests that can no longer fail. `From<DoogatError>` now delegates to `From<&DoogatError>`, so `borrowed_and_owned_conversions_agree_for_every_simple_variant`, `..._for_every_structured_code` and `..._for_field_and_context_bearing_errors` compare a function to itself; they pass against the pre-change code too. `to_graphql_error` is literally `to_graphql_error_from_app(AppError::from(e))`, so `legacy_and_app_graphql_paths_agree_on_message_and_extensions`, `..._on_every_policy_code` and `..._on_every_structured_constructor` are the same. About 120 lines of tests assert an invariant that holds by construction. Simplification: delete them, keeping the literal-value tests (`legacy_graphql_error_on_*`, `legacy_graphql_error_exposes_field_hint_only_where_the_policy_has_one`). | ddb-core/src/app_contract/error/tests.rs:546 | 8 | ALICE |
| R2 | [1/4] | 🟡 | No guard that every `codes::*` constant is registered in `ERROR_POLICY`. An unregistered code silently becomes a redacted 500, and that was exactly the INVALID_ZONE regression. The only enumeration is `ALL_POLICY_CODES`, a hand-copied list of the private table in the server test crate (ddb-server/src/error/tests.rs:497). It can drift from the table without failing, and it does not catch a new constant missing from the table. Fix: add a core test that checks each `codes` constant (or a `codes::ALL` array) resolves through `policy_for`, and that `ERROR_POLICY` has no duplicate codes. | ddb-core/src/app_contract/error.rs:162 | 8 | ALICE |
| R3 | [1/4] | 🟡 | `DoogatError::Parse` changed on REST and NoSQL from redacted `INTERNAL_ERROR`/500 to `PARSE_ERROR`/422 with the raw message. The PRD says "no HTTP status changes on any code" and gives only 409/404/500 as category defaults, so the 422 for PARSE_ERROR is not in the spec. REST and NoSQL now also expose parse text, which can echo stored-file content that REST previously hid. The change is disclosed in CHANGELOG.md:22 and the table row is at ddb-core/src/app_contract/error.rs:77-82. | ddb-core/src/app_contract/error.rs:77 | general | BLAKE |
| R4 | [1/4] | 🟡 | Conversion-equivalence tests now compare the borrowed conversion with an owned conversion that delegates directly to it, so they cannot detect independent drift. Remove the expanded equivalence helpers and loops; retain literal assertions for code, category, field, and details. | ddb-core/src/app_contract/error/tests.rs:512 | 8 | BOB |
| R5 | [1/4] | ⚪ | The header of the moved test file says "the tests are unchanged", but the diff adds many tests there (INVALID_ZONE, the unregistered-code redaction and the code-driven rendering test). The comment is inaccurate. | ddb-core/src/app_contract/error/tests.rs:3 | 8 | ALICE |
| R6 | [1/4] | ⚪ | `unregistered_code_minted_at_runtime` builds a time-based string and uses `Box::leak`, which makes the test non-deterministic. A fixed second unregistered literal proves the same rule, as the neighbouring fixed codes already show. | ddb-server/src/error/tests.rs:553 | 11 | ALICE |
| R7 | [1/4] | ⚪ | No test checks that every `codes::*` constant has a row in ERROR_POLICY. The code lists in the tests are hand-written, so a new code without a row silently becomes Internal/500. That fails closed, but it leaves the "touch ONE table" promise unenforced. | ddb-core/src/app_contract/error/tests.rs:88 | general | BLAKE |
| R8 | [1/4] | ⚪ | Small additions beyond the PRD, all justified by regression fixes or tests: a new public codes::INVALID_ZONE with a table row; table rows for TRANSACTION_NOT_SUPPORTED, PARSE_ERROR, INVALID_PATH and BAD_REQUEST; nosql_error made public for the parity suite. This public API is not in the PRD. | ddb-server/src/nosql_api.rs:25 | general | BLAKE |
| R9 | [1/4] | ⚪ | `classify` is now referenced only by tests (ddb-server/src/error.rs:9). It is a leftover thin wrapper the PRD said to keep. | ddb-server/src/error.rs:9 | general | BLAKE |
| R10 | [1/4] | ⚪ | I found no executable showboat walkthrough for 00173, which the PRD's Test Strategy asks for. The checkout has no dev/local/walkthroughs directory (it is gitignored), so I cannot confirm either way. | N/A | general | BLAKE |
| R11 | [1/4] | ⚪ | The NoSQL leg of the parity suite checks the wiring, not an independent policy, because NoSQL just calls the REST helper. This is acknowledged in the test comment at ddb-server/tests/error_parity.rs:82. | ddb-server/tests/error_parity.rs:86 | general | BLAKE |

No 🔴 CRITICAL and no 🟠 HIGH finding in this cycle.

## Alice

Cycle-1 findings, checked against the code — all resolved, no regression from the rework:

- R1/R14/R16 (duplicate `From` impls): resolved. `From<DoogatError>` now calls `AppError::from(&err)` (ddb-core/src/app_contract/error.rs:263); `code_for`, `policy_for` and `field_for` replace the arm-by-arm copies.
- R4 (the `INVALID_ZONE` regression): resolved. `codes::INVALID_ZONE` is registered as InvalidInput / 422 (error.rs:174), helpers.rs:607 uses the constant, and every non-test Structured code construction resolves through `ERROR_POLICY`. Tests at core tests.rs:405 and error_parity.rs:341.
- R5 (no single table): largely resolved. One `ERROR_POLICY` table holds category, `http_status` and `display_prefix`; `error_rendering` and the conversion both read it. Remaining gap is her R2 finding above.
- R6/R8/R10 (context leak on the legacy GraphQL path): resolved. `to_graphql_error` is now `to_graphql_error_from_app(AppError::from(e))` (ddb-server/src/error.rs:28) and `attach_context` is removed; tests at ddb-server/src/error/tests.rs:568-601.
- R2/R7/R9/R13 (parity tests): resolved. Expectations are literals for code, message, status and `FfiVariant`; `nosql_error` is driven through the real axum response body; the suite adds INVALID_PATH, BAD_REQUEST, PARSE_ERROR, TRANSACTION_NOT_SUPPORTED, INVALID_ZONE and an unregistered code.
- R3/R11/R12 (docs): resolved. CHANGELOG, errors.md and server.md record the unregistered-code (422→500) and Parse (500→422) status changes and the legacy GraphQL redaction.
- R15: resolved. `SQL_ERROR` has a doc comment; the redundant `SCHEMA_APPLY_PARTIAL` arm is gone.
- R17: the discarded item; not re-raised.

File and function sizes: both new test files are under 800 lines (core 671, server 774). No function exceeds 50 lines. Her review is static — her own `cargo test` run hit the tool timeout while compiling, so she relied on the cycle gate result.

New findings: R1, R2, R5, R6 above.

R1: pass
R2: fail
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

(verdict warning: missing R5)

## Blake

He read the code and docs without the diff. He did not get a test result: his `cargo test -p ddb-server --test error_parity --test http_error_t143 --lib` was still compiling after 600 s. Every behavioral claim is from reading the code and tests.

Found and matching the spec: `SqlEngine` → `SQL_ERROR` / InvalidInput (ddb-core/src/app_contract/error.rs:203) with the code registered (ddb-core/src/error.rs:125); the sole `ERROR_POLICY` table carrying code, category, `http_status` and display prefix (error.rs:64) read by the conversion (error.rs:241); `ErrorRendering`, `error_rendering` and the core `FfiVariant` enum with a plain `u16` status and no HTTP/UniFFI/server dependency in ddb-core; Internal-category redaction to "internal error" with no field or details, logged once; the per-code statuses (SQL_ERROR, NOT_NULL_VIOLATION, UNKNOWN_FIELD, TYPE_NOT_REGISTERED 422; VALIDATION_ERROR 400; Conflict 409; NotFound 404; Internal 500); the thin adapters (`http_error_response`, `rest_error`, `nosql_error`, `to_graphql_error_from_app`, `classify`) with `to_graphql_error` now routed through `AppError` and the `SqlEngine` branch in `classify` gone; the parity suite over four entry points plus `FfiVariant` against literal expectations; the docs and CHANGELOG; and FFI/PgWire deferred as the PRD says.

Findings:

- [BLAKE] 🟡 `DoogatError::Parse` changed on REST and NoSQL from redacted `INTERNAL_ERROR`/500 to `PARSE_ERROR`/422 with the raw message. The PRD says "no HTTP status changes on any code" and gives only 409/404/500 as category defaults, so the 422 for PARSE_ERROR is not in the spec. REST and NoSQL now also expose parse text, which can echo stored-file content that REST previously hid. The change is disclosed in CHANGELOG.md:22 and the table row is at ddb-core/src/app_contract/error.rs:77-82. | File: ddb-core/src/app_contract/error.rs:77 | Task: general
- [BLAKE] 🟡 A structured code that is not in the table went from 422 with its own code and message on REST to 500 redacted. This follows the PRD's Internal-redaction rule and CHANGELOG.md:22 discloses it, but the PRD's "no HTTP status changes on any code" is not literally true. Any structured code added elsewhere without a table row will redact silently. | File: ddb-core/src/app_contract/error.rs:310 | Task: general
- [BLAKE] ⚪ Prefix stripping is keyed on the code's table row. A hand-built `AppError` with a code such as `VALIDATION_ERROR` and a message that really starts with "validation: " loses that text. The policy is quietly tied to the thiserror `Display` strings in ddb-core/src/error.rs. | File: ddb-core/src/app_contract/error.rs:305 | Task: general
- [BLAKE] ⚪ No test checks that every `codes::*` constant has a row in `ERROR_POLICY`. | File: ddb-core/src/app_contract/error/tests.rs:88 | Task: general
- [BLAKE] ⚪ Small additions beyond the PRD (public `codes::INVALID_ZONE`; table rows for TRANSACTION_NOT_SUPPORTED, PARSE_ERROR, INVALID_PATH, BAD_REQUEST; `nosql_error` made public). | File: ddb-server/src/nosql_api.rs:25 | Task: general
- [BLAKE] ⚪ `classify` is now referenced only by tests. | File: ddb-server/src/error.rs:9 | Task: general
- [BLAKE] ⚪ No executable showboat walkthrough found for 00173 (the walkthrough directory is gitignored, so he cannot confirm either way). | File: N/A | Task: general
- [BLAKE] ⚪ The NoSQL leg of the parity suite checks the wiring, not an independent policy. | File: ddb-server/tests/error_parity.rs:86 | Task: general

B1: pass
B2: pass
B3: pass
B4: pass
B5: fail
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
B17: pass
B18: pass
B19: pass

## Bob

Doubt lens + de-slop, after one retry (inlined).

FIX:
- [BOB] 🟡 Conversion-equivalence tests now compare the borrowed conversion with an owned conversion that delegates directly to it, so they cannot detect independent drift. Remove the expanded equivalence helpers and loops; retain literal assertions for code, category, field, and details. | File: ddb-core/src/app_contract/error/tests.rs:512 | Task: 8

VERIFY:
- (none)

KNOWN:
- (none)

R1: pass
R2: fail
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
D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Carl

Backend: gemini via the copilot CLI. He read the context, the full diff and the pack, inspected `ddb-core/src/app_contract/error.rs`, `ddb-server/src/error.rs` and `ddb-server/src/http_error.rs`, checked file line counts and swept for TODO/FIXME/`dbg!`/`todo!`/`unimplemented!` and `#[ignore]` (no matches). His own `cargo test-ci` attempt contended on the cargo build lock and produced no result.

[CARL] ✅ No issues found

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

Verdict: 11 findings
Tests: 2523 passed, 0 failed, 0 skipped (reused from last-verification.json at 6f36f19)
