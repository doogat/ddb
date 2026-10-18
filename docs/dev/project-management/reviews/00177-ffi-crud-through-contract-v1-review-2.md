---
prd: docs/dev/project-management/prds/wip/00177-ffi-crud-through-contract-v1.md
review: 2
date: 2026-10-18
head_sha: 757c55f53aef3783f5646d24666dfa9468d38313
reviewers: alice,blake,bob
agents:
  alice: available
  blake: available
  bob: claude-fallback (codex sandbox refused)
  carl: failed (stalled, killed)
dispatch_rows:
  bob: fda4eab3
  carl: b85053d7
---

# Review: 00177-ffi-crud-through-contract-v1 (cycle 2)

Diff range: `59f970fcc8b502b364a9d93a0ab5adccab60ec00..757c55f53aef3783f5646d24666dfa9468d38313`

codex_rung_guard: not fired

## Cycle notes

- **Doubt lens ran on the Claude fallback, not codex — again.** `codex-run.sh` exited 0 but
  published a lack-of-input refusal (`sandbox-exec: sandbox_apply: Operation not permitted`),
  the same deterministic sandbox denial that refused twice in cycle 1 including its retry.
  The CLI retry was therefore skipped as known-futile (recorded as an autonomous decision) and
  a Claude subagent ran Bob's exact assembled doubt prompt (rubric D1-D5 + de-slop). That output
  is what the `## Bob` section and the consolidation use. The doubt lens did not drop.
  `codex_thread_id` is omitted: Bob produced no usable codex output.
- **Carl failed.** The gemini lane (copilot backend, `gemini-3.8-flash`) ran for over 50 minutes
  without publishing its `-o` file and was killed (exit 144). Treated as a failed reviewer per
  `references/retry-policy.md`; the other three lenses carried the cycle. Not latched for the
  batch (not an exit 4 configuration refusal).
- **Staging gate.** `review-stage` was first run with `--gate-command "cargo test-ci"` and was
  killed at the 10-minute background limit before rendering prompts. It was re-run with a no-op
  gate (inputs staged in 7s) and the suite was run separately in this session; see `Tests:`.
- **Cycle-1 queued checks, both resolved this cycle** (neither carried a result before):
  `cargo test-ci` → exit 101 (the single known flake, green in isolation; carried forward as
  finding R25 below), and `cargo build -p ddb-cli && cargo test -p ddb-e2e` → exit 0
  (603 passed, 0 failed, 2 ignored). The e2e check passed, so it is not carried forward.
- Mechanical test checks produced no `[MECH]` lines: both detectors are Python-only and every
  changed file in this diff is Rust, Markdown or JSONL.
- Settled-decisions ledger (8 entries) was passed to `consolidate_findings.py` with
  `--ledger-dismiss BLAKE`; three Blake re-raises were auto-dismissed (listed below). One of the
  three dismissals matched on a weak premise (the stale-comment finding), but Bob raised the same
  issue independently, so it survives as R14 — nothing was lost.
