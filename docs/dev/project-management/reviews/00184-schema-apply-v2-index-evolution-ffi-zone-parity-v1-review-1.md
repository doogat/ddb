---
prd: docs/dev/project-management/prds/wip/00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md
review: 1
date: 2026-10-20
head_sha: 64f7c42ac31ebbb6e7deef3671f4861292563088
codex_thread_id: 01a159ee-f6e7-7e82-ac4c-8f01fe033f65
reviewers: alice,blake,bob
agents:
  alice: available
  blake: available
  bob: available
  carl: unavailable
dispatch_rows:
  bob: 174ec5c6
  carl: bf4cc2d0
---

# Review: 00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1

Diff range: `aa902c2dd83d2bebc2e57a6b30610c0217c73735..64f7c42ac31ebbb6e7deef3671f4861292563088`

codex_rung_guard: not fired

pack: ok

## Review Summary

Reviewed: 5 completed tasks
PRDs checked: 00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1

### Agent Status

- Alice: ✅ Available (consensus lens, legacy engine)
- Blake: ✅ Available (blind lens, PRD-only)
- Bob: ✅ Available (doubt lens, codex; D1-D5 emitted)
- Carl: ⚠️ Unavailable: gemini-run backend=copilot model=gemini-3.8-flash exited 1, "You have exceeded your monthly quota". `[RETRY] carl attempt 1/1` re-dispatched and hit the same quota wall. Exit 1 is a runtime/quota failure, not exit 4, so Carl is NOT latched unavailable for the batch.
- Eve: ⏸️ Disabled: resolved doubt reviewer is `codex` and the codex doubt-roster guard did not fire (no task has a codex implementor).

### Method notes

