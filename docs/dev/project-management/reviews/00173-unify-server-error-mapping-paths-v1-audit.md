# Decision Audit Log: 00173-unify-server-error-mapping-paths-v1

PRD: `00173-unify-server-error-mapping-paths-v1.md`
Started: 2026-10-17T14:08:50Z
Completed: 2026-10-17T14:08:50Z
Autonomous: 33  |  Deferred: 0  |  Doubts: 0

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: 4-way error mapper fan-out

**Choice**: Single ErrorRendering policy in app_contract::error.rs; classify/to_graphql_error_from_app/http_error_response become thin wrappers; SqlEngine arm + prefix-stripping moved into error_rendering; FFI/PgWire wiring deferred to 00177/00180

**Rationale**: per PRD 00173 Success Metrics, requiring one table every transport reads

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: `From<&DoogatError> for AppError::from` is about 117 lines and repeats the by-value impl arm for arm. That fails the 50-line limit (R12) and creates a second drift point. A comment says "keep in lockstep" and one test guards it. Simplification: make `From<DoogatError>` call `AppError::from(&err)`, or extract one shared `classify_structured(code, context)` helper. This is behavior-preserving, since the by-value path would only gain payload clones.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: The ffi_variant check in `assert_parity` is a `match` over all four variants with `{}` arms. It cannot fail at runtime and is vacuous (R2). The parity helper also compares `classify`, `to_graphql_error_from_app` and `http_error_response`, which all call `error_rendering`. Code and message equality is therefore near-tautological. The status expectation is the only discriminating assert. Fix: assert the expected `FfiVariant` per sample, and compare against literal expected code and message strings.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: CHANGELOG, errors.md and server.md say "No HTTP status changed for any existing code". That is inaccurate for unregistered Structured codes (422 to 500) and for Parse (500 to 422, with a code change). The compatibility note is also missing the legacy-GraphQL (`to_graphql_error`) side of the redaction change. Fix: make the note accurate once the two High items are resolved.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: Regression: unregistered Structured codes now render as redacted 500 "internal error" on REST/NoSQL, and on GraphQL's legacy classify path. `INVALID_ZONE` is a live, user-actionable code from SQL `CREATE TABLE ... ZONE <bad>`. It is built at ddb-core/src/sql_engine/helpers.rs:607 and is not in `codes`. Before the change, REST gave 422 with the message. Now it falls to the `_ => Internal` arm at ddb-core/src/app_contract/error.rs:239 and returns 500 "internal error". GraphQL's `to_graphql_error` path used to pass the message through and now redacts it. `http_error_t143.rs` rewrote `unknown_structured_code_maps_to_422_with_code_passthrough` to bless this. That contradicts the PRD ("no HTTP status changes on any code"). The CHANGELOG and errors.md say the same. Fix: register `INVALID_ZONE` as an InvalidInput code (422) in `codes` and in the policy, and add a REST test for it.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: There is no single table. The spec's Success Metrics and Error-case scenario say a new error code needs one edit, and that is not true here. The code→category mapping is hand-copied in `From<DoogatError> for AppError` and again in `From<&DoogatError> for AppError`. The status mapping and the Display-prefix list are two more separate `match` blocks in `error_rendering`. A new Conflict-category structured code needs edits in at least three places. Unlisted codes fall through to `Internal` and 500. The "derive existing category conversions from the sole policy table" requirement is not met, and no test covers the brand-new-code scenario.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: The GraphQL resolvers' main error path still re-decides redaction. `to_graphql_error` takes code and message from the policy, but `attach_context` (called at ddb-server/src/error.rs:29) copies the raw `Structured` context into the extensions with no redaction check. For an `Internal`-category structured code such as `SCHEMA_APPLY_PARTIAL` or an unregistered code, GraphQL gets the redacted message but still ships the unredacted context. `to_graphql_error` is used by roughly 40 call sites in `schema/queries.rs`, `discovery_queries.rs`, `singleton.rs` and `operations.rs`, while only 3 sites use `to_graphql_error_from_app`. The spec says no adapter re-decides redaction and GraphQL and REST make the same redaction decision, and this path breaks both. This path also never emits the `field` extension.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: The parity tests prove little, and the NoSQL and `ffi_variant` legs are missing. Every renderer calls the same `error_rendering` function, and `assert_parity` compares `classify` against that same function through two wrappers. A wrong shared policy would pass. NoSQL is not driven in `error_parity.rs`. The only NoSQL coverage is a 4-case unit test in nosql_api.rs:127. The ffi_variant check is an exhaustive `match` with empty arms (error_parity.rs:66-71), which asserts nothing. The spec requires "ffi_variant hint exists for each" sample and identical code and redaction across GraphQL, REST and NoSQL.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: Legacy GraphQL adapters still attach original Structured context after classify redacts the message. SCHEMA_APPLY_PARTIAL can therefore expose details that error_rendering explicitly discarded. Route to_graphql_error/to_server_error through the sanitized rendering, including context.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: The parity tests never drive the NoSQL renderer (`nosql_error`), although the file header and the task say they cover NoSQL. They also skip INVALID_PATH and BAD_REQUEST (400), TRANSACTION_NOT_SUPPORTED, PARSE_ERROR and unregistered Structured codes. Those gaps let the two regressions above through. Fix: add samples for those codes, and route at least one sample through `nosql_error`.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: The parity test never calls `to_graphql_error`, the legacy path that carries the context leak. It covers only `classify` and `to_graphql_error_from_app`, so it cannot catch the leak above.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: The docs are stale or incomplete. The server.md "Error Mapping" table still says "All others → INTERNAL_ERROR", although `Parse` is now `PARSE_ERROR` and structured codes keep their own code. The table also omits the structured codes. errors.md and the CHANGELOG do not record the `Parse` and unregistered-code changes.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: Compatibility claims omit actual status changes: Parse changes from redacted INTERNAL_ERROR/500 to exposed PARSE_ERROR/422; unknown Structured codes change from 422 to 500. Document these changes and add a Parse regression covering its intended disclosure policy.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: The conformance suite never invokes NoSQL; calling error_rendering does not exercise that adapter. Add representative NoSQL response assertions against REST, including Internal redaction and SQL_ERROR.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: From<DoogatError> duplicates entire match structure of From<&DoogatError>; delegate via AppError::from(&err)

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: `codes::SQL_ERROR` has no doc comment, unlike its neighbours (for example `TRANSACTION_NOT_SUPPORTED`). In the by-value and borrowed `Structured` arms, `SCHEMA_APPLY_PARTIAL => Internal` is redundant next to `_ => Internal`.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: Maintainability and scope notes. The two `From` impls are near-identical copies (about 120 lines) held together only by a "keep in lockstep" comment (ddb-core/src/app_contract/error.rs:167). Only `borrowed_and_owned_conversions_agree` guards them, and it covers 4 samples. The code→prefix strip table in `error_rendering` is keyed on code strings and is a fifth copy of per-code knowledge (ddb-core/src/app_contract/error.rs:338). `rest.rs` and `nosql_api.rs` still build inline `ErrorBody` values with hardcoded `NOT_FOUND` and `BAD_REQUEST` (nosql_api.rs:43-80). These sit outside the policy but match its codes, so I count it as acceptable. PgWire and FFI are untouched, which matches the spec's scoping to 00177 and 00180.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: R3/R11/R12 compatibility claims inaccurate (unregistered Structured 422->500; Parse 500 redacted INTERNAL_ERROR -> 422 exposed PARSE_ERROR); server.md Error Mapping table stale

