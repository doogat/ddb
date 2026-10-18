---
prd: docs/dev/project-management/prds/wip/00177-ffi-crud-through-contract-v1.md
review: 1
date: 2026-10-18
head_sha: 59f970fcc8b502b364a9d93a0ab5adccab60ec00
reviewers: alice,blake,bob,carl
agents:
  alice: available
  blake: available
  bob: available
  carl: available
dispatch_rows:
  bob: caaa6863
  carl: d47485bd
---

# Review: 00177-ffi-crud-through-contract-v1

Diff range: `235edf0836ef851aea95a91768d792a1c0b6648d..59f970fcc8b502b364a9d93a0ab5adccab60ec00`

codex_rung_guard: not fired

## Cycle notes

- **Doubt lens ran on the Claude fallback, not codex.** Bob/codex was dispatched twice
  (initial + one retry per `references/retry-policy.md`). Both runs returned a
  lack-of-input refusal: `sandbox_apply: Operation not permitted` — the codex sandbox
  could not read the staged context/diff/pack files, so no source review was performed.
  Both refusals carried blanket `R{n}: fail` lines, which are artifacts of the refusal and
  carry no review signal. A Claude subagent then ran Bob's exact assembled doubt prompt
  (doubt rubric D1-D5 + de-slop), and that output is what this file's `## Bob` section and
  the consolidation use. The doubt lens did not drop. `codex_thread_id` is omitted because
  Bob produced no usable codex output this cycle.
- **Staging gate.** `autopilot review-stage` ran `cargo test-ci` and reported
  `exit 101, no PASS/FAIL/SKIP/EXIT summary line` (its first attempt was killed at a
  background time limit and had reported a 1800s timeout). Neither result is usable, so
  the suite was re-run in this session; see the `Tests:` line.
- No settled-decisions ledger exists (cycle 1), so `consolidate_findings.py` ran without
  `--ledger`/`--ledger-dismiss`.
- Mechanical test checks produced no `[MECH]` lines: both detectors are Python-only and
  every changed file in this diff is Rust, Markdown or JSONL.
