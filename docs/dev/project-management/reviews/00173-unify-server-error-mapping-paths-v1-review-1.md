---
prd: docs/dev/project-management/prds/wip/00173-unify-server-error-mapping-paths-v1.md
review: 1
date: 2026-10-17
head_sha: 7b2923693033ad680411b15d7612bc11e3f36af5
reviewers: alice,blake,bob,carl
agents:
  alice: available
  blake: available
  bob: available
  carl: available
dispatch_rows:
  bob: a92b661b
  carl: 17ac438c
---

# Review: 00173-unify-server-error-mapping-paths-v1

Diff range: `083b56ca40675950e6d77c2f2e5b52636b937ffe..7b2923693033ad680411b15d7612bc11e3f36af5`

codex_rung_guard: not fired

pack: ok

## Review Summary

Reviewed: 7 completed tasks
PRDs checked: 00173-unify-server-error-mapping-paths-v1.md
Cycle: 1 of cap 2 (full review, `--since work_start_sha`)

### Agent Status
- Alice: ✅ Available (consensus lens, legacy engine)
- Blake: ✅ Available (blind lens, PRD-only)
- Bob: ✅ Available (doubt lens, codex) — after one retry (inlined). The first dispatch was a lack-of-input refusal: codex could not open any file (`sandbox-exec: sandbox_apply: Operation not permitted`) and failed every rubric rule. The retry inlined the review context and the full diff verbatim and produced a complete review.
- Carl: ✅ Available (UI/design lens, gemini via copilot backend)

