---
prd: docs/dev/project-management/prds/wip/00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md
review: 2
date: 2026-10-20
head_sha: 10bbf7d95dc9f017f51c70bb23bbca1b420b38a9
codex_thread_id: 01a159ee-f6e7-7e82-ac4c-8f01fe033f65
reviewers: alice,blake,bob
agents:
  alice: available
  blake: available
  bob: available
  carl: unavailable
dispatch_rows:
  bob: 241d4895
  carl: 63d46edc
---

# Review: 00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1

Diff range: `64f7c42ac31ebbb6e7deef3671f4861292563088..10bbf7d95dc9f017f51c70bb23bbca1b420b38a9`

codex_rung_guard: not fired

pack: ok

## Review Summary

Reviewed: 9 completed tasks (incremental review of the cycle-1 rework, 784fce4..c6fad8a)
PRDs checked: 00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1

### Agent Status

- Alice: ✅ Available (consensus lens, legacy engine; incremental verification of the 24 cycle-1 findings)
- Blake: ✅ Available (blind lens, PRD-only)
- Bob: ✅ Available (doubt lens, codex, resumed thread 01a159ee; D1-D5 emitted)
- Carl: ⚠️ Unavailable: `gemini-run` backend=copilot model=gemini-3.8-flash exited 1, "You have exceeded your monthly quota". `[RETRY] carl attempt 1/1` hit the same monthly quota wall. Exit 1 is a runtime/quota failure, not exit 4, so Carl is NOT latched unavailable for the batch.
- Eve: ⏸️ Disabled: resolved doubt reviewer is `codex` and the codex doubt-roster guard did not fire (no task has a codex implementor).

### Method notes