- Engram context pack: ok.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [3/4] | 🟡 | The parser fix is a behavior change outside the FFI surface: it removes the double quoting in `serialize` and `serialize_yaml_value`, so it affects every writer. It has no parser-level regression test. Coverage is indirect, through the FFI test `create_structured_stores_num_bool_list_and_quoted_field_values` and the reordered e2e `fix_verbose_reports_title_noncompliant`. Add a `parser/tests.rs` case that serializes and re-parses an extra field containing `:`, `[` and `{` and asserts the value is unchanged | ddb-core/src/parser/mod.rs:672 | 2 | ALICE, BLAKE, BOB |
| R2 | [3/4] | 🟡 | The `DdbError` variants `Git`, `Yaml`, `Sql`, `Automerge`, `Parse`, `Config` and `SqlEngine` are still exported but nothing produces them any more. The code, the ledger and the docs say so, but Swift/Kotlin consumers still get dead cases. Remove them in a follow-up breaking-surface PRD | ddb-core/src/ffi/records.rs:41 | 3 | ALICE, BLAKE, BOB |
| R3 | [2/4] | 🟡 | `update_structured_on_an_unknown_id_errors_without_creating_a_doogat` accepts `NotFound \| Validation` through an either-or match. The variant is deterministic under the policy-derived mapping, so the hedge can hide a mapping regression. Pin the one expected variant (the policy makes a missing id `NotFound`) | ddb-core/src/ffi/tests/structured_crud_tests.rs:2221 | 2 | ALICE, BOB |
| R4 | [2/4] | 🟡 | `core_to_field_value` and `core_to_field_scalar` are `#[cfg(test)]` only, so about nine tests in `records_tests.rs` (round trip, verbatim strings, bit-exact numbers, repeated items, native variants) exercise code that never ships. The design allowed this, but the tests cover only the `field_value_to_core` direction as production behavior. Drop the test-only direction and its tests, or keep it and say so in the file header | ddb-core/src/ffi/records.rs:398 | 1 | ALICE, BOB |
| R5 | [2/4] | 🟡 | `ddb-core/src/ffi/tests.rs` is now 1982 lines (limit 800) and this diff grew it by 20 net lines. The overage is already recorded as a deferred low, but the file was edited again rather than split. The new test files are under the limit | ddb-core/src/ffi/tests.rs:1 | 2 | ALICE, BOB |
| R6 | [2/4] | 🟡 | The capsule edit adds a duplicated backlog line, `- [ ] 00179-sql-identifier-escaping-materializer-v1` appears twice | docs/dev/project-management/meta/project-capsule.md:88 | general | ALICE, BOB |
| R7 | [2/4] | ⚪ | `meta.id.map(\|id\| id.0).unwrap_or_default()` returns an empty `id` string if the service ever returns a doogat without an id, instead of an error. Create and update always set one today, so this only masks a future contract break | ddb-core/src/ffi/records.rs:502 | 1 | ALICE, BLAKE |
| R8 | [2/4] | ⚪ | Verification notes: `cargo test -p ddb-core --lib ffi::` passed 83 of 83; the Swift bindgen run exited 0 and the generated `ddb_core.swift` contains `createStructured`, `updateStructured` and the `conflict` error case. Kotlin bindgen, the parser tests and `ddb-e2e` were not run by the reviewers (cargo build-lock contention), so task 5's "e2e green" claim is unconfirmed by them | N/A | 4 | ALICE, BOB |
| R9 | [1/4] | 🟠 | `CreateResultRecord`, `UpdateResultRecord`, `FieldEntryValue`, `FieldValue`, `FieldScalar`, `ConflictActionRecord` and `UpdateStructuredInput` are not re-exported from `ddb_core::ffi`, while every earlier record is, per the file's own "re-exports keep paths stable" rule. Rust callers outside the crate cannot construct `UpdateStructuredInput` or `FieldEntryValue`, so they cannot call `update_structured` or pass fields to `create_structured`. The e2e test works around this with `ConflictAction::Error.into()` and a comment saying the type is unnameable, and PRD 00193 plans to drive these exports from outside the crate. Add the seven names to the `pub use records::{..}` list | ddb-core/src/ffi/mod.rs:14 | 2 | ALICE |
| R10 | [1/4] | 🟠 | CreateResultRecord/UpdateResultRecord are lossy, not the lossless mirror the PRD requires. They carry only id, title, doogat_type, tags, path and warnings. The core payload (ParsedDoogat from DoogatService::create/update) also has body, meta.date, meta.extra (the structured fields just written), sections, links, updated_at and more, and all of it is dropped. The PRD says "an id-only create or warning-only update would drop required result fields". The CHANGELOG itself says "id, meta, path". A caller cannot see the stored fields, body or date of a create or update result. | ddb-core/src/ffi/records.rs:480 | 1 | BLAKE |
| R11 | [1/4] | 🟠 | In-repo callers were not migrated. The Swift verification suite still calls the removed `driver.createDoogat(content:message:)` at 9 sites (lines 30, 47, 62, 85, 96, 254, 282, 319, 320), so it no longer compiles against regenerated bindings. The diff touched nothing under tests/swift, so "bindings regenerate clean" and "migrate in-repo callers" are unmet. tests/swift/Sources/DoogatDB/Exports.swift and the README are also untouched. | tests/swift/Tests/DoogatDBTests/DoogatDBTests.swift:30 | 2 | BLAKE |
| R12 | [1/4] | 🟡 | Stale docs still describe the retired error variants as live, although task 5 required documenting the collapse. `ffi.md:319` says import merge failures surface as `DdbError::Git` with a `bundle merge failed:` message, but they now arrive as `Io` with a `sync: bundle merge failed:` prefix. `bundle.md:98` makes the same `DdbError::Git` claim in the FFI row. `building-apps.md:101` says `catch DdbError.sql` "remains supported" and `:282` still lists `DdbError::SqlEngine`. The Swift error-handling example at `building-apps.md:576` catches `.sql` and never shows `.conflict`. `walkthrough.md:817` still says FFI surfaces `SqlEngine` | docs/src/technical/ffi.md:319 | 5 | ALICE |
| R13 | [1/4] | 🟡 | `update_structured_unsets_key_and_sets_field_keeping_other_frontmatter` only loops over `rec.warnings`, so the loop passes with zero warnings. No test shows `update_structured` forwarding a real warning end to end (the PRD edge case "the record carries any resulting warning"). Only the hand-built `from_output` unit test covers the mapping. Add one update fixture that produces a warning | ddb-core/src/ffi/tests/structured_crud_tests.rs:2077 | 2 | ALICE |
| R14 | [1/4] | 🟡 | Several test functions exceed the 50-line limit. `create_structured_registered_type_persists_meta_fields_and_body` is 57 lines and `update_structured_unsets_key_and_sets_field_keeping_other_frontmatter` is 65. `update_structured_applies_new_title_tags_type_and_body` is 55 and the e2e `ffi_and_cli_report_identical_unregistered_type_base_only_warning` is 53. Extract assertion helpers such as `assert_record_matches_stored` | ddb-core/src/ffi/tests/structured_crud_tests.rs:1754 | 2 | ALICE |
| R15 | [1/4] | 🟡 | `update_structured` takes an `UpdateStructuredInput` record instead of the flat argument list in the PRD design contract and in task 5's old-to-new mapping. The reason (clippy `too_many_arguments`, 8/7) appears only in the assumptions ledger. The CHANGELOG and `ffi.md` document the final shape correctly. The design doc's Interfaces section was not updated | ddb-core/src/ffi/driver.rs:100 | 2 | ALICE |
| R16 | [1/4] | 🟡 | The raw helpers `create_doogat_raw` and `update_doogat_raw` are still public. No production code calls them any more. The remaining callers are ddb-core tests (raw_ffi_semantics.rs and others) and ddb-server/src/rest/search_tests.rs:491. The PRD says to delete helpers left with no consumers. The doc comment still says "Unknown types are accepted (legacy FFI contract)", which is the silent policy the PRD wanted gone. Those tests pin the legacy behaviour and keep it alive as library API. | ddb-core/src/service/create.rs:428 | 2 | BLAKE |
| R17 | [1/4] | 🟡 | The FFI default policy is only a doc-comment sentence on the enum variant ("FFI `create` is the second consumer"). `BaseOnly` is hard-coded inline in the driver. No named constant or `CreateCommand` default is recorded in the contract. The documented rationale for differing from GraphQL/REST (`Strict`) is not in commands.rs. The PRD asked for BaseOnly to be named as the FFI default with a doc comment in commands.rs. | ddb-core/src/app_contract/commands.rs:19 | 1 | BLAKE |
| R18 | [1/4] | 🟡 | The compatibility checklist was not updated, although the PRD's maintainer decision names it. docs/src/technical/compatibility-checklist.md has no FFI create/update or error-variant entries (the "FFI" matches are search-only). The PRD's explicit docs list (app-contract.md, ffi.md, conformance-harness.md, CHANGELOG) was updated. | docs/src/technical/compatibility-checklist.md:78 | 5 | BLAKE |
| R19 | [1/4] | 🟡 | `ddb-core/src/ddb.udl` was extended with every new record/enum but its `[Error] interface DdbError` was not: the new `Conflict(string msg, string? code, sequence<DdbErrorContextEntry> context)` variant is missing while retired `Git`/`Yaml`/`Sql`/`Automerge`/`Parse`/`Config`/`SqlEngine` remain, so the UDL now describes an error surface the Rust side cannot produce and omits the one it can | ddb-core/src/ddb.udl:3 | 5 | BOB |
| R20 | [1/4] | 🟡 | `core_top_level_map_degrades_without_panicking` has no assertion (`let _ = core_to_field_value(...)`); it can only fail on a panic, so the documented "top-level map degrades to an empty list" contract is unpinned even though the doc comment states it | ddb-core/src/ffi/tests/records_tests.rs:1182 | 1 | BOB |
| R21 | [1/4] | 🟡 | DdbError in ddb.udl is missing the Conflict variant added to records.rs | ddb-core/src/ddb.udl:15 | 5 | CARL |
| R22 | [1/4] | ⚪ | `doogat_result_parts` returns a 6-tuple behind a `DoogatResultParts` alias, and two `from_output` bodies re-destructure it field by field. A private `struct DoogatResultFields` (or one constructor with `Self { .. }` built from a shared helper) is easier to read. The design doc prescribes the tuple, so this is optional | ddb-core/src/ffi/records.rs:489 | 1 | ALICE |
| R23 | [1/4] | ⚪ | ddb.udl is documentation only (ffi.md:326), and it lacks the new `Conflict` variant. Its `SearchResult` is also stale (6 fields instead of the 10-field record). | ddb-core/src/ddb.udl:3 | 1 | BLAKE |
| R24 | [1/4] | ⚪ | The GraphQL/CLI parity tests live in tests/e2e/integration_ffi_warning_parity.rs, not in ddb-core/src/ffi/tests.rs as the PRD's structure map says. The GraphQL parity check uses the TITLE_FROM_TEMPLATE warning, because GraphQL rejects unregistered types under `Strict`. This is reasonable. The CLI parity check covers `BaseOnly` code and message. | tests/e2e/integration_ffi_warning_parity.rs:66 | 4 | BLAKE |
| R25 | [1/4] | ⚪ | `FfiVariant::NotFound` maps to `DdbError::NotFound { msg }`, which has no `code`/`context` fields, so a `Structured` `SINGLETON_NOT_FOUND`'s context is silently discarded at the boundary while Conflict/Validation keep theirs | ddb-core/src/ffi/records.rs:320 | 3 | BOB |
| R26 | [1/4] | ⚪ | Cannot statically verify: Swift/Kotlin bindings regenerate clean (task 4 acceptance); the assumptions ledger states the Swift snippet's argument labels were written by hand and bindgen was not run | docs/src/guide/building-apps.md:2671 | 4 | BOB |

