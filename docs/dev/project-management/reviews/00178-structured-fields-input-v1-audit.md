# Decision Audit Log: 00178-structured-fields-input-v1

PRD: `00178-structured-fields-input-v1.md`
Started: 2026-10-19T07:28:38Z
Completed: 2026-10-19T07:28:38Z
Autonomous: 22  |  Deferred: 2  |  Doubts: 0

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: fields input is a stringified JSON blob, forcing downstreams to stringify booleans/numbers

**Choice**: reuse existing JSON scalar for fields input, new core normalize_create_fields/normalize_update_fields in app_contract for null-policy (create-omit, update-unset), REST accepts serde_json::Value object, GraphQL fields type String->JSON on 3 generic inputs plus both singleton mutations, FFI unchanged

**Rationale**: PRD requires native object fields; 2 review dispatches fixed singleton input-type mismatch and singleton update dropping null-derived unsets before planning

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: PRD deviation: PRD 00178 requires legacy GraphQL String-variable operations to fail schema validation; the dynamic schema accepts a String variable in a JSON position and rejects only at execution inside the decoder. Record the deviation explicitly as an accepted PRD exception (PRD + compatibility checklist), or enforce validation. Flagged by all three lenses.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: GraphQL rejections of encoded-string fields and set/unset collisions carry no extensions.code, so they are not InvalidInput/VALIDATION_ERROR. Route the decode/collision errors through DoogatError::Validation and the existing GraphQL error mapping so they carry VALIDATION_ERROR, and assert the code in a test. REST already does this; docs already claim GraphQL does.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: The test fields_object_decodes_native_values_without_string_coercion is 69 lines, past the 50-line limit. Split it into scalar, container and precision cases, or table-drive it.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: Unmigrated test left broken by this PRD: graphql_create_doogat_surfaces_title_from_template_warning fails with 'fields must be a JSON object, not a JSON-encoded string'. Confirmed by running cargo test -p ddb-server --test graphql_response_warnings_t154. Change the literal to object form, and sweep ddb-server/tests/ for any other encoded-string fields literal that cargo test-ci does not cover.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: R4: unmigrated encoded-string fields literal in ddb-server/tests/graphql_response_warnings_t154.rs leaves a failing test

**Choice**: auto-fix (rework task); verified failing in this session by running the target directly

**Rationale**: mechanical test migration the PRD's own change requires; cargo test-ci does not cover this target

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: R2: GraphQL fields-decode and set/unset-collision errors carry no extensions.code, so they are not InvalidInput/VALIDATION_ERROR

**Choice**: research-then-decide Protocol D (public API shape), verdict proceed -> auto-fix (rework task)

**Rationale**: the docs and compatibility checklist already promise VALIDATION_ERROR on GraphQL, and the repo's One-error-policy coherence invariant requires transports to derive from the single mapping; adding the code closes a documented gap rather than changing a promised shape

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: R1: PRD requires legacy GraphQL String-variable operations to fail schema validation; they fail at execution instead (3/3 consensus)

**Choice**: auto-fix (rework task): record the deviation as an explicit accepted PRD exception, or enforce validation

**Rationale**: already documented in server.md and the compatibility checklist but never recorded as an accepted PRD exception; cheapest correct close is to make the exception explicit in the PRD

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: R5-R19: fifteen single-voice Medium/Low findings

**Choice**: discarded, recorded in the settled-decisions ledger with reason single-voice

**Rationale**: Phase 5 rule: a Medium or Low raised by one reviewer of three is discarded, never a task; ledgered so a re-raise is auto-dismissed next cycle

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: Carl (gemini) produced no output after the Watcher's full budget (~57 min)

**Choice**: logged, continued with Alice, Blake and Bob (graceful degradation)

**Rationale**: Safety Checks: a single transient reviewer failure must not break an unattended run; three lenses including blind and doubt completed

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R2: The R2 fix is incomplete. The singleton `update_<type>` and `upsert_<type>` resolvers still wrap decode errors in a bare `async_graphql::ServerError::new(msg, None)`. The same legacy encoded-string rejection therefore still carries no `extensions.code` on the singleton mutations, while the generic mutations now return VALIDATION_ERROR. Use `to_graphql_error(DoogatError::Validation(msg))` at both sites.

**Choice**: discard

