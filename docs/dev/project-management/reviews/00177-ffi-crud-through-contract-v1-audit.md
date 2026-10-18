# Decision Audit Log: 00177-ffi-crud-through-contract-v1

PRD: `00177-ffi-crud-through-contract-v1.md`
Started: 2026-10-18T18:43:14Z
Completed: 2026-10-18T18:43:14Z
Autonomous: 15  |  Deferred: 25  |  Doubts: 0

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: R9 missing ffi re-exports for the seven new structured types

**Choice**: auto-fix

**Rationale**: Purely additive (adds names to an existing `pub use`); no signature, type or schema changes. Blocks PRD 00193's external callers, so it is a real PRD-acceptance gap.

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: R10 CreateResultRecord/UpdateResultRecord drop most of the ParsedDoogat payload

**Choice**: auto-fix

**Rationale**: Research-then-decide Protocol D (HIGH + public API): verdict proceed. The PRD's own acceptance text requires a lossless mirror and names the failure mode ('an id-only create or warning-only update would drop required result fields'); the maintainer decision of 2026-09-05 authorises direct replacement of the FFI surface in beta with no deprecation window, so widening the record now is cheaper than a later breaking change.

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: R11 Swift verification suite still calls the removed raw create export at 9 sites

**Choice**: auto-fix

**Rationale**: Clear mechanical fix and an explicit PRD acceptance criterion ('migrate in-repo callers', 'bindings regenerate clean'). Verified directly: rg finds 9 stale call sites in tests/swift, none in tests/kotlin.

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: R1 parser double-quoting fix has no colocated regression test

**Choice**: auto-fix

**Rationale**: Additive (adds a test). 3/4 consensus, and the repo's testing rule requires every bug fix to ship a regression test.

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: R3 either-or error-variant hedge; R13 warning loop that passes on zero warnings; R20 assertion-free degradation test; R14 over-long test functions

**Choice**: auto-fix

**Rationale**: Test-quality fixes, all additive or mechanical. R3/R13/R20 are tests that cannot fail as written, which is exactly the class the testing rule forbids.

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: R12 stale docs naming retired error variants as live; R18 compatibility checklist not updated; R15 design doc Interfaces section not updated for UpdateStructuredInput; R17 BaseOnly not recorded as the FFI default in commands.rs; R6 duplicated capsule backlog line

**Choice**: auto-fix

**Rationale**: Documentation defects inside task 5's own stated scope. R17 is a verbatim PRD success metric ('recorded as the documented FFI default in the app contract (commands.rs), NOT a constant buried in create.rs').

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: R19/R21/R23 ddb.udl [Error] interface DdbError lacks the new Conflict variant and its SearchResult is stale

**Choice**: auto-fix

**Rationale**: Raised independently by Bob, Carl and Blake, and verified directly at ddb.udl:4-16. The same diff already extended the UDL with every new record and enum, so leaving the error block behind is an inconsistency this PRD introduced.

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: R8 reviewers could not run Kotlin bindgen, the parser tests or ddb-e2e (cargo build-lock contention)

**Choice**: routed to verification

**Rationale**: Queued in 00177-ffi-crud-through-contract-v1-checks-1.json as `cargo test-ci` and `cargo build -p ddb-cli && cargo test -p ddb-e2e`; the rework pass's step-7 verification runs them, so no task is created.

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: R26 cannot statically verify that Swift/Kotlin bindings regenerate clean

**Choice**: routed to verification

**Rationale**: Queued in 00177-ffi-crud-through-contract-v1-checks-1.json; the rework pass's step-7 verification runs the gate. Alice separately ran the Swift bindgen this cycle (exit 0, generated ddb_core.swift carries createStructured/updateStructured/conflict).

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: R16 raw create_doogat_raw/update_doogat_raw helpers still public

**Choice**: discard

**Rationale**: False premise. The finding itself names live consumers (ddb-core tests, ddb-server/src/rest/search_tests.rs:491), so the PRD's 'delete helpers left with no consumers' condition is not met, and task 2's description scopes these service-level helpers explicitly out of this PRD.

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: R24 GraphQL/CLI parity tests placed in tests/e2e rather than ddb-core/src/ffi/tests.rs

**Choice**: discard

**Rationale**: Not a defect - the finder's own text concludes the placement and the warning choice are reasonable. A GraphQL parity test needs a running server and belongs in tests/e2e/.

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: Bob/codex produced no review (sandbox denied its file reads on both the initial run and the one retry)

**Choice**: graceful degradation