- Consensus engine: `legacy` (single Alice subagent).
- Incremental cycle: diff scoped with `--since 64f7c42`; Alice, Bob and Carl received the prior cycle's consolidated findings and the settled-decisions ledger. Blake stayed blind (PRD-only).
- Consolidation ran via `consolidate_findings.py` over three reviewers with `--ledger ... --ledger-dismiss BLAKE`; two Blake re-raises were auto-dismissed (listed below).
- Mechanical test checks contributed 0 findings again: `compute_mech_facts.py` / `detect_tautological_tests.py` are python-only and checked 0 test functions in 0 test files across this Rust diff. The fail-first replay was likewise inapplicable. Alice's R6 and Bob's R12 are model-side tautology findings, not computed ones.
- Carry-forward: cycle 1's single queued check (`cargo test -p ddb-server --lib actor::tests::shared_actor_preserves_schema_applys_owned_transaction -- --exact`) was run at this HEAD and exited 0 (1 passed). Its `result` is recorded in `…-checks-1.json`, so nothing carried forward as a finding.
- Verification-check queue for cycle 2: empty. Bob's VERIFY bucket is `(none)` and Eve did not run, so no `…-checks-2.json` was written.
- `Tests:` line composed by hand from the gate log, as in cycle 1: `verification.run_gate`'s parser does not understand cargo's `test result:` summaries, so the staged section reads `counts unavailable (suite run this cycle, exit 0, …)`. Counts below are read directly from `docs/dev/tmp/gate-00184-c2.log` (47 + 1780 + 321 passed, 0 failed, exit 0). The cycle-1 actor-test failure is gone.
- `review-stage`'s own gate attempt was skipped ("another gate held the lock for 600s"); the full-suite run that produced the counts is the one whose log is named above, at this same HEAD with a clean tree for the source files under review.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [1/3] | 🟠 | Phase 2 acceptance ("conformance asserts add/drop/reapply outcomes on CLI/GraphQL/REST and FFI") is not met for REST. Both index-evolution fixtures list only Cli and Graphql, there is no REST conformance driver, and I found no REST test for unique_together evolution in ddb-server. The workflows.rs comment defers REST to "driver 00175", but the Transport Impact table marks REST as Guaranteed. | tests/e2e/conformance/workflows.rs:349 | 5 | BLAKE |
| R2 | [1/3] | 🟠 | The add-unique duplicate check is skipped whenever any column in the tuple is "genuinely new" (live_probe_column returns None). Inside apply, the typedef is then buffered and the git commit happens first. The unique index is only built afterwards, at commit-time rematerialize (handle_commit runs commit_batch, then rematerialize_type). If that rebuild fails, git already holds a typedef declaring the constraint and the index is stale. A reindex of the same data would hit the same failure. The code comment says this is "deliberately deferred" and yields a raw SQLite error rather than the friendly UNIQUE_VIOLATION. That contradicts the PRD risk text ("must fail clearly... atomic... self-healing"). I did not construct a failing case, so whether a new column can actually hold duplicates (for example via DEFAULT) is unverified. | ddb-core/src/sql_engine/ddl/unique_ops.rs:86 | 2 | BLAKE |
| R3 | [1/3] | 🟠 | `live_table_columns` converts query failures into an empty list and discards row errors with `flatten()`. The caller interprets missing columns as permission to skip duplicate validation, potentially committing an unchecked constraint. Return `Result<Vec<String>>`, collect row results, and propagate errors before updating the typedef. | ddb-core/src/sql_engine/ddl/unique_ops.rs:20 | 6 | BOB |
| R4 | [1/3] | 🟠 | A column absent from SQLite is not necessarily data-free: existing documents can already contain its frontmatter key. Adding that column plus uniqueness skips validation entirely, allowing duplicate values to reach rematerialization after Git commits. Validate effective document values before committing; cover two documents with the same previously undeclared field. | ddb-core/src/sql_engine/ddl/unique_ops.rs:105 | 6 | BOB |
| R5 | [1/3] | 🟠 | Rename resolution checks live names before replaying rename history, so reused names select the wrong data. With duplicate `a` values and distinct `b` values, `b→c`, then `a→b`, then `ADD UNIQUE(b)` checks original `b` instead of original `a`. Replay renames backwards chronologically before checking live-column existence, and add this regression. | ddb-core/src/sql_engine/ddl/unique_ops.rs:53 | 8 | BOB |
| R6 | [1/3] | 🟡 | `apply_unsupported_type_narrowing_combined_with_unique_rejects_atomically` never reaches the unique-constraint preflight. It rejects because the TEXT to INTEGER conversion is unsupported, and `handle_alter_column_type` has no Text to Integer arm. It would therefore pass against the pre-change code, so it does not fail first for the R5 concern. Its comment blames a "renderer defect" where the engine's parser requires `SET DATA TYPE`, but `normalize_alter_column_type` (helpers/types.rs:210) exists to rewrite exactly that. The real cause is unverified. The comment is unsubstantiated and should be corrected or removed. Prefer a scenario that actually combines a supported conversion with the constraint. | ddb-core/src/service/schema_apply.rs:1470 | 6 | ALICE |
| R7 | [1/3] | 🟡 | `apply_rename_with_unique_over_existing_duplicate_data_still_rejects` asserts only `Structured { code: SCHEMA_APPLY_PARTIAL }`. A "no such column" failure from the pre-change code produces the same code, so the test cannot tell the duplicate rejection from an unrelated failure. Assert that the message mentions the duplicate or unique violation, as the sibling duplicate tests do. | ddb-core/src/service/schema_apply.rs:1548 | 8 | ALICE |
| R8 | [1/3] | 🟡 | `live_table_columns` swallows errors with `unwrap_or_default()` and `rows.flatten()`. A failed PRAGMA returns an empty list, so every column looks "new" and the duplicate check is silently skipped. The commit can then proceed toward a post-commit rematerialize failure, the exact gap the pre-commit check exists to avoid. Return a `Result` and propagate it. | ddb-core/src/sql_engine/ddl/unique_ops.rs:19 | 6 | ALICE |
| R9 | [1/3] | 🟡 | `handle_add_unique_constraint` is now about 77 lines (lines 67-143), over the 50-line limit, and this diff added roughly 40 of them. Extract the duplicate probe (probe columns, quoting, the COUNT query and the Option match) into a helper such as `count_duplicate_groups(&self, table, columns) -> Result<i64>`. | ddb-core/src/sql_engine/ddl/unique_ops.rs:67 | 6 | ALICE |
| R10 | [1/3] | 🟡 | A duplicate-data failure on add surfaces as SCHEMA_APPLY_PARTIAL, with the UNIQUE_VIOLATION text only folded into the message. GraphQL redacts it to "internal error". The PRD asks for a structured error that reuses the unique-violation path. The code is not preserved at the apply_schema boundary, and the context lists only applied_ops and failed_op. Docs acknowledge this behaviour, but it still falls short of the stated mitigation. | ddb-core/src/service/schema_apply.rs:117 | 3 | BLAKE |
| R11 | [1/3] | 🟡 | `SchemaPlan` is documented as sorting destructive ops after additive ones, but the partition only moves DropColumn. DropUniqueConstraint (which is_destructive) stays in emit order. It works today only because diff_unique_together runs after the drop-columns pass. A rename combined with a constraint depends on RenameColumn not updating unique_together, so the Drop and Add pair are not no-ops. Fragile, but tested by the rename-keep-unique test. | ddb-core/src/schema_diff/differ.rs:34 | 1 | BLAKE |
| R12 | [1/3] | 🟡 | The new error regressions can pass without exercising duplicate validation: the rename test accepts any `SCHEMA_APPLY_PARTIAL`, and the type-change test explicitly accepts an earlier parser failure. Assert the duplicate-specific cause for rename rejection, and use a supported transformation to exercise preflight safety; retain HEAD and data-preservation assertions. | ddb-core/src/service/schema_apply.rs:1208 | general | BOB |
| R13 | [1/3] | 🟡 | Task 7 still leaves empty-plan and warning outcomes unpinned. Reapply checks only `applied=false`; GraphQL defaults missing fields to false/empty and still discards warnings. Validate response fields and assert empty ops and no unsupported-change warnings on reapply, using equivalent CLI assertions. | tests/e2e/conformance/driver_graphql.rs:369 | 7 | BOB |
| R14 | [1/3] | 🟡 | The new enforcement fixture duplicates both schema YAML constants from the migration fixture. Move those constants to module scope and share them so the two workflows cannot drift. | tests/e2e/conformance/workflows.rs:364 | 7 | BOB |
| R15 | [1/3] | ⚪ | The new FFI test covers only create, add and enforce. The PRD says FFI coverage should include add, drop and reapply outcomes, and there is no drop or reapply leg through `DoogatDriver::apply_schema`. | ddb-core/src/ffi/tests.rs:790 | 7 | ALICE |
| R16 | [1/3] | ⚪ | The comment on the all-or-nothing composite skip claims an existing duplicate is "caught later by the real UNIQUE INDEX at commit". That is wrong: a brand-new column is NULL for all existing rows, and SQLite UNIQUE permits NULLs, so no violation can occur. The skip is correct but the rationale is misleading. The comments also carry "(task 6)" and "(task 8)" build-process markers that rot in source. | ddb-core/src/sql_engine/ddl/unique_ops.rs:86 | 8 | ALICE |
| R17 | [1/3] | ⚪ | DropUniqueConstraint requires `allow_destructive`. This is documented, but the PRD's error-case text ("drop a unique_together whose removal would orphan nothing → index dropped") does not mention the gate. | ddb-core/src/schema_diff/plan.rs:174 | 1 | BLAKE |
| R18 | [1/3] | ⚪ | ddb.udl still defines ColumnDefRecord without `zone` (and without other drift). It is documentation-only per ffi.md:326, but it is now inconsistent with the proc-macro record. | ddb-core/src/ddb.udl:62 | 4 | BLAKE |
| R19 | [1/3] | ⚪ | I could not verify "bindings regenerate clean". No generated Swift or Kotlin artifact or smoke check for the new zone field is in the tree. Bindings come from the uniffi proc-macro at build time. | N/A | 4 | BLAKE |

