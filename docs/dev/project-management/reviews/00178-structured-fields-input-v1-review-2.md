---
prd: docs/dev/project-management/prds/wip/00178-structured-fields-input-v1.md
review: 2
date: 2026-10-19
head_sha: d53521ca6d4db67dcf69348c79dea0d6cd4a1a6a
codex_thread_id: 01a151ce-acca-7e63-80a9-dc8caf8fb111
reviewers: alice,blake,bob
agents:
  alice: available
  blake: available
  bob: available
  carl: skipped
dispatch_rows:
  bob: 489a4fa8
---

# Review: 00178-structured-fields-input-v1

Diff range: `74aa5eb31647fa070a05ec91c6d40c9f0b776ef3..d53521ca6d4db67dcf69348c79dea0d6cd4a1a6a`
(incremental review of the cycle-1 rework only; cycle 1 reviewed the full PRD range)

codex_rung_guard: not fired

## Review Summary

Reviewed: 9 completed tasks (5 original + 4 `[D1]` rework tasks)
PRDs checked: 00178-structured-fields-input-v1
Pack: ok

### Agent Status
- Alice: ✅ Available (consensus lens, legacy engine)
- Blake: ✅ Available (blind lens, PRD-only)
- Bob: ✅ Available (codex doubt lens + de-slop, inlined prompt, resumed cycle-1 thread `01a151ce`)
- Carl: ⚠️ Skipped. In cycle 1 Carl stalled through the Watcher's whole 6-run budget (~57 min) and published no output, counting as a failed reviewer under retry-policy graceful degradation. He was not re-dispatched this cycle: the reviewed diff has no frontend or design surface (GraphQL/REST boundary decoding and Rust tests), so his specialist lens adds nothing here, and a second 57-minute stall would have consumed the cycle. This is a recorded skip, not a silent drop. His exit was never a permanent exit 4, so no batch latch was written and a later batch will try him again.