Mechanical test checks (tautological-test detector, fail-first replay): no `[MECH]` findings.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [3/4] | 🟡 | `From<&DoogatError> for AppError::from` is about 117 lines and repeats the by-value impl arm for arm. That fails the 50-line limit (R12) and creates a second drift point. A comment says "keep in lockstep" and one test guards it. Simplification: make `From<DoogatError>` call `AppError::from(&err)`, or extract one shared `classify_structured(code, context)` helper. This is behavior-preserving, since the by-value path would only gain payload clones. | ddb-core/src/app_contract/error.rs:41 | 2 | ALICE, BOB, CARL |
| R2 | [3/4] | 🟡 | The ffi_variant check in `assert_parity` is a `match` over all four variants with `{}` arms. It cannot fail at runtime and is vacuous (R2). The parity helper also compares `classify`, `to_graphql_error_from_app` and `http_error_response`, which all call `error_rendering`. Code and message equality is therefore near-tautological. The status expectation is the only discriminating assert. Fix: assert the expected `FfiVariant` per sample, and compare against literal expected code and message strings. | ddb-server/tests/error_parity.rs:838 | 6 | ALICE, BOB, CARL |
| R3 | [2/4] | 🟠 | CHANGELOG, errors.md and server.md say "No HTTP status changed for any existing code". That is inaccurate for unregistered Structured codes (422 to 500) and for Parse (500 to 422, with a code change). The compatibility note is also missing the legacy-GraphQL (`to_graphql_error`) side of the redaction change. Fix: make the note accurate once the two High items are resolved. | CHANGELOG.md:21 | 7 | ALICE, BLAKE |
| R4 | [1/4] | 🟠 | Regression: unregistered Structured codes now render as redacted 500 "internal error" on REST/NoSQL, and on GraphQL's legacy classify path. `INVALID_ZONE` is a live, user-actionable code from SQL `CREATE TABLE ... ZONE <bad>`. It is built at ddb-core/src/sql_engine/helpers.rs:607 and is not in `codes`. Before the change, REST gave 422 with the message. Now it falls to the `_ => Internal` arm at ddb-core/src/app_contract/error.rs:239 and returns 500 "internal error". GraphQL's `to_graphql_error` path used to pass the message through and now redacts it. `http_error_t143.rs` rewrote `unknown_structured_code_maps_to_422_with_code_passthrough` to bless this. That contradicts the PRD ("no HTTP status changes on any code"). The CHANGELOG and errors.md say the same. Fix: register `INVALID_ZONE` as an InvalidInput code (422) in `codes` and in the policy, and add a REST test for it. | ddb-core/src/app_contract/error.rs:239 | 3 | ALICE |
| R5 | [1/4] | 🟠 | There is no single table. The spec's Success Metrics and Error-case scenario say a new error code needs one edit, and that is not true here. The code→category mapping is hand-copied in `From<DoogatError> for AppError` and again in `From<&DoogatError> for AppError`. The status mapping and the Display-prefix list are two more separate `match` blocks in `error_rendering`. A new Conflict-category structured code needs edits in at least three places. Unlisted codes fall through to `Internal` and 500. The "derive existing category conversions from the sole policy table" requirement is not met, and no test covers the brand-new-code scenario. | ddb-core/src/app_contract/error.rs:102 | 2 | BLAKE |
| R6 | [1/4] | 🟠 | The GraphQL resolvers' main error path still re-decides redaction. `to_graphql_error` takes code and message from the policy, but `attach_context` (called at ddb-server/src/error.rs:29) copies the raw `Structured` context into the extensions with no redaction check. For an `Internal`-category structured code such as `SCHEMA_APPLY_PARTIAL` or an unregistered code, GraphQL gets the redacted message but still ships the unredacted context. `to_graphql_error` is used by roughly 40 call sites in `schema/queries.rs`, `discovery_queries.rs`, `singleton.rs` and `operations.rs`, while only 3 sites use `to_graphql_error_from_app`. The spec says no adapter re-decides redaction and GraphQL and REST make the same redaction decision, and this path breaks both. This path also never emits the `field` extension. | ddb-server/src/error.rs:29 | 3 | BLAKE |
| R7 | [1/4] | 🟠 | The parity tests prove little, and the NoSQL and `ffi_variant` legs are missing. Every renderer calls the same `error_rendering` function, and `assert_parity` compares `classify` against that same function through two wrappers. A wrong shared policy would pass. NoSQL is not driven in `error_parity.rs`. The only NoSQL coverage is a 4-case unit test in nosql_api.rs:127. The ffi_variant check is an exhaustive `match` with empty arms (error_parity.rs:66-71), which asserts nothing. The spec requires "ffi_variant hint exists for each" sample and identical code and redaction across GraphQL, REST and NoSQL. | ddb-server/tests/error_parity.rs:63 | 6 | BLAKE |
| R8 | [1/4] | 🟠 | Legacy GraphQL adapters still attach original Structured context after classify redacts the message. SCHEMA_APPLY_PARTIAL can therefore expose details that error_rendering explicitly discarded. Route to_graphql_error/to_server_error through the sanitized rendering, including context. | ddb-server/src/error.rs:10 | general | BOB |
| R9 | [1/4] | 🟡 | The parity tests never drive the NoSQL renderer (`nosql_error`), although the file header and the task say they cover NoSQL. They also skip INVALID_PATH and BAD_REQUEST (400), TRANSACTION_NOT_SUPPORTED, PARSE_ERROR and unregistered Structured codes. Those gaps let the two regressions above through. Fix: add samples for those codes, and route at least one sample through `nosql_error`. | ddb-server/tests/error_parity.rs:807 | 6 | ALICE |
| R10 | [1/4] | 🟡 | The parity test never calls `to_graphql_error`, the legacy path that carries the context leak. It covers only `classify` and `to_graphql_error_from_app`, so it cannot catch the leak above. | ddb-server/tests/error_parity.rs:37 | 6 | BLAKE |
| R11 | [1/4] | 🟡 | The docs are stale or incomplete. The server.md "Error Mapping" table still says "All others → INTERNAL_ERROR", although `Parse` is now `PARSE_ERROR` and structured codes keep their own code. The table also omits the structured codes. errors.md and the CHANGELOG do not record the `Parse` and unregistered-code changes. | docs/src/technical/server.md:917 | 7 | BLAKE |
| R12 | [1/4] | 🟡 | Compatibility claims omit actual status changes: Parse changes from redacted INTERNAL_ERROR/500 to exposed PARSE_ERROR/422; unknown Structured codes change from 422 to 500. Document these changes and add a Parse regression covering its intended disclosure policy. | docs/src/technical/errors.md:141 | 7 | BOB |
| R13 | [1/4] | 🟡 | The conformance suite never invokes NoSQL; calling error_rendering does not exercise that adapter. Add representative NoSQL response assertions against REST, including Internal redaction and SQL_ERROR. | ddb-server/tests/error_parity.rs:34 | 6 | BOB |
| R14 | [1/4] | 🟡 | From<DoogatError> duplicates entire match structure of From<&DoogatError>; delegate via AppError::from(&err) | ddb-core/src/app_contract/error.rs:50 | 2 | CARL |
| R15 | [1/4] | ⚪ | `codes::SQL_ERROR` has no doc comment, unlike its neighbours (for example `TRANSACTION_NOT_SUPPORTED`). In the by-value and borrowed `Structured` arms, `SCHEMA_APPLY_PARTIAL => Internal` is redundant next to `_ => Internal`. | ddb-core/src/error.rs:123 | 1 | ALICE |
| R16 | [1/4] | ⚪ | Maintainability and scope notes. The two `From` impls are near-identical copies (about 120 lines) held together only by a "keep in lockstep" comment (ddb-core/src/app_contract/error.rs:167). Only `borrowed_and_owned_conversions_agree` guards them, and it covers 4 samples. The code→prefix strip table in `error_rendering` is keyed on code strings and is a fifth copy of per-code knowledge (ddb-core/src/app_contract/error.rs:338). `rest.rs` and `nosql_api.rs` still build inline `ErrorBody` values with hardcoded `NOT_FOUND` and `BAD_REQUEST` (nosql_api.rs:43-80). These sit outside the policy but match its codes, so I count it as acceptable. PgWire and FFI are untouched, which matches the spec's scoping to 00177 and 00180. | ddb-core/src/app_contract/error.rs:167 | 2 | BLAKE |
| R17 | [1/4] | ⚪ | I did not confirm the tests pass. `cargo test -p ddb-server --test error_parity --test http_error_t143 --lib error` hit the 300 s tool timeout while compiling, and I found no output after that. This review is static. | ddb-server/tests/error_parity.rs:1 | 6 | BLAKE |