### Auto-dismissed (ledger)

- [BLAKE] 🟡 Implementation scope exceeds the PRD. It introduces a new user-facing SQL grammar, `ALTER TABLE x ADD UNIQUE (...)` and `DROP UNIQUE (...)`, via new regexes handled in the generic SQL engine. This is reachable from `ddb query`, FFI `execute_sql`, GraphQL `executeSql` and PgWire. The PRD calls the DDL module's export "internal constraint operation execution", and its PgWire row says there is no SQL surface for this and not to route PlanOps into raw DDL. The grammar is documented only as a rendered-SQL column in schema-apply.md; docs/src/technical/sql-engine.md does not mention it. | File: ddb-core/src/sql_engine/mod.rs:247 — Protocol D (High + public API) verdict escalate: the PRD requires the constraint ops but specifies no SQL signature, and the grammar is externally reachable rather than internal-only, so the framework forbids auto-proceeding. The grammar follows ddb's established internal-DDL pattern (SET ZONE, SET/DROP SEARCH KEY, DROP SINGLETON, DROP TITLE TEMPLATE all live in the same try_custom_ddl path and are equally reachable), which makes 'is this dialect public API, and should DROP UNIQUE honor allow_destructive' a maintainer decision about a pre-existing convention, not a defect introduced by this PRD. Deferred to batch end with both sub-questions named.
- [BLAKE] 🟡 The FFI zone test is a pure `ColumnDefRecord::from` unit test on hand-built ColumnDefs. No test goes through `driver.list_type_schemas()` on a real created type and compares to the GraphQL or YAML value. The existing list_type_schemas test (ffi/tests.rs:329) does not assert zone. The mapping is trivial and correct, but the PRD acceptance wording ("asserts the column zone matches GraphQL/YAML") is only indirectly covered. | File: ddb-core/src/ffi/tests/records_tests.rs:643 — single-voice