### Orchestrator spot-checks

Three findings were verified directly before classification:

- **R9 confirmed.** `ddb-core/src/ffi/mod.rs:14-18` re-exports fourteen names; none of the
  seven new types is among them.
- **R11 confirmed.** `rg` over `tests/swift` returns 9 call sites of the removed raw create
  export; `tests/kotlin` has none.
- **R19/R21/R23 confirmed.** `ddb.udl:4-16` declares `Git`, `Yaml`, `Sql`, `Automerge`,
  `Io`, `Parse`, `NotFound`, `Config`, `Validation`, `SqlEngine`, `VersionMismatch` — no
  `Conflict`.

## Alice

Consensus lens, implementation-aware. Thirteen findings (one 🟠, eight 🟡, four ⚪), listed
in the consolidated table above. Alice independently ran `cargo test -p ddb-core --lib ffi::`
(83 passed, 0 failed) and the Swift bindgen (`ddb-uniffi-bindgen --language swift`, exit 0;
generated `ddb_core.swift` contains `createStructured`, `updateStructured` and the
`conflict` error case).

R1: pass
R2: pass
R3: pass
R4: fail
R6: pass
R7: pass
R8: pass
R9: pass
R10: pass
R11: pass
R12: fail
R13: fail

## Blake

Blind lens, PRD-only: no diff, no file list, no design doc, no review history. Ten findings
(two 🟠, five 🟡, three ⚪). Blake confirmed the core PRD mechanics independently — structured
create/update build the contract commands, `BaseOnly` is applied, `on_conflict` is exposed,
native typed inputs convert through the private `FieldValue`/`FieldScalar` enums, the raw
FFI exports are gone, and the error mapping derives from `error_rendering(..).ffi_variant`
with `SINGLETON_NOT_FOUND` now mapping to `NotFound`. He ran `cargo test -p ddb-core --lib
ffi::` (83 passed, 0 failed) and did not run e2e or bindgen.