- Engram context pack: ok.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [3/3] | 🟠 | `DoogatService::update` now emits `UNREGISTERED_TYPE_BASE_ONLY` for every transport, not only FFI. The CLI (`ddb-cli/src/commands/crud.rs:106,118`) prints it to stderr, and the server actor (`ddb-server/src/actor/mod.rs:315`) and REST `{data, warnings}` return it. The PRD Transport Impact section says CLI, GraphQL and REST are "Unchanged". This was added to satisfy prior R13, but it is a shared-service change. Nothing is updated for it: CHANGELOG, `docs/src/guide/building-apps.md:353` (still says "Emitted only on the lenient create path (CLI)"), and the compat checklist. No service-level or non-FFI test pins it. Either scope the warning to FFI, or keep it and add a CHANGELOG entry, fix the docs, and add a `service/update_facade_tests.rs` case | ddb-core/src/service/update.rs:237 | 6 | ALICE, BLAKE, BOB |
| R2 | [2/3] | 🟠 | `extra` in the result records is not lossless, and nothing in the user-facing docs says so. A frontmatter value that is a map becomes `FieldValue::List([])` (`records.rs:427-430`). Nested items inside a list are dropped. Only a `tracing::warn!` records this, no warning reaches the record. `update_structured` on an existing doogat with nested frontmatter therefore reports that field as `[]`, a wrong value rather than an omitted one. AGENTS.md says user-facing paths must not silently drop data. Document the limit in `ffi.md`, or surface an `AppWarning`-style entry on the record | ddb-core/src/ffi/records.rs:441 | 6 | ALICE, BLAKE |
| R3 | [2/3] | 🟡 | The two new mirror tests are near-copies, about 35 duplicated lines. `update_result_record_mirrors_body_extra_sections_links_and_updated_at` is 60 lines, over the 50-line limit. Extract a shared `assert_record_mirrors(...)` helper taking the output and the record fields. Neither test asserts that `sections`, `links` or `extra` are non-empty in the fixture, so a fixture that stopped producing them would pass vacuously. Add `assert!(!expected_sections.is_empty())` and similar | ddb-core/src/ffi/tests/records_tests.rs:503 | 6 | ALICE, BOB |
| R4 | [2/3] | 🟡 | Prior finding R14 is only partly resolved. `assert_record_matches_stored` was extracted for two tests, but `update_structured_unsets_key_and_sets_field_keeping_other_frontmatter` (about 65 lines), `create_structured_registered_type_persists_meta_fields_and_body` (about 54) and `update_structured_applies_new_title_tags_type_and_body` still exceed 50 lines | ddb-core/src/ffi/tests/structured_crud_tests.rs:120 | 2 | ALICE, BOB |
| R5 | [1/3] | 🟡 | `docs/src/technical/ffi.md:133-134` still documents `CreateResultRecord` as `{ id, title, doogat_type, tags, path, warnings }` and `UpdateResultRecord` as "same shape". The rework added `body`, `date`, `extra`, `sections`, `links` and `updated_at`, and the new `SectionRecord`, `LinkRecord`, `ZoneRecord` and `LinkKindRecord` types are not documented anywhere. CHANGELOG.md:12 says "id, meta, path and any warnings". `building-apps.md:196` says "meta, its path". Update all three | docs/src/technical/ffi.md:133 | 6 | ALICE |
| R6 | [1/3] | 🟡 | Parser change outside the PRD: `serialize` and `serialize_yaml_value` no longer pass extra-field values through `yaml_quote`, to fix double-quoting. The fix is justified and has a changelog entry and a regression test. It still changes frontmatter output on every write path (CLI, GraphQL, REST, FFI), so it is scope creep, and values already stored with the old quoting are not migrated. | ddb-core/src/parser/mod.rs:672 | 2 | BLAKE |
| R7 | [1/3] | 🟡 | `ddb-core/src/ddb.udl` is out of sync with the Rust records. `CreateResultRecord` and `UpdateResultRecord` in the UDL lack `body`, `date`, `extra`, `sections`, `links` and `updated_at`. The commit message for 70a6386 says it synced the UDL. | ddb-core/src/ddb.udl:110 | 1 | BLAKE |
| R8 | [1/3] | 🟡 | `tests/swift/Sources/DoogatDB/ddb_core.swift` is tracked in git, although `.gitignore` lists it as a generated file. It still declares `createDoogat(content:message:)` and `updateDoogat(id:content:message:)` and has no `createStructured`, while the migrated Swift tests call `createStructured`. The regenerated bindings were not committed, and I could not confirm regeneration works. | tests/swift/Sources/DoogatDB/ddb_core.swift:547 | 4 | BLAKE |
| R9 | [1/3] | 🟡 | Docs now contradict the shipped behavior: `UNREGISTERED_TYPE_BASE_ONLY` is documented as create-only ("Emitted only on the lenient create path (CLI)") and the per-interface table lists only create paths, app-contract.md scopes it to create, and CHANGELOG.md:12 describes only the create behavior | docs/src/guide/building-apps.md:353 | 6 | BOB |
| R10 | [1/3] | 🟡 | UDL record drift reintroduced in a file this cycle edited: `CreateResultRecord`/`UpdateResultRecord` still declare six fields while the Rust records carry twelve, and `SectionRecord`, `LinkRecord`, `ZoneRecord`, `LinkKindRecord` are undeclared (UDL is documentation-only per ffi.md:326, so this is doc accuracy, not a build break) | ddb-core/src/ddb.udl:110 | 8 | BOB |
| R11 | [1/3] | 🟡 | The records are still not the lossless `ParsedDoogat` mirror the PRD requires: `reference_section`, `inline_fields`, `body_tags` and `checkboxes` are dropped and the doc comment hedges "lossless-for-the-named-fields"; `reference_section` (the three-zone reference zone) cannot be re-derived from `body`, unlike the three parse-derived collections | ddb-core/src/ffi/records.rs:566 | 6 | BOB |
| R12 | [1/3] | 🟡 | Constant and no-op asserts in the new re-export test: `assert_eq!(ConflictActionRecord::Error, ConflictActionRecord::Error)` is self-comparing and `fn assert_reexported<T>() {}` with two calls asserts nothing; the `use` statement is the only real check | ddb-core/src/ffi/tests/records_tests.rs:566 | 6 | BOB |
| R13 | [1/3] | 🟡 | That same test is a weak oracle for the defect it names: `use crate::ffi::{..}` resolves for a `pub(crate) use` too, so it cannot fail if the re-export stops being public — the behavior its doc comment claims to pin ("an external caller cannot name or construct these types") | ddb-core/src/ffi/tests/records_tests.rs:543 | 6 | BOB |
| R14 | [1/3] | 🟡 | Stale, now-false comment plus dead indirection left behind by the export fix: the e2e parity test still says `ConflictActionRecord` "is not re-exported from `ddb_core::ffi`, so it is unnameable here" and routes through `ConflictAction::Error.into()` with a `ddb_core::types::ConflictAction` import | tests/e2e/integration_ffi_warning_parity.rs:46 | 6 | BOB |
| R15 | [1/3] | 🟡 | The new update-warning branch's documented negative half is unpinned — no test updates an already-unregistered-typed doogat with `doogat_type: None` and asserts zero warnings — and the service change has no colocated service-level test at all (every case in update_facade_tests.rs passes `doogat_type: None`) | ddb-core/src/service/update_facade_tests.rs:16 | 6 | BOB |
| R16 | [1/3] | 🟡 | Duplicated registration check: `list_type_schemas()` + `any(|s| s.table_name == ty)` now exists in both create.rs:111 and update.rs:238, and `update` loads the schema list a third time after the write (update_doogat_parsed already loads it at :59 and :143) | ddb-core/src/service/update.rs:238 | 6 | BOB |
| R17 | [1/3] | ⚪ | The doc comment on `try_baseonly_unregistered` is now false. It says "The only BaseOnly caller is the CLI, which also uses Error", but FFI is a second BaseOnly caller and takes a caller-supplied `on_conflict`, which is ignored on this path. The `created` to `stored` wording change in the shared warning message also alters the CLI stderr text for create (e2e tests match the code only, so nothing breaks) | ddb-core/src/service/create.rs:97 | 6 | ALICE |
| R18 | [1/3] | ⚪ | The Swift tests now call `createStructured(title:tags:doogatType:body:fields:onConflict:)` with `.error`. The labels follow UniFFI naming, but I did not run `swift build` or `swift test` here, so compilation against regenerated bindings is unconfirmed. Earlier cycles also left the Swift snippet labels as hand-written | tests/swift/Tests/DoogatDBTests/DoogatDBTests.swift:21 | 7 | ALICE |
| R19 | [1/3] | ⚪ | `DdbError::Io` carries the unredacted `err.to_string()` for Internal-category errors. The shared `error_rendering` redacts those to "internal error" for the other transports. This is probably fine for an in-process FFI, but it is a documented divergence from the single policy. | ddb-core/src/ffi/records.rs:102 | 3 | BLAKE |
| R20 | [1/3] | ⚪ | `DoogatDriver::read_doogat` returns a raw `String` and `delete_doogat` takes a `message`. They are not routed through the contract, but that belongs to PRD 00175, not this one. | ddb-core/src/ffi/driver.rs:96 | general | BLAKE |
| R21 | [1/3] | ⚪ | The new parser regression covers `:`, `[`, `]`, `{`, `}` but not `#`, which CHANGELOG.md:53 also claims round-trips and which is the one character that opens a YAML comment; `rg` finds no frontmatter-value `#` round-trip test | ddb-core/src/parser/tests.rs:908 | 8 | BOB |
| R22 | [1/3] | ⚪ | `updated_at` is compared as a `format!("{:?}")` string on both sides instead of a direct `Option<String>` equality, which weakens the failure message for no gain | ddb-core/src/ffi/tests/records_tests.rs:496 | 6 | BOB |
| R23 | [1/3] | ⚪ | Cannot verify in this session: Swift/Kotlin bindings regenerate clean and the hand-rewritten Swift suite compiles against them (no Swift toolchain run; 9 call sites were rewritten by hand in this diff) | tests/swift/Tests/DoogatDBTests/DoogatDBTests.swift:24 | 7 | BOB |
| R24 | [1/3] | ⚪ | Cannot verify in this session: the full fast tier. The doubt lens ran `cargo test -p ddb-core --lib ffi::` — 87 passed, 0 failed, 0 ignored — but `cargo test -p ddb-core --lib` did not finish in its window. The orchestrator ran the full tier this cycle; see the `Tests:` line and R25 | N/A | general | BOB |
| R25 | [1/3] | ⚪ | Carried forward from cycle 1's verification-check queue: `cargo test-ci` exits 101 at this HEAD. 2062 passed, 1 failed, 0 skipped; the single failure is `actor::tests::shared_actor_preserves_schema_applys_owned_transaction`, the known load-sensitive timeout flake, which passes when re-run in isolation (verified this cycle). The flake is not pinned as `#[ignore]` and keeps the fast tier red under load | ddb-server/src/actor/tests/mod.rs:22 | general | verify-check |