### Staging caveat (fail loud)
The first `review-stage` dispatch was killed at the harness 10-minute background
cap mid-gate and produced no summary. It was relaunched with a one-hour
background budget and completed (`elapsed_s` 1244). Its gate verdict was
`counts unavailable (exit 101)`, which is not a counts line, so `autopilot
verify` was run once afterwards, clean and sequential with no concurrent cargo.
The `Tests:` line below is that clean run's result. No reviewer output was
affected.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [1/3] | 🟠 | Prior R2 fix (route GraphQL decode/collision errors through DoogatError::Validation) is wired in ddb-server/src/schema/mutations/operations.rs:31, :65, :104 and :149, but no test asserts `extensions.code == "VALIDATION_ERROR"` on any GraphQL path. Task 6 explicitly required "assert the code in a test". The diff adds none. The e2e GraphQL rejection tests only check message text with contains() (tests/e2e/integration_structured_fields_input.rs:290 and :309), and the only VALIDATION_ERROR assertion there is the REST one (:259). Reverting these four call sites to `ServerError::new(msg, None)` would still pass every test. Add an assertion on `errors[0].extensions.code` for the encoded-string literal and String-variable cases, and for a set/unset collision. | tests/e2e/integration_structured_fields_input.rs:303 | 6 | ALICE |
| R2 | [1/3] | 🟡 | The R2 fix is incomplete. The singleton `update_<type>` and `upsert_<type>` resolvers still wrap decode errors in a bare `async_graphql::ServerError::new(msg, None)`. The same legacy encoded-string rejection therefore still carries no `extensions.code` on the singleton mutations, while the generic mutations now return VALIDATION_ERROR. Use `to_graphql_error(DoogatError::Validation(msg))` at both sites. | ddb-server/src/schema/mutations/singleton.rs:45 | 6 | ALICE |
| R3 | [1/3] | 🟡 | The set/unset collision message gets a doubled prefix. `fields_and_unsets` does `.map_err(|e| e.to_string())`, so the string already starts with "validation: ". The resolver then re-wraps it in `DoogatError::Validation`, and `error_rendering` strips the prefix only once. The client sees "validation: field 'x' is both set and unset in the same update". Carry the `DoogatError` through, or unwrap the inner message before re-wrapping, which is simpler. | ddb-server/src/schema/input.rs:98 | 6 | ALICE |
| R4 | [1/3] | 🟡 | The PRD's Phase 2 conformance task is only half done. It asks for a create-with-typed-fields step across GraphQL + REST. The step exists for GraphQL only (`tests/e2e/conformance/cross_driver_crud.rs:227`). The fixture comment says the REST leg is deferred because no REST conformance driver exists (00175 is not done), so only `GraphqlDriver` and `CliDriver` exist. REST native-boolean storage is covered by `rest_create_stores_native_boolean_and_number` in the e2e file, but not in the conformance harness the PRD names. | tests/e2e/conformance/workflows.rs:187 | Phase 2 | BLAKE |
| R5 | [1/3] | 🟡 | FFI has no way to send a null for a field value. `FieldValue` has only Str, Num, Bool and List (`ddb-core/src/ffi/records.rs:365`), and the driver wraps every entry in `Some` (`driver.rs:87-92` and `:114-117`). Update null intent works only through `unset_fields`. On create, a caller omits the key. This is a defensible reading of "binding-safe representation", but the PRD says null semantics agree across transports. A client cannot send `{note: null}` as one payload. Neither does the 00177 spec appear to require it. FFI also cannot carry nested maps, since `FieldValue::List` holds scalars only. | ddb-core/src/ffi/records.rs:365 | Phase 1 (FFI) | BLAKE |
| R6 | [1/3] | 🟡 | Large integers are silently coerced to f64 and lose precision above 2^53, on both REST (`ddb-server/src/rest.rs:247-250`) and GraphQL (`ddb-server/src/schema/base_types.rs:46-48`). The PRD's coercion risk says to "fail clearly at the boundary". The code has a test that it does not panic, but nothing warns or rejects. | ddb-server/src/schema/base_types.rs:46 | Risks | BLAKE |
| R7 | [1/3] | 🟡 | Task 6 fixes error mapping but adds no regression asserting `extensions.code`. Existing message-only tests would pass if the fix were reverted. Add resolver-level tests asserting `VALIDATION_ERROR` for encoded-string fields and set/unset collisions. | ddb-server/src/schema/mutations/operations.rs:31 | 6 | BOB |
| R8 | [1/3] | 🟡 | The accepted exception incorrectly claims execution-time rejection has an identical effect and "nothing is written." Earlier fields in a multi-mutation operation can already have committed. Keep the accepted exception, but limit the guarantee to the rejected mutation field and document that earlier writes are not rolled back. | docs/dev/project-management/prds/wip/00178-structured-fields-input-v1.md:29 | 9 | BOB |
| R9 | [1/3] | 🟡 | The supplied gate exited 101 before later crates ran; the isolated timeout-test pass does not complete validation. Run `cargo test-ci` without concurrent cargo processes and confirm the complete gate exits successfully. | N/A | general | BOB |
| R10 | [1/3] | ⚪ | Formatting drift: the new lines are over 100 columns, so rustfmt would rewrap them. Examples are `let fields = fields_map(map).map_err(\|msg\| to_graphql_error(DoogatError::Validation(msg)))?;` at :31, and the same shape at :149. A small helper such as `validation_error(msg) -> async_graphql::Error` would remove the four repeated `to_graphql_error(DoogatError::Validation(msg))` closures and fix the line length. | ddb-server/src/schema/mutations/operations.rs:31 | 6 | ALICE |
| R11 | [1/3] | ⚪ | The REST fields-decode error echoes the entire rejected JSON value back to the client (`format!("fields must be a JSON object, got {other}")`). For a large string payload the error body can be large. | ddb-server/src/rest.rs:227 | Phase 1 REST | BLAKE |
| R12 | [1/3] | ⚪ | Some docs still describe the old behavior. `docs/src/technical/transport-policy-inventory.md:76-77` says REST `fields` is a "JSON-string member (parsed via `parse_fields_json`)"; that function no longer exists. `docs/src/guide/building-apps.md:112,142` still says "(A JSON string until PRD 00178; see D-14.)", which reads as history but is confusing next to the new text. | docs/src/technical/transport-policy-inventory.md:77 | Phase 2 docs | BLAKE |
| R13 | [1/3] | ⚪ | I did not run Swift/Kotlin bindgen, `cargo test-ci`, or the e2e suite, so the Phase 1 "bindgen succeeds" acceptance and the e2e greenness are unverified by me. | N/A | general | BLAKE |

Consolidation note: the script merged Alice's two singleton findings (`singleton.rs:45` and `:99`) into R2 after suffix stripping; both sites need the same fix. No `[MECH]` test-check findings were produced this cycle, and cycle 1 wrote no verification-check queue, so nothing was carried forward into the table.

### Auto-dismissed (ledger)

- [BLAKE] ⚪ The singleton `update_<type>` and `upsert_<type>` `input` argument was also changed from `String` to `JSON` (`ddb-server/src/schema/mutations/singleton.rs:44,98`). The PRD does not list singletons. It is consistent with the intent and documented in the CHANGELOG, but it is a public request-shape change beyond the PRD's named inputs. | File: ddb-server/src/schema/mutations/singleton.rs:44 — single-voice; documented as D-14
- [BLAKE] ⚪ The walkthrough is numbered `00001-structured-fields-input.md` instead of following the `{5-digit PRD id}-slug` convention in AGENTS.md (the PRD is 00178). I only checked that the file exists. I did not check how it was authored (showboat or hand-edited) or whether it verifies. | File: dev/local/walkthroughs/00001-structured-fields-input.md — single-voice; duplicate view of R14