Consolidation note: Bob's finding lines carry a markdown list prefix (`- [BOB] ...`) inside his FIX/VERIFY/KNOWN buckets, which `consolidate_findings.py` does not parse. The prefix was stripped from his saved output and consolidation re-run, so his four findings are in the table above. The first run without him is not what this file reports.

## Alice

Six findings plus one low note; see R1-R4, R9, R15 above. Two High items: the unregistered-Structured-code status regression (422→500, `INVALID_ZONE` named concretely) and the undocumented `DoogatError::Parse` change (500 redacted `INTERNAL_ERROR` → 422 exposed `PARSE_ERROR`).

```
R1: pass
R2: fail
R3: pass
R4: fail
R6: pass
R7: pass
R8: pass
R9: fail
R10: pass
R11: pass
R12: fail
R13: pass
```

## Blake

Blind lens, PRD-only. Four High findings: no single table (code→category copied across two `From` impls, status and prefix tables separate); the legacy `to_graphql_error`/`attach_context` path still ships unredacted structured context for Internal-category codes; REST status/message changes contradicting the CHANGELOG's "no status changed" claim; parity tests that cannot fail because every renderer calls the same function, with NoSQL and `ffi_variant` legs missing.

Confirmed correct by Blake: the `SqlEngine` arm and `codes::SQL_ERROR` exist; `SQL_ERROR` is `InvalidInput`, 422, message preserved on GraphQL/REST/NoSQL; `SCHEMA_APPLY_PARTIAL` redacted and 500; `SCHEMA_DESTRUCTIVE_BLOCKED` 409; `VALIDATION_ERROR` 400; `classify`'s `SqlEngine` branch gone; no pinned CLI `INTERNAL_ERROR` expectation to update. He did not run the tests (tool timeout while compiling) — his review is static (R17).

```
B1: fail
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
B16: fail
B17: pass
B18: pass
B19: pass
```

## Bob

Doubt lens (codex), after one retry (inlined). FIX bucket: the legacy-GraphQL context leak (R8), the omitted compatibility claims (R12), the no-assertion `FfiVariant` match (R2), and the never-invoked NoSQL adapter (R13). VERIFY bucket: none. KNOWN bucket: the borrowed-conversion duplication, which he records as an accepted design limitation rather than a defect (R1/R16).

```
R1: fail
R2: fail
R3: pass
R4: fail
R6: pass
R7: pass
R8: pass
R9: fail
R10: pass
R11: pass
R12: fail
R13: pass

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass
```

## Carl

UI/design lens (gemini via copilot). No frontend surface in this diff; he reviewed it as a generalist and raised three quality findings: the 117-line borrowed conversion, the duplicated match structure, and the empty `FfiVariant` match (R1, R14, R2).

Verdict: 17 findings
Tests: 1932 passed, 0 failed, 0 skipped (suite run this cycle)