**Rationale**: Safety-check row 'a reviewer errored transiently': logged and continued. A Claude subagent ran Bob's exact assembled doubt prompt so the doubt lens did not drop; codex_thread_id omitted from the review file because no usable codex output exists.

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: review-stage's test gate was unusable (first attempt killed at a background limit reporting a 1800s timeout; second reported exit 101 with no summary line)

**Choice**: re-ran the suite this cycle

**Rationale**: A review must not record counts it did not obtain. cargo test-ci was re-run at HEAD 59f970f: 2057 passed, 1 failed; the single failure is the known load-sensitive ddb-server actor flake named in the session brief, re-run in isolation and green.

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: codex (Bob) published a lack-of-input refusal again (sandbox_apply: Operation not permitted)

**Choice**: skipped the one CLI retry and dispatched the Claude doubt-lens fallback directly

**Rationale**: cycle 1 proved the denial deterministic - both the initial run and its retry refused identically; the fallback runs Bob exact assembled prompt so the doubt lens did not drop

### [autonomous] 2026-10-18T18:43:14Z

**Decision**: Carl (gemini via copilot gemini-3.8-flash) ran over 50 minutes without publishing its output file

**Choice**: killed the lane and treated Carl as a failed reviewer; continued on Alice, Blake and the doubt lens

**Rationale**: retry-policy graceful degradation - a stalled CLI reviewer past the watcher budget is a failed reviewer; not an exit 4, so not latched for the batch

### [deferred] 2026-10-18T18:43:14Z

**Decision**: The `DdbError` variants `Git`, `Yaml`, `Sql`, `Automerge`, `Parse`, `Config` and `SqlEngine` are still exported but nothing produces them any more. The code, the ledger and the docs say so, but Swift/Kotlin consumers still get dead cases. Remove them in a follow-up breaking-surface PRD

**Rationale**: deferred by review-close

### [deferred] 2026-10-18T18:43:14Z

**Decision**: `core_to_field_value` and `core_to_field_scalar` are `#[cfg(test)]` only, so about nine tests in `records_tests.rs` (round trip, verbatim strings, bit-exact numbers, repeated items, native variants) exercise code that never ships. The design allowed this, but the tests cover only the `field_value_to_core` direction as production behavior. Drop the test-only direction and its tests, or keep it and say so in the file header

**Rationale**: deferred by review-close

### [deferred] 2026-10-18T18:43:14Z

**Decision**: `ddb-core/src/ffi/tests.rs` is now 1982 lines (limit 800) and this diff grew it by 20 net lines. The overage is already recorded as a deferred low, but the file was edited again rather than split. The new test files are under the limit

**Rationale**: deferred by review-close

### [deferred] 2026-10-18T18:43:14Z

**Decision**: `meta.id.map(|id| id.0).unwrap_or_default()` returns an empty `id` string if the service ever returns a doogat without an id, instead of an error. Create and update always set one today, so this only masks a future contract break

**Rationale**: deferred by review-close

### [deferred] 2026-10-18T18:43:14Z

**Decision**: `doogat_result_parts` returns a 6-tuple behind a `DoogatResultParts` alias, and two `from_output` bodies re-destructure it field by field. A private `struct DoogatResultFields` (or one constructor with `Self { .. }` built from a shared helper) is easier to read. The design doc prescribes the tuple, so this is optional

**Rationale**: deferred by review-close

### [deferred] 2026-10-18T18:43:14Z

**Decision**: `FfiVariant::NotFound` maps to `DdbError::NotFound { msg }`, which has no `code`/`context` fields, so a `Structured` `SINGLETON_NOT_FOUND`'s context is silently discarded at the boundary while Conflict/Validation keep theirs