**Choice**: auto-fix (task 10)

**Rationale**: Documentation must match shipped behavior; additive doc correction once R4/R5 land.

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: R4 unregistered Structured codes now 500 redacted on REST/NoSQL (INVALID_ZONE regression)

**Choice**: auto-fix (task 8)

**Rationale**: Protocol D (High + public API): verdict proceed. The PRD states no HTTP status changes on any code, so this is a regression against an explicit success metric, not a judgment call.

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: R5 no single table: code->category copied across two From impls; status and prefix tables separate

**Choice**: auto-fix (task 8)

**Rationale**: PRD success metric requires adding a code to touch ONE table; unmet as implemented.

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: R6/R8 legacy to_graphql_error/attach_context ships unredacted Structured context for Internal-category codes

**Choice**: auto-fix (task 11)

**Rationale**: Information disclosure on ~40 GraphQL call sites and a direct breach of the PRD rule that no adapter re-decides redaction.

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: R7/R9/R10/R13 parity tests near-tautological; NoSQL renderer and ffi_variant legs absent

**Choice**: auto-fix (task 9)

**Rationale**: Additive test work; the missing legs are what let R4 and R6 through.

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: R1/R2/R14/R15/R16 duplication of the borrowed/owned conversions, vacuous FfiVariant match, missing doc comment

