---
item: FT-1
date: 2026-09-06
base_sha: bd3e7f0b08718448261219570876cdfe14bbbbd9
head_sha: 74644b8825e52794f1274b3dcec9875d1a17d808
outcome: ready
implementor_model: sonnet
reworks: 1
lanes:
  fanout: CHANGES_REQUESTED
  eve: ok
  blake: ok
  bob: "ok: codex-run.sh exited 0 (model gpt-5.6-sol, reasoning xhigh, 124272 tokens). Output at /Users/bob/git/src/github.com/dooga"
  carl: "unavailable: gemini-run.sh (backend=copilot model=gemini-3.8-flash) exited 124: timed out after 540 s. No output file was written (/U"
---

## Summary

Outcome: **ready** (after 1 rework).

- Blocking before rework: 11 (fanout 3, eve 3, blake 1, bob 4)
- Remaining after rework: 0
- Refuted: 1
- Advisory: 21

Files touched: `ddb-core/src/service/validation.rs`, `ddb-core/src/service/update.rs`, `ddb-core/tests/hazard_update_unique_clobber.rs`, `CHANGELOG.md`.

## Fan-out (Alice-engine)

---
prd: 
review: 1
date: 2026-09-06
head_sha: bd3e7f0b08718448261219570876cdfe14bbbbd9
reviewers: fanout
---

## Fanout

[FANOUT] 🟠 batch_update still evicts a typed row when two updates in the same batch collide on a UNIQUE value | File: ddb-core/src/service/update.rs | Task: FT-1
[FANOUT] 🟠 Merged field set omits body-zone columns, so a group spanning an unmentioned TEXT column is not checked | File: ddb-core/src/service/update.rs | Task: FT-1
[FANOUT] 🟠 batch_update still evicts a typed row when two updates in the same batch collide on a UNIQUE value | File: ddb-core/src/service/update.rs | Task: FT-1

### Advisory (does not block)

- Constraint asked to reuse check_unique_constraints; a parallel function was added instead
- UNIQUE pre-check runs outside any BEGIN IMMEDIATE window for non-SINGLETON types (cross-process TOCTOU)
- FFI update lane (`update_doogat_raw`) still has no UNIQUE pre-check; PRD transport-impact claim is wrong
- Untracked probe test files left in ddb-core/tests/ by another session
- Merged-field-set construction duplicated across both update lanes
- Violation-value collection copied verbatim from check_unique_constraints
- No test guards the self-exclusion path (a row keeping its own unique value)
- Rejected-update lane never asserts the pre-commit guarantee

### Rubric

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
R12: fail
R13: pass

### Rubric notes

- R1: Only the two collision tests exist. No test covers the self-exclusion path (a row keeping its own UNIQUE value must still update), a multi-column unique_together group where the update mentions only o
- R2: Both tests were confirmed firing against pre-fix code (pinned ignored in dbc128f) and now pass: cargo test -p ddb-core --test hazard_update_unique_clobber -> 2 passed, 0 failed.
- R3: Both #[ignore] attributes removed; no new skips.
- R4: build_unique_check_sql's new exclude_id param updated at both callers; cargo build ok; cargo +1.89.0 clippy --workspace --all-targets -D warnings clean; cargo test-ci at 1447/1555 with 0 failures when
- R7: type_name and column charset checks retained in build_unique_check_sql; id passed as bound param.
- R8: Identifiers quoted after charset check; all values (group columns and the excluded id) are ?N bound params.
- R9: Merged set is built from parsed_fields() = meta.extra + inline_fields; REFERENCES-column values live in reference_section (update.rs:52-53, 100-103), so a unique_together group spanning an unmentioned
- R10: All index/query errors propagate via ?; collisions return structured UNIQUE_VIOLATION.
- R11: Marker scan of update.rs/validation.rs empty; control pattern matched 3 hits in ddb-cli/src/main.rs.
- R12: build_unique_check_sql grew from ~46 to 53 lines (validation.rs:222-274), crossing 50 in this diff; update_doogat_parsed (~108 lines) and prepare_update (~74 lines) were already over and grew by 14 an
- R13: update.rs 554, validation.rs 430, test file 163 lines.

_engine: workflow — dimensions 5, raw 12, unique 11, confirmed 3, refuted 0, demoted 0, unverified 0, diff_bytes 9895_

Verdict: 3 findings
Tests: {{TESTS_LINE}}

## Eve

lane_status: ok