## Alice

Consensus lens, implementation-aware, incremental scope.

Prior finding status:
- R1 (PRD deviation on String variables): resolved. PRD 00178 now records it as an accepted exception in Success Metrics, Behavior and Critical Scenarios.
- R2 (no `extensions.code`): partially resolved. The generic create, update, batch and createMany resolvers now use `to_graphql_error(DoogatError::Validation)`, and `to_graphql_error` sets `extensions.code`. It is untested, and the singleton resolvers are not covered.
- R3 (69-line test): resolved. Split into four tests of under 50 lines each, with a shared `assert_fields_decode` helper.
- R4 (t154 legacy literal): resolved. `ddb-server/tests/graphql_response_warnings_t154.rs:45` now uses `fields: { url: "https://example.com" }`. Alice did not re-run the test, and the wider sweep of `ddb-server/tests` for other encoded-string `fields` literals was not evidenced.
- R5-R19: settled, discarded in cycle 1, not re-raised.

Regression check on the scoped diff: the three `rest.rs` test splits assert the same cases as the original test, with no weakened asserts. The `to_graphql_error` import in `operations.rs` resolves. No other regressions found. The `contains()` checks in the e2e tests still match, because the migration hint text is unchanged.

Findings: R1, R2, R3, R10 (see the consolidated table).

Rubric verdicts:
R1: fail
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
R13: fail

R13 note: `ddb-server/src/rest.rs` is now 912 lines, up from 878, and `ddb-server/src/schema/base_types.rs` was already over 800. Alice raised no finding for either, because both were settled in cycle 1 (rest.rs discarded, base_types.rs pre-existing debt). The verdict records the fact only.

## Blake

Blind lens, PRD-only prompt, no diff and no file list. Blake found the implementation himself and did not run tests; he took the cycle gate as given.

What he verified: core normalization in `ddb-core/src/app_contract/fields_normalize.rs` (create omits nulls, update converts nulls to unsets and merges with explicit `unset_fields`, dedupes, rejects set/unset collisions as `DoogatError::Validation` in the InvalidInput category, unit-tested across the Phase 0 acceptance list); REST `fields` now `Option<serde_json::Value>` with a decoder that rejects non-objects and nested nulls, both verbs routed through the shared normalizers; GraphQL `fields` typed `JSON`, string values rejected with the migration hint, `parse_fields_json` gone and `decode_json_fields_object` in its place, all generic and singleton callers migrated; FFI create and update routed through the normalizers with native `Num`/`Bool` preserved; e2e coverage across GraphQL, REST and FFI, and migration notes in CHANGELOG, `rest-api.md`, `server.md`, `data-model.md`, `app-contract.md`, `compatibility-checklist.md` and `building-apps.md`.

Findings: R4, R5, R6, R11, R12, R13, plus the two ledger-dismissed re-raises above.

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
B16: fail
B17: pass
B18: pass
B19: pass

## Bob

Doubt lens (rubric D1-D5) plus de-slop, codex, static-only sandbox, resumed from his cycle-1 thread so he verified his own prior critique rather than re-reviewing from zero.

FIX: R7 (no regression asserting `extensions.code`), R8 (the accepted exception overclaims that nothing is written when an execution-time rejection happens mid-operation).

VERIFY: R9 — run `cargo test-ci` with no concurrent cargo and confirm the complete gate passes. **This check was run** (`autopilot verify`, clean and sequential at `d53521c`) and it did not pass: exit 101, 2103 passed, 1 failed. The failure is `actor::tests::shared_actor_preserves_schema_applys_owned_transaction`, which passes in isolation in 0.35s but fails inside the full ddb-server parallel lib suite. Reproducing it with no concurrent cargo refines cycle 1's reading: it is a parallel-unsafe test, not a host-load artifact. It is pre-existing and unrelated to this PRD's diff (it was already logged in PRD 00177's review history), and it is recorded as a deferred item rather than charged to 00178. The result is written back into `00178-structured-fields-input-v1-checks-2.json`.

KNOWN: none.

R1: fail
R2: pass
R3: pass
R4: pass
R6: pass
R7: pass
R8: pass
R9: fail
R10: pass
R11: pass
R12: pass
R13: pass

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

Verdict: 13 findings
Tests: 2103 passed, 1 failed (suite run this cycle; `actor::tests::shared_actor_preserves_schema_applys_owned_transaction` fails inside the full ddb-server parallel lib suite and passes in isolation in 0.35s, reproduced on a clean sequential gate run with no concurrent cargo, so it is a parallel-unsafe pre-existing test rather than a load artifact; gate exit 101 aborted at ddb-server, so later crates were not reached)