- Consensus engine: `legacy` (single Alice subagent).
- Consolidation ran via `consolidate_findings.py` over three reviewers; no ledger existed (cycle 1), so no `--ledger` flags and no auto-dismissals.
- Mechanical test checks computed 0 findings: `compute_mech_facts.py` / `detect_tautological_tests.py` are python-only and skipped every Rust and Swift file in the diff ("Checked 0 test function(s) in 0 test file(s)"). The fail-first replay was likewise inapplicable. **The mechanical lens therefore contributed nothing this cycle** — Bob's R20 and Alice's R8 are model-side tautology findings, not computed ones.
- `Tests:` line composed by hand from the gate log, NOT copied from the staged `## Test gate` section and NOT re-derived by a second `autopilot verify` run. The staged line reads `Tests: counts unavailable (suite run this cycle, exit 101, no PASS/FAIL/SKIP/EXIT summary line)`: `verification.run_gate`'s parser does not understand cargo's `test result:` summaries, so re-running the ~30-minute suite through `autopilot verify` would return the same counts-free line. Counts below are read directly from `docs/dev/tmp/gate-00184-c1.log` (47 + 1774 + 320 passed, 1 failed).
- `last-verification.json` was not written: the work tree carries two modified autopilot telemetry files (`dispatch-metrics.jsonl` and its ledger mirror), and `run_gate` refuses to certify a dirty tree.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [1/3] | 🟠 | Adding a column and a unique_together over it in one apply fails mid-plan. AddColumn only buffers the typedef and defers the table rebuild (ddl/alter.rs:158, ddl/alter.rs:396). The new handler then checks the column against the buffered typedef, which passes, and runs the duplicate-check SELECT against the old materialized table. That SELECT hits "no such column" and surfaces as SCHEMA_APPLY_PARTIAL (ddl/unique_ops.rs:882). The differ deliberately orders AddColumn before AddUniqueConstraint (differ.rs:1206), so this is a supported-looking, common path. The fix is to skip the SQL duplicate check when the column is absent from the live table, or when the type is pending rematerialize. Add an apply_schema integration test for it. | ddb-core/src/sql_engine/ddl/unique_ops.rs:901 | 2 | ALICE |
| R2 | [1/3] | 🟠 | Spec violation: the apply path routes the new ops through new public raw-SQL grammar (`ALTER TABLE x ADD UNIQUE (...)` and `DROP UNIQUE (...)`). The PRD says PgWire has no SQL surface for this and that the constraint execution is internal. These statements are now intercepted by `try_custom_ddl`, so they are reachable from `ddb query`, GraphQL `executeSql` and PgWire. A raw `DROP UNIQUE` skips the `allow_destructive` gate that only `apply_schema` enforces. `docs/src/technical/sql-engine.md` and the CHANGELOG describe it only as apply output, not as user-callable SQL. | ddb-core/src/sql_engine/mod.rs:238 | 3 | BLAKE |
| R3 | [1/3] | 🟠 | Phase 2 acceptance is not met. The PRD asks for conformance asserting add/drop/reapply outcomes on CLI, GraphQL, REST and FFI. The `index_evolution_migration` fixture runs only create, add and reapply, and drop is never exercised. Its assertions are only "every step is Ok" and "no variant mismatch", so they never check `applied=false` on reapply, the op kinds, or duplicate rejection after the migration. There is no REST leg, and there is no `ddb-core/src/ffi` unit test of `apply_schema` with `unique_together` (the FFI tests contain no unique case). | tests/e2e/conformance/workflows.rs:294 | 5 | BLAKE |
| R4 | [1/3] | 🟠 | Conformance passes the original unsupported-change behavior: both drivers discard reports and warnings, and tests only require success. Preserve and assert applied/ops/warnings, add the missing drop step, and probe duplicate enforcement. | tests/e2e/conformance/cross_driver_schema_apply.rs:26 | 5 | BOB |
| R5 | [1/3] | 🟠 | The preflight checks existing SQLite values while rematerialization occurs after the Git commit; combined schema transformations may invalidate that check. Test one apply changing TEXT values `'01'`/`'1'` to INTEGER and adding uniqueness; require rejection before HEAD changes and preservation of both rows. | ddb-core/src/sql_engine/ddl/unique_ops.rs:45 | 2 | BOB |
| R6 | [1/3] | 🟠 | Unique-constraint operations may precede the rename they depend on: the documented planner postpones renames, while ADD validates destination columns immediately. Test renaming constrained column `a` to `b` and updating `unique_together` in one apply; require convergence, preserved data, and one commit. | ddb-core/src/schema_diff/differ.rs:106 | 1 | BOB |
| R7 | [1/3] | 🟠 | The mandatory fast gate failed: 2141 passed, one actor transaction test timed out. Reproduce with `cargo test -p ddb-server --lib actor::tests::shared_actor_preserves_schema_applys_owned_transaction -- --exact --nocapture`, resolve the timeout, then require a passing `cargo test-ci`; attribution to this diff remains unproven. | ddb-server/src/actor/tests/mod.rs:22 | general | BOB |
| R8 | [1/3] | 🟡 | The new conformance fixture cannot fail against pre-change code, so it is tautological. SchemaApply returns Null on both drivers with empty warnings (tests/e2e/conformance/driver_cli.rs:74, driver_graphql.rs:2330 in the diff). Before this change an unsupported unique_together returned success with an SCHEMA_UNSUPPORTED_CHANGE warning and applied=false, which also passes every step. The fixture has no drop step and nothing asserts that the migration was applied or that the re-apply was empty, although the task requires add/drop/reapply outcomes. Have SchemaApply surface applied and op kinds, or follow each apply with a probe that a duplicate INSERT is rejected. Add a drop step. | tests/e2e/conformance/workflows.rs:2407 | 5 | ALICE |
| R9 | [1/3] | 🟡 | Nothing tests unique_together combined with other schema changes through apply_schema. Missing cases: a constraint over a column added in the same plan (R1 is the live bug there), a constraint on a column dropped in the same plan, and rename_from. rename_from is unverified and probably broken. Renames run after additive ops, so AddUniqueConstraint on the new column name hits "column not found". The handlers also never rewrite a live constraint that still names the old column. Add integration tests, or document the limit. | ddb-core/src/service/schema_apply.rs:1056 | 3 | ALICE |
| R10 | [1/3] | 🟡 | differ.rs grows by 445 lines to roughly 1370, well over the 800-line limit. The tests are about 400 of those lines. Move the unique_together differ tests into a sibling test module (for example schema_diff/tests/unique.rs), as unique_constraint.rs already does for sql_engine. | ddb-core/src/schema_diff/differ.rs:967 | 1 | ALICE |
| R11 | [1/3] | 🟡 | The handler adds a `WHERE col IS NOT NULL` filter to the duplicate check, which the design contract does not have. It is correct, since SQLite UNIQUE permits multiple NULLs, but no test covers it. Add one test where two rows hold NULL and ADD UNIQUE must succeed, and one where two rows hold equal non-NULL values and it must fail. | ddb-core/src/sql_engine/ddl/unique_ops.rs:896 | 2 | ALICE |
| R12 | [1/3] | 🟡 | The only new e2e in `tests/e2e/schema_apply.rs` covers the duplicate-data failure. The PRD asks for a NEW e2e asserting a successful `unique_together` migration on an existing type. Only the conformance CLI driver reaches that outcome, and it asserts just `Ok`. | tests/e2e/schema_apply.rs:482 | 5 | BLAKE |
| R13 | [1/3] | 🟡 | The cycle's test gate shows exit 101 with no summary line, so the full-suite result is unknown and its cause is not established. A narrow `cargo test -p ddb-core --lib` filter (schema_apply, schema_diff, unique, ffi records) exited 0, and every unique-constraint, differ, schema_apply and FFI zone test passed. The gate failure therefore lies outside that subset. | N/A | general | BLAKE |
| R14 | [1/3] | 🟡 | The `zone` FFI test is a unit test of `ColumnDefRecord::from`. No test goes through `list_type_schemas` or compares against GraphQL/YAML values end to end. The mapping itself is correct and matches GraphQL: undeclared zone is `None`, and the tokens are frontmatter, body and reference. Regeneration of the Swift/Kotlin bindings could not be verified. | ddb-core/src/ffi/tests/records_tests.rs:643 | 4 | BLAKE |
| R15 | [1/3] | 🟡 | The new Swift initializer requires `zone`; optional type does not make its argument optional. Existing four-argument constructors break, contradicting the design's compatibility claim. Provide a generated default or document the source-breaking migration. | tests/swift/Sources/DoogatDB/ddb_core.swift:1055 | 4 | BOB |
| R16 | [1/3] | 🟡 | The duplicate query manually surrounds identifiers with quotes instead of escaping them, violating the explicit SQL identifier invariant. Reuse the existing identifier-quoting helper for columns and table names. | ddb-core/src/sql_engine/ddl/unique_ops.rs:38 | 2 | BOB |
| R17 | [1/3] | 🟡 | FFI coverage tests only `ColumnDefRecord::from`; it never calls `list_type_schemas` or exercises index evolution through FFI. Add public-driver tests for discovery zone values and apply/add/drop/reapply outcomes. | ddb-core/src/ffi/tests/records_tests.rs:643 | general | BOB |
| R18 | [1/3] | 🟡 | NULL exclusion was added without its regression test, as the assumptions ledger acknowledges. Test repeated tuples containing NULL succeed while repeated fully non-NULL tuples fail without committing. | ddb-core/src/sql_engine/ddl/unique_ops.rs:40 | 2 | BOB |
| R19 | [1/3] | 🟡 | Four tests rerun the same complete CLI/GraphQL workflow separately. Run it once in one test and retain the count, success, comparison, and report assertions together. | tests/e2e/conformance/cross_driver_schema_apply.rs:16 | 5 | BOB |
| R20 | [1/3] | 🟡 | `variants.len() == 8` asserts the size of its own eight-element literal, not enum completeness. Remove this self-oracle; test actual dispatch behavior instead. | tests/e2e/conformance/fixture.rs:204 | 5 | BOB |
| R21 | [1/3] | 🟡 | New tests extend already oversized files, and the happy-path apply test exceeds the rubric's function limit. Move the new suites into test submodules and extract focused assertion helpers without weakening coverage. | ddb-core/src/service/schema_apply.rs:1075 | general | BOB |
| R22 | [1/3] | ⚪ | The final assertion of the drop-then-readd test only checks `Structured { .. }`. A SCHEMA_APPLY_PARTIAL for some unrelated reason would also pass. Assert the "duplicate" message, as the sibling error-case test does. | ddb-core/src/service/schema_apply.rs:1176 | 3 | ALICE |
| R23 | [1/3] | ⚪ | The regenerated ddb_core.swift diff is 2663 lines. The file was last generated in March 2026, so the diff carries unrelated FFI drift from 00176 and 00177, not just the additive `zone` field the task expected. It is probably unavoidable, but the commit should say so. | tests/swift/Sources/DoogatDB/ddb_core.swift:1 | 4 | ALICE |
| R24 | [1/3] | ⚪ | The differ ordering relies on `diff_type` emitting `AddColumn` before `AddUniqueConstraint`, and `DropUniqueConstraint` before `DropColumn`. Both orderings are unit-tested, and an end-to-end CLI run (add column plus add unique, then reapply, then drop with `--allow-destructive`) worked. | ddb-core/src/schema_diff/differ.rs:93 | 1 | BLAKE |

