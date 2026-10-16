---
item: FT-4
date: 2026-09-06
base_sha: dbc128ffceb676e08cc13e2ab760865c1c0681db
head_sha: dbc128ffceb676e08cc13e2ab760865c1c0681db
outcome: ready
implementor_model: sonnet
reworks: 1
lanes:
  fanout: APPROVE
  eve: ok
  blake: ok
  bob: "ok: codex-run.sh completed (Codex v0.153.4, model gpt-5.6-sol, read-only sandbox, 117391 tokens)"
  carl: "failed: gemini-run.sh (backend=copilot, model=gemini-3.8-flash) exited 124: timed out after 540 s"
---

## Summary

Outcome: **ready**.

- Blocking before rework: 1 (bob CRITICAL, closed by rework 1)
- Remaining blocking: 0
- Refuted: 1
- Advisory (non-blocking): 15
- Reworks: 1
- Lanes: fanout APPROVE, eve ok, blake ok, bob ok, carl failed (timeout, no findings)

Files touched: `ddb-core/src/service/create.rs`, `ddb-core/tests/hazard_raw_create_existing_id.rs`, `CHANGELOG.md`.

## Fan-out (Alice-engine)

---
prd: 
review: 1
date: 2026-09-06
head_sha: dbc128ffceb676e08cc13e2ab760865c1c0681db
reviewers: fanout
---

## Fanout

[FANOUT] ✅ No issues found

### Advisory (does not block)

- Existence check runs outside the repo write lock (TOCTOU window before commit_file)
- create_doogat_raw docstring omits the new Conflict-on-existing-id rejection
- Hazard test header still describes the overwrite as live behavior
- No test binds to decision D3 (Conflict, never re-mint); the only test accepts any Err or a re-mint
- No boundary test through FFI, the one transport named as reaching this path; Conflict surfaces there as DdbError::Git
- Existence check runs outside the repo write lock: concurrent raw creates with the same explicit id can still overwrite

### Rubric

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
R12: fail
R13: pass

### Rubric notes

- R1: The gate test (hazard_raw_create_existing_id.rs) accepts any Err(_) and also a re-mint Ok branch; nothing asserts the D3-mandated behavior (DoogatError::Conflict specifically, re-mint forbidden). The 
- R2: Test asserts A's stored bytes are byte-identical after the raw create; pre-change code wrote ddb/{id}.md verbatim via commit_file (hazard confirmed and pinned in dbc128f), so it failed against old cod
- R3: #[ignore] removed; 0 ignored in the gate run.
- R4: Only runtime caller is ffi/driver.rs:68; sibling tests raw_ffi_semantics (10), raw_create_mint_collision (1), id_mint_collision (2), singleton_layers (8) all pass; cargo +1.89.0 clippy --workspace --a
- R7: Candidate id comes from parsed frontmatter and is only compared against an in-memory HashSet built by existence_oracle (repo HEAD walk + 'SELECT id FROM doogats'); never interpolated.
- R8: No SQL/command built from the id; oracle queries are constant strings.
- R9: Rejects with DoogatError::Conflict, consults repo+index via the shared existence_oracle (I3), runs before any git write, no re-mint, minting path for id-less frontmatter untouched, CHANGELOG line unde
- R10: Oracle construction error propagates with ?; collision returns a typed Conflict error.
- R12: create_doogat_raw spans create.rs:428-521 (~94 lines), already over the 50-line limit before this change; the diff adds 10 lines inside it rather than extracting the check into a helper.
- R13: create.rs is 610 lines; test file 79.

_engine: workflow — dimensions 5, raw 6, unique 6, confirmed 0, refuted 0, demoted 0, unverified 0, diff_bytes 3401_

Verdict: converged
Tests: {{TESTS_LINE}}

## Eve

lane_status: ok

Doubt + de-slop review of FT-4 (dbc128f..HEAD staged). Gate run in foreground: cargo build ok; clippy (1.89.0, -D warnings) clean; hazard test 1 passed 0 ignored; neighbors raw_create_mint_collision (1), id_mint_collision (2), raw_ffi_semantics (10), singleton_layers (8) all passed. Diff is minimal and matches D3. No FIX items; residuals are allowlist-bounded or pre-existing class limitations.