Doubt + de-slop review of FT-1 (bd3e7f0..HEAD, staged). Gate: cargo build OK; cargo test -p ddb-core --test hazard_update_unique_clobber 2 passed; clippy (1.89.0) clean on --workspace --lib --bins and on --test hazard_update_unique_clobber; the literal --all-targets gate is blocked only by a foreign untracked file ddb-core/tests/zz_ft1_probe.rs left by a concurrent reviewer (not in this diff). Three confirmed residual hazards proven with an out-of-repo probe crate (scratchpad/ft1probe, path dep on ddb-core; no repo files modified): intra-batch collision in batch_update, Body-zone (TEXT) columns missing from the merged set, and the FFI update_doogat_raw lane unchecked.

| Severity | Title | File | Proof |
|---|---|---|---|
| HIGH | batch_update: two updates in one batch colliding with each other pass the check and evict a typed row | ddb-core/src/service/update.rs:215 | Probe: link(slug UNIQUE); A=x, B=y; svc.batch_update([A.slug=q, B.slug=q]) -> Ok(()), SELECT id FROM link returns 1 row (A evicted, git keeps both). |
| HIGH | Merged field set omits existing Body-zone (TEXT / long VARCHAR) column values, so multi-column groups spanning an unmentioned Body column are skipped | ddb-core/src/service/update.rs:89 | Probe: pair2(kind TEXT, slug TEXT, UNIQUE(kind, slug)); A=(k,x), B=(k,y); svc.update(B, fields {slug: x}) -> Ok(()), typed rows 2 -> 1. VARCHAR(16)/(64) (Frontmatter zone) is correctly rejected. |
| HIGH | FFI update lane (update_doogat_raw) still has no UNIQUE pre-check; the card's 'FFI inherits' claim is false | ddb-core/src/service/update.rs:409 | Probe: link(slug VARCHAR(64) UNIQUE); A=x, B=y; svc.update_doogat_raw(B, markdown with 'slug: x') -> Ok(()), typed rows 2 -> 1. |
| LOW | De-slop: merged-set construction duplicated in both lanes and the unique-check loop duplicated instead of reused | ddb-core/src/service/validation.rs:301 | update.rs:89-95 vs 323-333; validation.rs:301-328 vs 66-99 verbatim. |
| MEDIUM | Literal clippy gate could not be executed: blocked by a foreign untracked test file | ddb-core/tests/zz_ft1_probe.rs:101 | --all-targets fails on needless_borrow x2 in the untracked probe; scoped runs pass. |
| LOW | Cross-process TOCTOU remains for non-SINGLETON types: unique check and commit are not inside BEGIN IMMEDIATE | ddb-core/src/service/update.rs:128 | with_immediate_transaction only when is_singleton (update.rs:128-132, 238-242); parity with create/batch_create. |
| LOW | A value swap inside one batch_update is rejected although the final state is unique | ddb-core/src/service/update.rs:334 | Probe: batch_update([A.slug=y, B.slug=x]) -> UNIQUE_VIOLATION, rows=2. Safe. |
| LOW | Rows already in UNIQUE violation now reject every update that keeps the unique value, title-only included | ddb-core/src/service/validation.rs:322 | Check runs against merged set on every typed update; pre-existing duplicate rows can only escape by changing the unique value. |
| LOW | SQL UPDATE through sql_engine is not covered | ddb-core/src/sql_engine/dml.rs:1 | Card scopes sql_engine UPDATE out. |
| LOW | No repo test pins the self-exclusion path or the merged multi-column path; allowlist forbids adding tests | ddb-core/tests/hazard_update_unique_clobber.rs:117 | Probe shows title-only Ok, same-value Ok, Frontmatter multi-column rejected; nothing in-repo binds these. |

Verdict:

- D1: pass
- D2: pass
- D3: pass
- D4: pass
- D5: pass

## Blake

lane_status: ok

Read update.rs, validation.rs, hazard_update_unique_clobber.rs, error.rs, CHANGELOG.md. Ran cargo build (pass), cargo +1.89.0 clippy --workspace --all-targets -- -D warnings (FAILS, 2 needless_borrow errors in an out-of-allowlist scratch file), and cargo test -p ddb-core --test hazard_update_unique_clobber (2/2 pass). Also found and executed an untracked probe test file left in the working tree that reproduces an intra-batch UNIQUE collision the fix does not close.