## Verification-check queue

Written to `docs/dev/project-management/reviews/00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1-checks-1.json`, one entry: R7's reproduction command.

Not queued, with reason:

- R5 — not queued: the VERIFY text prescribes a new test to write, not an existing runnable command.
- R6 — not queued: same, a new test to author.

## Alice

Consensus lens, implementation-aware. Seven findings: one 🟠, four 🟡, two ⚪ (R1, R8-R11, R22-R23).

Reviewer notes, carried verbatim as context rather than findings:

- Test-gate failure: `actor::tests::shared_actor_preserves_schema_applys_owned_transaction` was re-run in isolation with `cargo test -p ddb-server --lib shared_actor_preserves_schema_applys_owned_transaction` and passed in 1.03s. The test applies a single-column type with no `unique_together` (ddb-server/src/actor/tests/mod.rs:12), producing one CreateType op, so it never reaches the new differ or handler code. Most likely a 30s timeout under machine load during the ~30-minute gate run. Alice's verdict: flake, not caused by this diff.
- Verification limits she declared: the add-column-plus-unique case was not executed; R1 is read from `ddl/alter.rs:60`, `ddl/alter.rs:396`, `ddl/unique_ops.rs:882`, `ddl/unique_ops.rs:901`. The rest of `cargo test-ci` was not re-run.