B1: fail
B2: fail
B3: fail
B4: pass
B5: pass
B6: fail
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

Doubt lens (D1-D5) + de-slop, run on the Claude fallback after codex refused twice — see
Cycle notes. Twelve findings (five 🟡, seven ⚪) plus explicit FIX / VERIFY / KNOWN buckets.

FIX:
- Parser quoting fix has no colocated regression test — ddb-core/src/parser/mod.rs:672 — add a `serialize`/round-trip unit test in `parser` for a value containing `:`/`[`/`#`, asserting a single level of quoting.
- UDL error enum missing `Conflict` and still listing retired variants — ddb-core/src/ddb.udl:3 — add the `Conflict(string msg, string? code, sequence<DdbErrorContextEntry> context)` line (and decide, in one place, whether the retired variants stay declared).
- Either-or hedge on the unknown-id update error — ddb-core/src/ffi/tests/structured_crud_tests.rs:2222 — assert the single variant the policy produces for `DoogatError::NotFound`.
- Assertion-free degradation test — ddb-core/src/ffi/tests/records_tests.rs:1182 — assert `core_to_field_value(Value::Map(..)) == FieldValue::List(vec![])`, the behavior the doc comment promises.
- Duplicated backlog entry — docs/dev/project-management/meta/project-capsule.md:2598 — delete the repeated `00179-...` line.