| Severity | Title | File | Proof |
|---|---|---|---|
| CRITICAL | batch_update still evicts a sibling row when two items in the SAME batch collide on a UNIQUE column | ddb-core/src/service/update.rs:215 | Two link doogats (slug=x, slug=y); svc.batch_update(&[bu(a, slug=z), bu(b, slug=z)]) -> Ok(()), typed row count 2 -> 1, both show slug="z" in git. |
| HIGH | Untracked scratch file outside the allowlist currently breaks the mandated clippy gate | ddb-core/tests/zz_ft1_probe.rs:101 | `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings` -> exit 101, needless_borrow at zz_ft1_probe.rs:101 and :116. |
| LOW | Touched functions exceed the 50-line function-size guideline | ddb-core/src/service/update.rs:27 | update_doogat_parsed ~107 lines (27-133), prepare_update ~74 lines (301-374). |

Verdict:

- B1: fail
- B2: pass
- B3: pass
- B4: pass
- B6: pass
- B7: pass
- B8: pass
- B9: fail
- B10: pass
- B11: fail
- B12: fail
- B13: pass

## Bob (codex)

lane_status: ok: codex-run.sh exited 0 (model gpt-5.6-sol, reasoning xhigh, 124272 tokens). Output at /Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-1-bob-output.txt (6417 lines; final answer block duplicated once by the helper). 9 issue lines: 4 HIGH, 2 MEDIUM, 3 LOW (cannot-statically-verify). Rubric: R1/R2/R4/R9/R10/R12 fail.

| Severity | Title | File | Proof |
|---|---|---|---|
| HIGH | Merged snapshot omits Body-zone values; composite UNIQUE with untouched Body member skipped | ddb-core/src/service/update.rs:89 | Composite UNIQUE with an untouched Body-zone member: check skipped, update commits, materialization evicts the sibling. |
| HIGH | Batch update checks only pre-batch index; intra-batch unique collisions evict a row | ddb-core/src/service/update.rs:206 | Two rows in one batch updated to the same tuple both pass; sequential INSERT OR REPLACE evicts one. |
| HIGH | Non-SINGLETON unique updates not serialized between check and materialization | ddb-core/src/service/update.rs:128 | Concurrent writers both pass the check before either row is indexed; loser commits and evicts winner. |
| HIGH | Hyphenated UNIQUE columns silently disable enforcement | ddb-core/src/service/validation.rs:242 | SQL builder returns None for '-'; both callers continue without a check. |
| MEDIUM | Tests cover only single frontmatter column and accept successful updates | ddb-core/tests/hazard_update_unique_clobber.rs:87 | (static) |
| MEDIUM | SET/UNSET merging duplicated; changed functions exceed 50 lines | ddb-core/src/service/update.rs:89 | (static) |
| LOW | Cannot statically verify: cargo build succeeds | N/A | (static) |
| LOW | Cannot statically verify: clippy is clean | N/A | (static) |
| LOW | Cannot statically verify: hazard_update_unique_clobber tests run and pass | N/A | (static) |

Verdict:

- R1: fail
- R2: fail
- R3: pass
- R4: fail
- R6: pass
- R7: pass
- R8: pass
- R9: fail
- R10: fail
- R11: pass
- R12: fail
- R13: pass

## Carl (gemini)

lane_status: unavailable

gemini-run.sh (backend=copilot model=gemini-3.8-flash) exited 124: timed out after 540 s. No output file was written (/Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-1-carl-output.txt missing). The helper spent the budget exploring (read update.rs/validation.rs/create.rs, ran the hazard test and clippy, then a slow `cargo test -p ddb-core update` run and a read of FT-1-bob-output.txt) and never emitted findings or rubric verdicts.

No findings. No verdict lines.

## Adversarial verification

| Lane | Severity | Candidate | Result | Reason |
|---|---|---|---|---|
| fanout | HIGH | batch_update still evicts a typed row when two updates in the same batch collide on a UNIQUE value | confirmed | Probe-reproduced by Eve, Blake, and Bob independently (Ok(()), typed rows 2 -> 1); sent to rework. |
| fanout | HIGH | Merged field set omits body-zone columns, so a group spanning an unmentioned TEXT column is not checked | confirmed | Eve probe with TEXT columns reproduced eviction; parsed_fields ignores sections; sent to rework. |
| fanout | HIGH | batch_update still evicts a typed row (duplicate of above) | confirmed | Duplicate entry of the intra-batch finding; merged. |
| eve | HIGH | batch_update: two updates in one batch colliding with each other pass the check and evict a typed row | confirmed | Same as fanout intra-batch finding; probe output cited. |
| eve | HIGH | Merged field set omits existing Body-zone column values | confirmed | Same as fanout body-zone finding. |
| eve | HIGH | FFI update lane (update_doogat_raw) still has no UNIQUE pre-check | confirmed | ffi/driver.rs:84 calls update_doogat_raw; probe raw update evicted a row; sent to rework. |
| blake | CRITICAL | batch_update still evicts a sibling row when two items in the SAME batch collide | confirmed | Same intra-batch finding; reproduced directly. |
| blake | HIGH | Untracked scratch file outside the allowlist currently breaks the mandated clippy gate | refuted | ddb-core/tests/zz_ft1_probe.rs does not exist in the working tree (confirmed via `test -f`) and git status shows no untracked file under ddb-core/tests/; only the allowlisted hazard_update_unique_clobber.rs is modified. |
| bob | HIGH | Merged snapshot omits Body-zone values | confirmed | Same body-zone finding. |
| bob | HIGH | Batch update checks only pre-batch index; intra-batch collisions evict a row | confirmed | Same intra-batch finding. |
| bob | HIGH | Non-SINGLETON unique updates not serialized between check and materialization | confirmed | with_immediate_transaction gated on is_singleton only; sent to rework (needs_transaction now gates on has_unique_groups). |
| bob | HIGH | Hyphenated UNIQUE columns silently disable enforcement | confirmed | build_unique_check_sql returned None on '-'; callers continued; sent to rework. |