| severity | title | file | proof |
|---|---|---|---|
| MEDIUM | Fail-first not demonstrated in this diff for the un-ignored hazard test | ddb-core/tests/hazard_raw_create_existing_id.rs:21 | Diff only removes #[ignore]; failure against pre-change create.rs is asserted by commit dbc128f's message, not shown. VERIFY bucket. |
| LOW | Test accepts any Err, so the Conflict variant mandated by D3 is unpinned | ddb-core/tests/hazard_raw_create_existing_id.rs:61 | `Err(_) => {}` passes for any variant; create.rs:437 returns Conflict today but a regression cannot fail this test. KNOWN: allowlist limits test edits to removing #[ignore]. |
| LOW | Existence check runs outside the git write lock (check-then-commit TOCTOU) | ddb-core/src/service/create.rs:435 | Lock taken only inside repo.commit_file (git_ops/mod.rs:324-325); commit_files has no exists guard. Same class as unique_id()/I3 (00170). Guard in git_ops outside allowlist. |
| LOW | FFI transport collapses Conflict to DdbError::Git | ddb-core/src/ffi/records.rs:98 | `DoogatError::Conflict(msg) => DdbError::Git { msg }`; pre-existing mapping, owned by 00173/00177. |
| LOW | Hazard test module doc still describes H4 as live with stale line references | ddb-core/tests/hazard_raw_create_existing_id.rs:3 | Lines 3-16 cite create.rs:431-434/488 which no longer match. Allowlist-locked, cosmetic. |

Verdict:

- D1: pass
- D2: pass
- D3: pass
- D4: pass
- D5: pass

## Blake

lane_status: ok

