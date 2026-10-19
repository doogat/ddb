---
prd: docs/dev/project-management/prds/wip/00178-structured-fields-input-v1.md
review: 1
date: 2026-10-19
head_sha: 74aa5eb31647fa070a05ec91c6d40c9f0b776ef3
codex_thread_id: 01a151ce-acca-7e63-80a9-dc8caf8fb111
reviewers: alice,blake,bob
agents:
  alice: available
  blake: available
  bob: available
  carl: timeout
dispatch_rows:
  bob: cdc98a08
  carl: 92bfd2bf
---

# Review: 00178-structured-fields-input-v1

Diff range: `be83c4ef1b09f0d214a19092ad21d35afd9fd7fb..74aa5eb31647fa070a05ec91c6d40c9f0b776ef3`

codex_rung_guard: not fired

## Review Summary

Reviewed: 5 completed tasks
PRDs checked: 00178-structured-fields-input-v1

### Agent Status
- Alice: ✅ Available (consensus lens, legacy engine)
- Blake: ✅ Available (blind lens, PRD-only)
- Bob: ✅ Available (codex doubt lens + de-slop, inlined prompt)
- Carl: ⚠️ Unavailable: stalled — no output file after the Watcher's full 6-run budget (~57 min); background dispatch stopped, counted as a failed reviewer per retry-policy graceful degradation

### Staging caveat (fail loud)
The `review-stage` run was killed at the harness background limit mid-gate, and a
restarted duplicate contended on the cargo build lock before being stopped. The
staged `Tests:` verdict from that run (`exit 101`) is therefore not trustworthy;
the gate was re-run cleanly via `autopilot verify` and its result is reported in
the `Tests:` line below. No reviewer output was affected.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [3/3] | 🟡 | PRD deviation: PRD 00178 requires "GraphQL String-variable operations fail schema validation" (Critical Scenarios, Error case; Phase 1 acceptance). The implementation accepts a `$f: String` variable in a JSON position and rejects only at execution, through the decoder. This is documented honestly (server.md:331, compatibility-checklist.md, and the test comment). The PRD's acceptance wording is still not met, and the deviation is not recorded as a PRD amendment or an accepted exception. Either enforce validation or amend the PRD | tests/e2e/integration_structured_fields_input.rs:266 | 5 | ALICE, BLAKE, BOB |
| R2 | [2/3] | 🟠 | GraphQL rejections of encoded-string `fields` and set/unset collisions carry no `extensions.code`, so they are not an InvalidInput/VALIDATION_ERROR. The decode helpers return a plain `String`, and the resolvers wrap it with `ServerError::new(msg, None)`. Only REST maps to `VALIDATION_ERROR` through `DoogatError::Validation`. The spec requires InvalidInput and "a structured invalid-input error"; app-contract.md claims it for GraphQL too. The e2e test only matches message text, never a code. | ddb-server/src/schema/mutations/operations.rs:31 | general | BLAKE, BOB |
| R3 | [2/3] | 🟡 | Function size: the test `fields_object_decodes_native_values_without_string_coercion` is 69 lines (limit 50). Split it into scalar, container and precision cases, or table-drive it | ddb-server/src/rest.rs:649 | 2 | ALICE, BOB |
| R4 | [1/3] | 🟠 | Integration test still sends the legacy encoded-string `fields` literal and now fails. Verified by the reviewer and re-verified in this session: `cargo test -p ddb-server --test graphql_response_warnings_t154` → `graphql_create_doogat_surfaces_title_from_template_warning` FAILED with "fields must be a JSON object, not a JSON-encoded string". The migration covered tests/e2e but missed this ddb-server integration target, which `cargo test-ci` does not run. Fix: change the literal to `fields: {url: "https://example.com"}` | ddb-server/tests/graphql_response_warnings_t154.rs:45 | 3 | ALICE |
| R5 | [1/3] | 🟡 | No test covers the singleton `update_<type>` null-to-unset threading (`unset_fields` was `vec![]`, now taken from `normalize_update_fields`) or singleton upsert null-omit. The design review flagged this twice as a blocker. Regressing that line back to `vec![]` would pass the whole suite | ddb-server/src/schema/mutations/singleton.rs:46 | 3 | ALICE |
| R6 | [1/3] | 🟡 | File-size limit: ddb-server/src/rest.rs grew from about 600 lines to 878, past the 800 limit, mostly from about 215 lines of inline unit tests. Move the new decoder tests into a sibling test module, as `mod search_tests;` already does | ddb-server/src/rest.rs:666 | 2 | ALICE |
| R7 | [1/3] | 🟡 | Non-discriminating tests: `structured_create_and_update_keep_native_number_and_bool_variants` and the e2e `ffi_create_stores_native_boolean_and_number` pass against the pre-change code, because FFI already took native scalars. They pin existing behaviour but do not show that the new `normalize_*` chokepoint is wired in | ddb-core/src/ffi/tests/structured_crud_tests.rs:438 | 1 | ALICE |
| R8 | [1/3] | 🟡 | Stale docs after the rename and delete: server.md still lists `opt_fields_map` (renamed to `opt_fields_and_unsets`), and transport-policy-inventory.md still cites the deleted `parse_fields_json` and a "JSON-string member" | docs/src/technical/server.md:83 | 5 | ALICE |
| R9 | [1/3] | 🟡 | The Phase 2 conformance task requires both GraphQL and REST to assert a native boolean. Only the GraphQL leg exists; the harness has no REST driver. The gap is documented. REST is pinned only by the standalone e2e scenario | tests/e2e/conformance/cross_driver_crud.rs:226 | general | BLAKE |
| R10 | [1/3] | 🟡 | The Risks section requires type-mismatch coercion tests ("test mismatches; fail clearly at the boundary"). None found for object `fields`: no number into a text column, no bool into a numeric column | tests/e2e/integration_structured_fields_input.rs | general | BLAKE |
| R11 | [1/3] | 🟡 | Singleton and batch resolver null handling lacks regression coverage. Helper tests cannot catch discarded unsets at resolver call sites. Add singleton update-null, singleton create-null, and batch null-plus-explicit-unset scenarios | ddb-server/src/schema/mutations/singleton.rs:44 | 3 | BOB |
| R12 | [1/3] | 🟡 | The conformance oracle cannot detect boolean/number stringification: declared column types coerce strings before both SQL and document assertions, as the assumptions ledger acknowledges. Add an untyped create/read assertion preserving `Bool` and `Number` variants | tests/e2e/conformance/cross_driver_crud.rs:225 | 4 | BOB |
| R13 | [1/3] | ⚪ | The GraphQL rejection of the encoded-string shape is a bare `ServerError::new(msg, None)` with no `extensions.code`; REST and FFI do carry a code. Low-priority parity gap (narrower restatement of R2) | ddb-server/src/schema/input.rs:70 | 3 | ALICE |
| R14 | [1/3] | ⚪ | The walkthrough exists only as the gitignored dev/local/walkthroughs/00001-structured-fields-input.md; the convention expects 00178-structured-fields-input.md, and `safe-showboat-verify` was not evidenced | N/A | 5 | ALICE |
| R15 | [1/3] | ⚪ | The e2e helper `assert_native_types` accepts either `priority: 3` or `priority: 3.0` (an either-or assert); acceptable, but it does not pin the rendering | tests/e2e/integration_structured_fields_input.rs:57 | 5 | ALICE |
| R16 | [1/3] | ⚪ | The singleton mutations changed their `input` from `String!` to `JSON!`. The spec names only generic create/update, so this widens the breaking surface beyond the spec. Documented as D-14 | ddb-server/src/schema/mutations/singleton.rs:85 | general | BLAKE |
| R17 | [1/3] | ⚪ | FFI has no null form in `FieldValue`, so null intent travels only through `unset_fields` (meets the spec). Duplicate keys in `Vec<FieldEntryValue>` collapse silently with last-wins | ddb-core/src/ffi/records.rs:434 | general | BLAKE |
| R18 | [1/3] | ⚪ | The walkthrough file is numbered 00001, not 00178, covers GraphQL only, and is gitignored (duplicate view of R14) | dev/local/walkthroughs/00001-structured-fields-input.md | general | BLAKE |
| R19 | [1/3] | ⚪ | `base_types.rs` already exceeded the 800-line limit before this change and grows further. Splitting it is outside this boundary-decoder change; record as module-maintenance debt | ddb-server/src/schema/base_types.rs:1710 | general | BOB |