## Verification-check queue

Empty this cycle: Bob's VERIFY bucket is `(none)` and Eve did not run, so no `…-checks-2.json` was written.

## Alice

Incremental review of cycle 2 (changes since 64f7c42). Of the 24 prior findings, R1, R3, R4, R5, R6 and R7 are resolved in code, with one caveat on R5.

Checks Alice ran: a narrow `cargo test -p ddb-core --lib` filter over the six new rework tests passed (6 of 6); `cargo test -p ddb-e2e index_evolution` compiled but printed no results before she finished, so the conformance legs are locally unverified (Tier 2 / CI); she did not run the full `cargo test-ci`, as instructed.

Prior-finding verification:

- R1 (add column plus unique in one apply): resolved. `live_table_columns` and `live_probe_column` skip the duplicate SELECT for a genuinely new column, and `apply_adds_column_and_unique_together_over_it_in_one_apply` checks that a duplicate insert is rejected afterwards.
- R6 (rename before constraint): resolved. `column_renames` is recorded in the transaction buffer by rename.rs, cleared with `txn` on commit and rollback, and carried by `suspend_transaction` / `resume_transaction`. Chained renames resolve back to the live column, with a unit test. (Bob's R5 this cycle disputes the ordering of that resolution.)
- R3 and R4 (conformance): resolved on CLI and GraphQL — a drop step exists, the GraphQL driver returns `applied` and `ops`, the CLI driver returns stdout, and a duplicate-reject-then-drop-lifts probe fixture exists. REST stays deferred per the task (Blake's R1 this cycle). The FFI test covers only the add leg.
- R7 (actor test timeout): resolved by widening the timeout to 90s; `checks-1.json` records exit 0. A timeout widening is a mitigation rather than a root-cause fix, with a reasoned justification from the author.
- R5 (preflight versus rematerialize): only partly resolved — the new test does not exercise the preflight (R6 this cycle).

New findings: R6, R7, R8, R9 (🟡) and R15, R16 (⚪) in the table above.

R1: pass
R2: fail
R3: pass
R4: pass
R6: pass
R7: pass
R8: pass
R9: pass
R10: fail
R11: pass
R12: fail
R13: fail

## Blake

Blind lens, PRD-only. Findings: R1, R2 (🟠), R10, R11 (🟡), R17, R18, R19 (⚪) in the table above, plus the two ledger-dismissed re-raises listed under Auto-dismissed.

Checked and found compliant:

- The differ emits Add/DropUniqueConstraint with set-of-sets comparison; unit tests cover add, drop, no-op, reorder, duplicates and ordering against AddColumn and DropColumn.
- `unique_together` is no longer pushed to `plan.unsupported`.
- `render_sql`, `kind`, `describe` and `is_destructive` are implemented with quoted identifiers.
- Apply runs inside the single-transaction apply, with an idempotent re-apply and a pre-write duplicate check in the common case.
- Tests cover add, drop and re-add, idempotency, the duplicate-data error leaving the typedef untouched, and a single-commit assertion.
- FFI `ColumnDefRecord.zone: Option<String>` is populated in `From<&ColumnDef>` with GraphQL null semantics.
- CHANGELOG and docs (schema-apply.md, ffi.md) are updated.
- A CLI e2e covers the duplicate-data failure, and conformance fixtures cover add, re-apply, drop and duplicate enforcement on CLI and GraphQL.

Test gate: Blake ran no tests, per his run inputs.

B1: fail
B2: pass
B3: pass
B4: pass
B5: fail
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

Doubt lens (codex, resumed thread). FIX bucket: R3, R4, R5 (🟠) and R12, R13, R14 (🟡) in the table above. VERIFY: (none). KNOWN: (none).

R1: fail
R2: fail
R3: pass
R4: fail
R6: pass
R7: pass
R8: pass
R9: fail
R10: fail
R11: pass
R12: fail
R13: fail

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Carl

Carl did not run: `gemini-run` backend=copilot model=gemini-3.8-flash exited 1 with "You have exceeded your monthly quota", and the single permitted retry hit the same wall. Not latched for the batch (exit 1, not exit 4). No UI/frontend lens this cycle; this diff is Rust backend and test code only.

## Follow-up Tasks Created

None by this skill (PRD 00249): task creation belongs to the decision gate in `run-autopilot/references/phase-review.md`.

Verdict: 19 findings
Tests: 2148 passed, 0 failed, 0 skipped (suite run this cycle, cargo test-ci exit 0)