**Choice**: auto-fix (tasks 8, 9)

**Rationale**: Mechanical, behavior-preserving simplifications.

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: R17 Blake did not confirm the tests pass (his tool timed out while compiling)

**Choice**: discarded

**Rationale**: Not a defect in the work. The cycle ran cargo test-ci at this HEAD: 1932 passed, 0 failed, 0 skipped. Recorded in the ledger.

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: Bob's first codex dispatch was a lack-of-input refusal (sandbox denied every file read)

**Choice**: retried once with the inlined prompt

**Rationale**: The doubt lens must not silently drop; the inlined retry produced a complete review with D1-D5 verdicts.

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: consolidate_findings.py dropped all of Bob's findings on the first run

**Choice**: normalized Bob's output and re-consolidated

**Rationale**: His FIX/VERIFY/KNOWN bucket lines carry a markdown list prefix the parser does not match; without the fix four findings including the redaction leak would have been lost.

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: 6 medium/low findings (R1,R2,R4,R5,R6,R7) are mechanical, additive or comment-only fixes

**Choice**: auto-fix via tail sweep after convergence

**Rationale**: no CRITICAL or HIGH survived cycle 2 and the doubt-roster constraint gate certified, so the cycle converged; the medium/low tail is swept by normal work tasks rather than another review cycle

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: Tests that can no longer fail. `From<DoogatError>` now delegates to `From<&DoogatError>`, so `borrowed_and_owned_conversions_agree_for_every_simple_variant`, `..._for_every_structured_code` and `..._for_field_and_context_bearing_errors` compare a function to itself; they pass against the pre-change code too. `to_graphql_error` is literally `to_graphql_error_from_app(AppError::from(e))`, so `legacy_and_app_graphql_paths_agree_on_message_and_extensions`, `..._on_every_policy_code` and `..._on_every_structured_constructor` are the same. About 120 lines of tests assert an invariant that holds by construction. Simplification: delete them, keeping the literal-value tests (`legacy_graphql_error_on_*`, `legacy_graphql_error_exposes_field_hint_only_where_the_policy_has_one`).

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: Conversion-equivalence tests now compare the borrowed conversion with an owned conversion that delegates directly to it, so they cannot detect independent drift. Remove the expanded equivalence helpers and loops; retain literal assertions for code, category, field, and details.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: No guard that every `codes::*` constant is registered in `ERROR_POLICY`. An unregistered code silently becomes a redacted 500, and that was exactly the INVALID_ZONE regression. The only enumeration is `ALL_POLICY_CODES`, a hand-copied list of the private table in the server test crate (ddb-server/src/error/tests.rs:497). It can drift from the table without failing, and it does not catch a new constant missing from the table. Fix: add a core test that checks each `codes` constant (or a `codes::ALL` array) resolves through `policy_for`, and that `ERROR_POLICY` has no duplicate codes.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: No test checks that every `codes::*` constant has a row in ERROR_POLICY. The code lists in the tests are hand-written, so a new code without a row silently becomes Internal/500. That fails closed, but it leaves the "touch ONE table" promise unenforced.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: The header of the moved test file says "the tests are unchanged", but the diff adds many tests there (INVALID_ZONE, the unregistered-code redaction and the code-driven rendering test). The comment is inaccurate.

**Choice**: auto-fixed

**Rationale**: fixed by review-close

### [autonomous] 2026-10-17T14:08:50Z

**Decision**: `unregistered_code_minted_at_runtime` builds a time-based string and uses `Box::leak`, which makes the test non-deterministic. A fixed second unregistered literal proves the same rule, as the neighbouring fixed codes already show.

**Choice**: auto-fixed

**Rationale**: fixed by review-close