## Alice

Implementation-aware consensus lens. Verified normalization logic, decoders, null
policy, collision check, batch-update semantics and singleton wiring as correct.
No security issues: no SQL or command construction, no secrets. Her one 🟠 is a
genuinely failing test target that `cargo test-ci` does not cover (R4), confirmed
independently in this session.

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
R13: fail

## Blake

Blind lens, PRD-only; located the code himself and did not re-run the suite.
Verified core normalization, transport routing, REST body types, GraphQL JSON
typing, docs/changelog coverage and the e2e scenario set against the spec.

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
B15: fail
B16: fail
B17: pass
B18: pass
B19: pass

## Bob

Doubt lens + de-slop (codex, static-only sandbox, inlined prompt).

FIX:
- GraphQL decoder and collision errors become bare `ServerError`s, bypassing the required InvalidInput classification — ddb-server/src/schema/mutations/operations.rs:64
- Legacy String-variable operations rejected during execution instead of schema validation — tests/e2e/integration_structured_fields_input.rs:269
- Singleton and batch resolver null handling lacks regression coverage — ddb-server/src/schema/mutations/singleton.rs:44
- The conformance oracle cannot detect boolean/number stringification — tests/e2e/conformance/cross_driver_crud.rs:225
- The new native-values decoder test exceeds the 50-line function limit — ddb-server/src/rest.rs:672

VERIFY:
- (none)

KNOWN:
- `base_types.rs` already exceeded the 800-line limit before this change — ddb-server/src/schema/base_types.rs:1710

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Carl

Carl: unavailable (stalled — no output after the Watcher's 6-run budget, ~57 min;
backend was copilot/gemini-3.8-flash and it spent its run contending on the cargo
build lock rather than finishing the review). Dispatch row closed as an error. A
single failed CLI reviewer does not block the cycle.

Verdict: 19 findings
Tests: 2101 passed, 1 failed (suite run this cycle; the single failure is `actor::tests::shared_actor_preserves_schema_applys_owned_transaction`, a load-induced timeout under concurrent cargo runs — re-run in isolation it passes in 0.90s; the gate aborted at ddb-server so later crates were not reached)