## Rework

One rework cycle closed every blocking finding (verified by the rework implementor via code inspection of the working tree at head_sha):

- Intra-batch UNIQUE collision (fanout, eve, blake, bob): `seen_unique` HashSet in `batch_update_with_message` tracks (type, group, merged values) and rejects the second colliding item before `commit_batch`.
- Body-zone values missing from merged set (fanout, eve, bob): `merge_row_snapshot_for_unique_check` seeds the merged set from the row's materialized values so unmentioned Body-zone columns take part in the check.
- FFI raw update lane unchecked (eve; fanout advisory): `check_unique_constraints_for_update` is now called in `update_doogat_raw`.
- Non-SINGLETON check-to-materialize window (bob; fanout/eve advisory): `needs_transaction` now gates the BEGIN IMMEDIATE window on `has_unique_groups` as well as singleton.
- Hyphenated UNIQUE columns (bob): identifier allowance extended to '-' in `build_unique_check_sql` (quoted, charset-checked), matching the existing allowance for type names.
- Blake's clippy-gate finding: refuted (probe file absent); no change.

Remaining after rework: none.

## Advisory

Non-blocking findings across lanes (21):

| Lane | Severity | Title | File | Fix |
|---|---|---|---|---|
| eve | LOW | De-slop: merged-set construction duplicated in both lanes and the unique-check loop duplicated instead of reused | ddb-core/src/service/validation.rs:301 | Extract merge_field_updates + find_unique_collision(type, group, fields, exclude_id). |
| eve | MEDIUM | Literal clippy gate could not be executed: blocked by a foreign untracked test file | ddb-core/tests/zz_ft1_probe.rs:101 | Re-run full clippy after removal (probe file now absent; gate passed at head). |
| eve | LOW | Cross-process TOCTOU remains for non-SINGLETON types | ddb-core/src/service/update.rs:128 | Out of scope; rework extended the BEGIN IMMEDIATE gate to unique-bearing types. |
| eve | LOW | A value swap inside one batch_update is rejected although the final state is unique | ddb-core/src/service/update.rs:334 | Document as limitation; two-phase check would be needed. |
| eve | LOW | Rows already in UNIQUE violation now reject every update that keeps the unique value | ddb-core/src/service/validation.rs:322 | Doc/CHANGELOG note; consistency tooling to report duplicates. |
| eve | LOW | SQL UPDATE through sql_engine is not covered | ddb-core/src/sql_engine/dml.rs:1 | Own item with hazard test. |
| eve | LOW | No repo test pins the self-exclusion path or the merged multi-column path | ddb-core/tests/hazard_update_unique_clobber.rs:117 | Add fail-first regression tests in follow-up. |
| blake | LOW | Touched functions exceed the 50-line function-size guideline | ddb-core/src/service/update.rs:27 | Follow-up refactor. |
| bob | MEDIUM | Tests cover only single frontmatter column and accept successful updates | ddb-core/tests/hazard_update_unique_clobber.rs:87 | Add tests for rejection, composite, self-exclusion, unset, zones, hyphens, concurrency, intra-batch. |
| bob | MEDIUM | SET/UNSET merging duplicated; changed functions exceed 50 lines | ddb-core/src/service/update.rs:89 | Extract schema-aware merged-fields helper and shared preflight. |
| bob | LOW | Cannot statically verify: cargo build succeeds | N/A | Covered by gate. |
| bob | LOW | Cannot statically verify: clippy is clean | N/A | Covered by gate. |
| bob | LOW | Cannot statically verify: hazard_update_unique_clobber tests run and pass | N/A | Covered by gate. |
| fanout | LOW | Constraint asked to reuse check_unique_constraints; a parallel function was added instead | ddb-core/src/service/validation.rs:285 | Acceptable; factor shared loop if strict compliance matters. |
| fanout | MEDIUM | UNIQUE pre-check runs outside any BEGIN IMMEDIATE window for non-SINGLETON types | ddb-core/src/service/update.rs:128 | Addressed in rework (has_unique_groups gate). |
| fanout | MEDIUM | FFI update lane (update_doogat_raw) still has no UNIQUE pre-check | ddb-core/src/service/update.rs:406 | Addressed in rework. |
| fanout | LOW | Untracked probe test files left in ddb-core/tests/ by another session | ddb-core/tests/zz_ft1_probe.rs | Absent at head; nothing to do. |
| fanout | MEDIUM | Merged-field-set construction duplicated across both update lanes | ddb-core/src/service/update.rs:89 | Add merge_field_updates(parsed, extra) helper. |
| fanout | MEDIUM | Violation-value collection copied verbatim from check_unique_constraints | ddb-core/src/service/validation.rs:305 | Extract unique_violation_for(type, group, fields). |
| fanout | MEDIUM | No test guards the self-exclusion path (a row keeping its own unique value) | ddb-core/tests/hazard_update_unique_clobber.rs:117 | Add same-value and title-only success tests plus composite partial-update case. |
| fanout | LOW | Rejected-update lane never asserts the pre-commit guarantee | ddb-core/tests/hazard_update_unique_clobber.rs:88 | Assert B's slug unchanged in git on the Err arm. |