**Rationale**: single-voice Medium/Low: discarded per the Classification single-voice rule

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R3: The set/unset collision message gets a doubled prefix. `fields_and_unsets` does `.map_err(|e| e.to_string())`, so the string already starts with "validation: ". The resolver then re-wraps it in `DoogatError::Validation`, and `error_rendering` strips the prefix only once.

**Choice**: discard

**Rationale**: single-voice Medium/Low: discarded per the Classification single-voice rule

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R5: FFI has no way to send a null for a field value. `FieldValue` has only Str, Num, Bool and List, and the driver wraps every entry in `Some`. Update null intent works only through `unset_fields`; on create, a caller omits the key.

**Choice**: discard

**Rationale**: single-voice Medium/Low: discarded per the Classification single-voice rule

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R6: Large integers are silently coerced to f64 and lose precision above 2^53, on both REST and GraphQL. The PRD's coercion risk says to "fail clearly at the boundary".

**Choice**: discard

**Rationale**: single-voice Medium/Low: discarded per the Classification single-voice rule

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R7: Task 6 fixes error mapping but adds no regression asserting `extensions.code`. Existing message-only tests would pass if the fix were reverted.

**Choice**: discard

**Rationale**: single-voice; same defect as R1, which is deferred as the cycle's unresolved HIGH

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R8: The accepted exception incorrectly claims execution-time rejection has an identical effect and "nothing is written." Earlier fields in a multi-mutation operation can already have committed.

**Choice**: discard

**Rationale**: single-voice Medium/Low: discarded per the Classification single-voice rule

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R9: The supplied gate exited 101 before later crates ran; the isolated timeout-test pass does not complete validation. Run `cargo test-ci` without concurrent cargo processes and confirm the complete gate exits successfully.

**Choice**: discard

**Rationale**: single-voice; the named check was run this cycle and its result is recorded in checks-2.json and deferred as a verify-escape

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R10: Formatting drift: the new lines are over 100 columns, so rustfmt would rewrap them. A small helper such as `validation_error(msg)` would remove the four repeated closures and fix the line length.

**Choice**: discard

**Rationale**: single-voice Medium/Low: discarded per the Classification single-voice rule

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R11: The REST fields-decode error echoes the entire rejected JSON value back to the client. For a large string payload the error body can be large.

**Choice**: discard

**Rationale**: single-voice Medium/Low: discarded per the Classification single-voice rule

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R12: Some docs still describe the old behavior: transport-policy-inventory.md still cites `parse_fields_json`, and building-apps.md still carries the "(A JSON string until PRD 00178)" note.

**Choice**: discard

**Rationale**: single-voice Medium/Low: discarded per the Classification single-voice rule

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R13: I did not run Swift/Kotlin bindgen, `cargo test-ci`, or the e2e suite, so the Phase 1 "bindgen succeeds" acceptance and the e2e greenness are unverified by me.

**Choice**: discard

**Rationale**: single-voice; a self-declared coverage limit of the blind reviewer, not a defect in the work

### [autonomous] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R4: Phase 2 conformance create-with-typed-fields step exists for GraphQL only

**Choice**: discard

**Rationale**: settled deferral from cycle 1 (REST conformance leg awaits PRD 00175)

### [deferred] 2026-10-19T07:28:38Z

**Decision**: [cycle 2] R1: the GraphQL VALIDATION_ERROR mapping added in task 6 is untested - no test asserts extensions.code == VALIDATION_ERROR on any GraphQL path, so reverting the four to_graphql_error call sites in ddb-server/src/schema/mutations/operations.rs to ServerError::new(msg, None) would still pass every test. Alice also found the same gap unfixed in the singleton update/upsert resolvers (ddb-server/src/schema/mutations/singleton.rs:45 and :99).

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-19T07:28:38Z

**Decision**: queued check failed: cargo test-ci -> exit 101. actor::tests::shared_actor_preserves_schema_applys_owned_transaction fails inside the full ddb-server parallel lib suite (2103 passed, 1 failed) and passes in isolation in 0.35s. Reproduced on a clean sequential gate run with no concurrent cargo, so it is a parallel-unsafe test rather than a host-load artifact. Pre-existing and unrelated to the 00178 diff; already logged in PRD 00177 review history. The gate aborts at ddb-server, so later crates are never reached by cargo test-ci.

**Rationale**: cycle-2 verification check did not pass; pre-existing parallel-unsafe test, out of scope for 00178