### Auto-dismissed (ledger)

- [BLAKE] 🟡 `create_doogat_raw` and `update_doogat_raw` remain `pub` on `DoogatService`, with only test consumers. | File: ddb-core/src/service/create.rs:421 — False premise; the PRD's deletion target was the FFI `DoogatDriver` exports, and task 2 explicitly scopes these service helpers out. Live consumers exist.
- [BLAKE] 🟡 `DdbError` still declares `Git`, `Yaml`, `Sql`, `Automerge`, `Parse`, `Config` and `SqlEngine`. | File: ddb-core/src/ffi/records.rs:42 — Removing a UniFFI enum variant is a breaking binding-surface change this PRD does not authorize; deferred to batch end in cycle 1.
- [BLAKE] ⚪ Stale comment: it says `ConflictActionRecord` is not re-exported. | File: tests/e2e/integration_ffi_warning_parity.rs:46 — Dismissed against the cycle-1 test-placement entry, which is a weak match. Bob raised the same stale comment independently; it stands as R14.

## Alice

Consensus lens, implementation-aware. Nine findings (one 🟠, five 🟡, three ⚪), listed in the
consolidated table. Alice verified the prior cycle's findings individually and reports R1, R3, R6,
R9, R10 (with the `extra` caveat now raised as R2), R11, R12, R13, R15, R17, R18, R19/R21 and R20
resolved; R14 only partly resolved (R4 above). She ran
`cargo test -p ddb-core --lib -- ffi:: parser::tests::serialize_and_reparse service::update service::create`:
103 passed, 0 failed, 0 ignored. She did not run `cargo test-ci`, `ddb-e2e` or bindgen.