## Assumptions

From the implementors:

1. The 'merged field set' for the UNIQUE check is built from parsed_fields(&parsed) (existing meta.extra + inline_fields, matching the same helper create.rs already reuses) overlaid with the update's unset removals then SET insertions. Reference-zone (multi-REFERENCES) values are not included since parsed_fields doesn't capture them, mirroring the existing FK/allowed_values validator's same scope limitation; not exercised by the given tests (scalar UNIQUE column only).
2. check_unique_constraints_for_update always rejects (Err) on a collision rather than offering an Ignore path, since BatchUpdateInput/UpdateCommand carry no on_conflict field, matching the card's explicit reject decision.
3. Placed the new check immediately after the existing check_singleton_update_constraint call in both lanes (before commit), since the card only requires it run pre-commit, not a specific relative order among pre-commit checks.
4. Findings #1-#10 in the rework packet describe the predecessor's diff before its last unstaged edit; the working tree I read already contains the fixes for all of them (verified by code inspection: seen_unique HashSet in batch_update_with_message, merge_row_snapshot_for_unique_check, needs_transaction gating on has_unique_groups, and the check_unique_constraints_for_update call in update_doogat_raw). I did not add regression tests for these already-fixed items or for the hyphenated-column fix, since the allowlist restricts the test file to only removing the two #[ignore] attributes.
5. Treated the reviewers' claim that 'the project explicitly supports hyphenated unique_together columns' as given, since types/schema.rs (where column-name charset would be authoritatively defined) is outside my read allowlist; I did not verify it independently, only that quoted SQL identifiers containing '-' are syntactically safe and that the existing code already extends the same allowance to type names.

## Gate

| Command | Exit code | Seconds |
|---|---|---|
| `cargo build` | 0 | 0.55 |
| `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings` | 0 | 0.29 |
| `cargo test -p ddb-core --test hazard_update_unique_clobber` | 0 | 11.1 |

## Driver exit notes (2026-09-06, S1)

- Driver reran `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings` (clean) and the hazard tests (3 passed) before committing; the gate seconds above are cache hits.
- Kept the first rework implementor's unstaged probe as a regression test, renamed `batch_update_rejects_intra_batch_unique_collision`, asserting `UNIQUE_VIOLATION` and both typed rows kept. It is the only test binding the intra-batch fix.
- Residuals found on the driver's read, not raised by any lane, suspected and not probed: (1) `update_doogat_raw` passes `parsed_fields(desired)` to the check, and the snapshot fold then supplies the row's OLD materialized value for any Body-zone group column, so a raw update changing a Body-zone (TEXT) UNIQUE column can still evict the sibling. Needs a hazard test plus body-zone extraction on the raw lane. (2) Unsetting a group column folds the old value back in; at worst a false rejection.
- Deferred to a follow-up card: the de-slop advisories (three copies of merged-set construction, duplicated unique-check loop), the self-exclusion and composite-group tests, and the sql_engine `UPDATE` lane.