```text
R1: fail
R2: fail
R3: pass
R4: fail
R6: pass
R7: pass
R8: pass
R9: pass
R10: pass
R11: pass
R12: pass
R13: fail
```

## Blake

Blind lens, PRD-only: he never saw the diff, the design doc, or any review history, and located the code himself. Six findings: two 🟠, three 🟡, one ⚪ (R2-R3, R12-R14, R24).

Manual checks he ran against a temp repo:

- Adding a unique constraint on an existing type works; duplicates are then rejected.
- Reapply prints "no changes".
- Dropping the constraint works with `--allow-destructive`.
- Adding a constraint over duplicate rows fails with a structured error, and the unit test confirms nothing is written to the typedef.

```text
B1: pass
B2: pass
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
```

## Bob

Doubt lens (codex, static-only sandbox) carrying rubric D1-D5 and the de-slop lens. Eleven findings: four 🟠, seven 🟡 (R4-R7, R15-R21). FIX bucket held eight items, VERIFY three, KNOWN none.

```text
R1: fail
R2: fail
R3: pass
R4: fail
R6: pass
R7: pass
R8: fail
R9: fail
R10: pass
R11: pass
R12: fail
R13: fail
D1: pass
D2: pass
D3: pass
D4: pass
D5: pass
```

## Carl

Not run. `gemini-run.sh` selected backend=copilot model=gemini-3.8-flash and exited 1 with "You have exceeded your monthly quota". The one-retry budget was spent on an identical re-dispatch, which hit the same wall. No output file was published and none was consolidated. Exit 1, not exit 4, so Carl is not latched unavailable for batch 202607161128 — a later PRD in this batch will try him again.

## Follow-up Tasks Created

None by this skill (PRD 00249: task creation belongs to the decision gate).

Verdict: 24 findings
Tests: 2141 passed, 1 failed, 0 skipped (suite run this cycle, cargo test-ci exit 101)