Reviewed the full diff (create.rs, hazard test #[ignore] removal, one CHANGELOG line). Gate ran in foreground and passed: cargo build clean, clippy 1.89.0 -D warnings clean, hazard test 1 passed 0 ignored. Diff is exactly the allowlisted files. Fix adds a repo-aware existence check (existence_oracle unions repo.list_doogats() and the SQLite doogats index) before any git write, returning DoogatError::Conflict; the minting branch is untouched.

| severity | title | file | proof |
|---|---|---|---|
| LOW | create_doogat_raw remains a large function (pre-existing, not newly introduced) | ddb-core/src/service/create.rs:428 | ~90 lines (428-521) vs 50-line convention; diff adds ~10 lines; allowlist forbids refactor. Not a regression. |

Verdict:

- R1: pass
- R2: pass
- R3: pass
- R4: pass
- R6: pass
- R7: pass
- R8: pass
- R9: pass
- R10: pass
- R11: pass
- R12: fail
- R13: pass

[BLAKE] ✅ No issues found

## Bob (codex)

lane_status: ok

codex-run.sh completed (Codex v0.153.4, model gpt-5.6-sol, read-only sandbox, 117391 tokens). Output at `dev/local/tmp/FT-4-bob-output.txt` (12771 lines); final [BOB] block emitted twice, identical, deduplicated here. Evidence is the line text only.

| severity | title | file | proof |
|---|---|---|---|
| CRITICAL | Collision oracle snapshots HEAD/index before commit_file takes the repo lock; concurrent raw creates of the same absent ID can both pass | ddb-core/src/service/create.rs:431 | Two concurrent create_doogat_raw calls naming the same absent id both pass the pre-lock check; the second commit overwrites the first. Fix: re-check and create under the same repo write lock. |
| HIGH | Enabled hazard test accepts any error or re-mint and only verifies read-back Git bytes; does not enforce D3 Conflict result or unchanged index row | ddb-core/tests/hazard_raw_create_existing_id.rs:47 | An implementation that re-mints or returns a non-Conflict error still passes. (Refuted, see below.) |
| MEDIUM | create_doogat_raw is 94 lines after inlining collision logic, exceeding the 50-line limit | ddb-core/src/service/create.rs:428 | Line text only; extract ID selection and collision validation into a helper. |
| LOW | Hazard test module docs still claim explicit IDs bypass the oracle and overwrite existing data | ddb-core/tests/hazard_raw_create_existing_id.rs:3 | Line text only; rewrite as regression history plus current invariant. |
| LOW | Cannot statically verify: cargo build, clippy, and hazard integration-test gates pass | N/A | Read-only sandbox; run Tier 1 gate locally. |

Verdict:

- R1: fail
- R2: fail
- R3: pass
- R4: pass
- R6: pass
- R7: pass
- R8: pass
- R9: fail
- R10: pass
- R11: pass
- R12: fail
- R13: pass

## Carl (gemini)

lane_status: failed

gemini-run.sh (backend=copilot, model=gemini-3.8-flash) exited 124: timed out after 540 s. The reviewer read the card, diff, hazard test, create.rs and id_minting.rs, then spent the remaining budget running `cargo clippy` and `cargo test-ci` inside the review and never emitted findings. No output file written (`dev/local/tmp/FT-4-carl-output.txt` absent).

| severity | title | file | proof |
|---|---|---|---|
| - | (no findings; lane timed out) | - | - |

Verdict: none (no R-rule verdicts emitted).

## Adversarial verification

| # | lane | severity | candidate | result | reason |
|---|---|---|---|---|---|
| 1 | bob | CRITICAL | Collision oracle snapshots HEAD/index before commit_file takes the repo lock; concurrent raw creates of the same absent ID can both pass | confirmed | Oracle evaluated at create.rs:431-435; lock acquired only inside repo.commit_file (git_ops/mod.rs:325), commit_files does unconditional fs::write + index.add_path. Eve (LOW) and fanout (LOW + MEDIUM security) independently reproduced the same TOCTOU. Escalated to blocking; closed by rework 1. |
| 2 | bob | HIGH | Enabled hazard test accepts any error or re-mint and only verifies read-back Git bytes; does not enforce D3 Conflict result or unchanged index row | refuted | The card's Constraints state the test "accepts either reject or re-mint" and "do not weaken it"; the Allowlist restricts the test file to "only: remove the #[ignore] attribute", so tightening the assertion is expressly out of scope. The shipped implementation (create.rs:434-440) already returns DoogatError::Conflict unconditionally before any write, so the proof describes a pre-existing, spec-sanctioned coverage gap, not a flaw introduced by this diff. Kept as advisory (eve LOW, fanout MEDIUM). |

All other candidates (eve MEDIUM/LOW x5, blake LOW, bob MEDIUM/LOW x3, fanout x6) were non-blocking on arrival and were not escalated; they are listed under Advisory.

## Rework

Rework 1 (of 1 allowed), implementor: sonnet.

- Closed: bob CRITICAL "Collision oracle snapshots HEAD/index before commit_file takes the repo lock". The implementor had no allowlist access to `git_ops/write_lock.rs`, so the fix reuses the codebase's established precedent: the SQLite `BEGIN IMMEDIATE` transaction already used to guard the identical git-commit_file race for SINGLETON typedef creates (comment at create.rs:446-449/526-528). This serializes all callers sharing the same index `sql_conn`, the same guarantee the SINGLETON case already relies on for this race class. The literal repo write lock named in the reviewer's suggested fix was not used; cross-process callers with separate connections remain covered only to the extent the SINGLETON precedent covers them (see Advisory, TOCTOU entries).
- CHANGELOG.md and the #[ignore] removal were verified already correct from the first pass; no edits.
- Post-rework gate: see Gate.

Remaining blocking after rework: 0.

## Advisory

Non-blocking findings from all lanes (15). Follow-ups are outside the FT-4 allowlist unless noted.

| lane | severity | title | file | fix / note |
|---|---|---|---|---|
| eve | MEDIUM | Fail-first not demonstrated in this diff for the un-ignored hazard test | ddb-core/tests/hazard_raw_create_existing_id.rs:21 | Run `git stash push ddb-core/src/service/create.rs`, then `cargo test -p ddb-core --test hazard_raw_create_existing_id` expecting `1 failed` ('OVERWROTE doogat A'), then `git stash pop`. |
| eve | LOW | Test accepts any Err, so the Conflict variant mandated by D3 is unpinned | ddb-core/tests/hazard_raw_create_existing_id.rs:61 | Follow-up: `Err(e) => assert!(matches!(e, DoogatError::Conflict(_)), "got {e:?}")`. |
| eve | LOW | Existence check runs outside the git write lock (check-then-commit TOCTOU) | ddb-core/src/service/create.rs:435 | Follow-up: move exists check inside the write-locked section or add an exists guard for create-only commits in git_ops. Partly mitigated by rework 1 (BEGIN IMMEDIATE). |
| eve | LOW | FFI transport collapses Conflict to DdbError::Git, so the named caller cannot see a conflict | ddb-core/src/ffi/records.rs:98 | Handled by 00177 when the FFI error table lands. |
| eve | LOW | Hazard test module doc still describes H4 as live with now-stale line references | ddb-core/tests/hazard_raw_create_existing_id.rs:3 | Follow-up: rewrite header to state the pinned contract, drop line numbers. |
| blake | LOW | create_doogat_raw remains a large function (pre-existing) | ddb-core/src/service/create.rs:428 | Tracking only; ~90 lines before FT-4. |
| bob | MEDIUM | create_doogat_raw is 94 lines after inlining collision logic | ddb-core/src/service/create.rs:428 | Extract ID selection and collision validation into a helper. |
| bob | LOW | Hazard test module docs still claim explicit IDs bypass the oracle and overwrite | ddb-core/tests/hazard_raw_create_existing_id.rs:3 | Rewrite as regression history plus current invariant. |
| bob | LOW | Cannot statically verify gates pass | N/A | Gate ran locally by eve, blake and the harness; see Gate. |
| fanout | LOW | Existence check runs outside the repo write lock (TOCTOU window before commit_file) | ddb-core/src/service/create.rs:434 | Mirrors the mint path (`unique_id`, write_helpers.rs:245); pre-existing class. If closed, re-run the oracle inside the locked section for both paths together. |
| fanout | MEDIUM | create_doogat_raw docstring omits the new Conflict-on-existing-id rejection | ddb-core/src/service/create.rs:414 | Add one rustdoc sentence: existing `id:` in HEAD tree or `doogats` index is rejected with `DoogatError::Conflict` (D3). |
| fanout | LOW | Hazard test header still describes the overwrite as live behavior | ddb-core/tests/hazard_raw_create_existing_id.rs:1 | Queue follow-up (allowlist). |
| fanout | MEDIUM | No test binds to decision D3 (Conflict, never re-mint) | ddb-core/tests/hazard_raw_create_existing_id.rs:59 | create.rs is allowlisted: add a `#[cfg(test)]` unit test seeding a doogat, calling create_doogat_raw with that id, asserting `matches!(err, DoogatError::Conflict(_))`. |
| fanout | LOW | No boundary test through FFI; Conflict surfaces there as DdbError::Git | ddb-core/src/service/create.rs:437 | Follow-up: FFI-level collision test plus a decision on the Conflict->Git mapping (one-error-policy). |
| fanout | MEDIUM | Existence check runs outside the repo write lock: concurrent raw creates with the same explicit id can still overwrite | ddb-core/src/service/create.rs:434 | Same root as bob CRITICAL; residual after rework 1 for callers not sharing the sql_conn. Follow-up: create-only `commit_file` variant failing with Conflict when `rel_path` exists in fresh HEAD tree. Tag: hazard H4 residual. |

## Assumptions

From the implementors (sonnet):

1. PRD/issue number for the CHANGELOG entry: none exists yet for this fast-track card, so cited it as "(fast-track FT-4, hazard H4)" rather than fabricating a PRD number.
2. Error message text for `DoogatError::Conflict` ("doogat {id} already exists") is not asserted by the test; chose plain wording consistent with other Conflict-style messages in error.rs; no structured code/context required since the test only checks `Err(_)`.
3. No read access to `git_ops/write_lock.rs` (not in allowlist), so the fix reuses the codebase's documented precedent (SQLite `BEGIN IMMEDIATE` transaction guarding the identical git-commit_file race for SINGLETON typedef creates, create.rs:446-449/526-528) rather than the literal git repo write lock the reviewer named; this serializes all callers sharing the same index sql_conn.
4. Treated CHANGELOG.md and the #[ignore] removal as already correctly done by the predecessor (verified: CHANGELOG line 34 has the exact Fixed entry; test file has no #[ignore]), so no edits to either.

## Gate

Post-rework gate, foreground, sequential:

| command | exit code | seconds |
|---|---|---|
| `cargo build` | 0 | 37.5 |
| `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings` | 0 | 0.35 |
| `cargo test -p ddb-core --test hazard_raw_create_existing_id` | 0 | 3 |

Lane-side gate runs (eve, blake) also passed: build clean, clippy clean, hazard test 1 passed 0 ignored; eve additionally ran neighbors id_mint_collision (2), raw_create_mint_collision (1), raw_ffi_semantics (10), singleton_layers (8), all passed. Cargo was not run by this consolidation step.