R1: pass
R2: pass
R3: pass
R4: pass
R6: pass
R7: pass
R8: pass
R9: fail
R10: fail
R11: pass
R12: fail
R13: fail

## Blake

Blind lens, PRD-only, no diff and no review history. Eleven findings (two 🟠, six 🟡, three ⚪);
three were auto-dismissed against the settled ledger (above), the rest are in the table. He
confirms the core work: `create_structured` and `update_structured` build `CreateCommand` and
`UpdateCommand` and return warning-carrying records, the raw FFI exports are gone,
`FFI_UNREGISTERED_TYPE_POLICY = BaseOnly` is named in the contract, the error allowlist is replaced
by policy-derived `ffi_variant`, and warning parity with GraphQL and the CLI is pinned in e2e. He
ran `cargo test -p ddb-core --lib ffi::`: 87 passed.

B1: fail
B2: fail
B3: pass
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
B15: pass
B16: fail
B17: pass
B18: pass
B19: pass

## Bob

Doubt lens + de-slop, run on the Claude fallback after codex's sandbox refused every file read
(see Cycle notes). Eighteen findings (one 🟠, nine 🟡, eight ⚪); buckets 14 FIX + 2 VERIFY +
2 KNOWN = 18. He re-raised none of the eight settled/discarded ledger items, and reports the single
🟠 as new this cycle, introduced by the rework for cycle-1 R13. Verified prior-cycle resolutions
individually (R1, R3, R4, R6, R9, R11, R12, R13, R15, R17, R18, R19/R21, R20 all resolved; R10 and
R14 partly). KNOWN: the swallowed `list_type_schemas()` error in the post-commit warning path is
deliberate and documented in the code; `service/update.rs` at 1067 lines is a pre-existing overage
of the same class as the settled `ffi/tests.rs` deferral.

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Carl

Carl: failed this cycle (gemini via copilot `gemini-3.8-flash` ran >50 min without publishing
output and was killed, exit 144). No findings. Not latched for the batch.

Verdict: 25 findings
Tests: 2062 passed, 1 failed, 0 skipped (suite run this cycle; `cargo test-ci` at HEAD 757c55f — ddb bin 47, ddb-core lib 1722, ddb-server lib 293 passed; the single failure is `actor::tests::shared_actor_preserves_schema_applys_owned_transaction`, the known load-sensitive flake, re-run in isolation this cycle and green: 1 passed, 0 failed. `cargo test -p ddb-e2e` also run this cycle: 603 passed, 0 failed, 2 ignored)