VERIFY:
- Bindings regenerate clean and the documented Swift labels match — run the repo's bindgen and diff the generated Swift signature of `createStructured` against the snippet in `docs/src/guide/building-apps.md`.
- Test suite status is unknown (timeout, then exit 101 with no summary) — run `cargo test-ci` and `cargo build -p ddb-cli && cargo test -p ddb-e2e` and report the pass/fail counts.

KNOWN:
- `core_to_field_value`/`core_to_field_scalar` are test-only — the design doc explicitly sanctioned `#[cfg(test)]` over shipping unused production code; removing them would drop the round-trip proof.
- Retired `DdbError` variants left declared — removing a UniFFI enum variant is a separate breaking-surface decision the PRD did not authorize.
- `NotFound` drops structured context — `DdbError::NotFound` has no context field; adding one is a binding-shape change outside this PRD.
- `ffi/tests.rs` over 800 lines — pre-existing, already recorded in the capsule for 00176; splitting the parent file is unrelated to this PRD's behavior.
- Parser quoting change spans all transports — it is the genuine root-cause fix the FFI typed-field test exposed; fixing it only on the FFI path would have been wrong.
- Field-key/type charset is not validated at the FFI write boundary — same shape as the existing CLI/GraphQL paths; tracked by 00179 sql-identifier-escaping.

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
R13: fail

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Carl

Frontend & design specialist lens (gemini via copilot backend). One finding: the UDL's
`DdbError` is missing the `Conflict` variant added to `records.rs` (R21). This PRD has no
UI surface, so Carl ran as a generalist.

Verdict: 26 findings
Tests: 2057 passed, 1 failed, 0 skipped (suite run this cycle; `cargo test-ci` at HEAD 59f970f — ddb bin 47, ddb-core lib 1717, ddb-server lib 293 passed; the single failure is `actor::tests::shared_actor_preserves_schema_applys_owned_transaction`, the known load-sensitive flake named in the session brief, re-run in isolation this cycle and green: 1 passed, 0 failed)