**Rationale**: deferred by review-close

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R1 (cycle 2): DoogatService::update emits UNREGISTERED_TYPE_BASE_ONLY on every transport (CLI stderr, GraphQL/REST warnings), ungated by any policy, while the PRD Transport Impact says CLI/GraphQL/REST are unchanged. No CHANGELOG entry, docs still call the warning create-only, no service-level or non-FFI test pins it.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R2 (cycle 2): the result records extra field is not lossless and nothing documents the limit - a frontmatter map becomes FieldValue::List([]) and nested list/map items are dropped with only a tracing::warn!, so update_structured reports a wrong value rather than an omitted one.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R3 (cycle 2): the two new mirror tests are near-copies (~35 duplicated lines), update_result_record_mirrors_body_extra_sections_links_and_updated_at is 60 lines (limit 50), and neither asserts sections/links/extra are non-empty, so both go vacuous if the fixture stops producing them.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R4 (cycle 2): prior finding R14 only partly resolved - update_structured_unsets_key_and_sets_field_keeping_other_frontmatter (~65 lines), create_structured_registered_type_persists_meta_fields_and_body (~54) and update_structured_applies_new_title_tags_type_and_body still exceed the 50-line limit.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R5 (cycle 2): ffi.md:133-134 still documents CreateResultRecord/UpdateResultRecord with the old six fields; the new body/date/extra/sections/links/updated_at fields and the SectionRecord/LinkRecord/ZoneRecord/LinkKindRecord types are undocumented, and CHANGELOG.md:12 plus building-apps.md:196 describe the old shape.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R6 (cycle 2): parser change outside the PRD - serialize and serialize_yaml_value no longer route extra-field values through yaml_quote, which changes frontmatter output on every write path (CLI, GraphQL, REST, FFI); values already stored with the old quoting are not migrated.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R7/R10 (cycle 2): ddb.udl is out of sync with the Rust records - CreateResultRecord and UpdateResultRecord declare six fields against twelve in Rust, and SectionRecord, LinkRecord, ZoneRecord and LinkKindRecord are undeclared, in a file this cycle edited.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R8 (cycle 2): tests/swift/Sources/DoogatDB/ddb_core.swift is tracked although .gitignore lists it as generated; it still declares the removed createDoogat/updateDoogat and has no createStructured, while the migrated Swift tests call createStructured. Regenerated bindings were not committed.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R9 (cycle 2): docs contradict the shipped behavior - UNREGISTERED_TYPE_BASE_ONLY is documented as create-only in building-apps.md:353 and its per-interface table, app-contract.md scopes it to create, and CHANGELOG.md:12 describes only create.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R11 (cycle 2): the records are still not the lossless ParsedDoogat mirror the PRD requires - reference_section, inline_fields, body_tags and checkboxes are dropped and the doc comment hedges lossless-for-the-named-fields; reference_section cannot be re-derived from body.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R12/R13 (cycle 2): the new re-export test carries constant and no-op asserts (self-comparing assert_eq! and an empty assert_reexported<T>()) and is a weak oracle - an in-crate use resolves for pub(crate) too, so it cannot fail if the re-export stops being public.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R14 (cycle 2): stale, now-false comment and dead indirection in the e2e parity test - it still says ConflictActionRecord is not re-exported and is unnameable, and routes through ConflictAction::Error.into() with a ddb_core::types import.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R15 (cycle 2): the new update-warning branch is half-tested - no test updates an already-unregistered-typed doogat with doogat_type None and asserts zero warnings, and the service change has no colocated service-level test at all.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R16 (cycle 2): duplicated registration check - list_type_schemas() plus an any() scan now exists in both create.rs:111 and update.rs:238, and update loads the schema list a third time after the write.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R17/R21/R22 (cycle 2) low tail, test and comment quality: the try_baseonly_unregistered doc comment is now false (FFI is a second BaseOnly caller and its on_conflict is ignored there) and the create warning text changed from created to stored with no CHANGELOG entry; the new parser regression omits the # character CHANGELOG.md:53 claims round-trips; updated_at is compared as a Debug string rather than Option<String>.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R18/R23 (cycle 2): Swift/Kotlin bindgen and the hand-migrated Swift suite were not run by any reviewer, so bindings regenerate clean and the nine rewritten Swift call sites compiling are unconfirmed. Queued as a verification check (checks-2.json, dev/bin/build-xcframework); the mobile toolchain is absent on this host.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R19 (cycle 2): DdbError::Io carries the unredacted err.to_string() for Internal-category errors, while the shared error_rendering redacts those to internal error for every other transport - a documented divergence from the one error policy.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R20 (cycle 2): DoogatDriver::read_doogat returns a raw String and delete_doogat takes a message, so neither is routed through the app contract - owned by PRD 00175, noted here for traceability.

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved

### [deferred] 2026-10-18T18:43:14Z

**Decision**: R24/R25 (cycle 2): carried forward from cycle 1 verification queue - cargo test-ci exits 101 at HEAD 757c55f (2062 passed, 1 failed) because of the known load-sensitive flake actor::tests::shared_actor_preserves_schema_applys_owned_transaction, which is green when re-run in isolation but is not pinned as ignored, so the fast tier stays red under load. cargo test -p ddb-e2e passed this cycle (603 passed, 0 failed, 2 ignored).

**Choice**: deferred to batch end

**Rationale**: rework cap reached with this finding unresolved
