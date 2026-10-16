---
item: FT-3
date: 2026-09-06
base_sha: 876a153b4039128f8052526629773fa189058c71
head_sha: 876a153b4039128f8052526629773fa189058c71
outcome: blocked
implementor_model: sonnet
reworks: 1
lanes:
  fanout: CHANGES_REQUESTED
  eve: ok
  blake: ok
  bob: "ok: codex-run.sh exited 0; output at dev/local/tmp/FT-3-bob-output.txt (413KB transcript)"
  carl: "unavailable: gemini-run.sh (backend=copilot, model=gemini-3.8-flash) timed out; `timeout 540` killed it with exit code 124, no output file written"
---

# FT-3 consolidated fast-track review

## Summary

Outcome: **blocked**.

- Blocking findings before rework: **7** (2 fan-out, 2 Blake, 3 Bob).
- Remaining blocking findings after one rework: **4** (2 Blake, 2 Bob) — each carries a closure claim from the implementor that was not accepted as closure.
- Refuted during adversarial verification: **3** (all Bob).
- Advisory (non-blocking) findings: **40**.
- Lanes: fan-out CHANGES_REQUESTED; Eve, Blake, Bob ran; Carl unavailable (timeout, no findings).

The item stays blocked on two structural deliverables the spec named explicitly: the duplicated cascade walker in `ddb-core/src/sql_engine/dml.rs` (spec forbids a second walker) and the entirely missing `docs/src/technical/sql-engine.md` update, plus two data-integrity findings on the new cascade path (silently discarded typedef/schema/child-row errors, and index rows removed before fallible cleanup).

## Fan-out (Alice-engine)

---
prd: 
review: 1
date: 2026-09-06
head_sha: 876a153b4039128f8052526629773fa189058c71
reviewers: fanout
---

## Fanout

[FANOUT] 🟠 Cascade walk reports CASCADE_CYCLE when the same child is reached twice (diamond / two FK columns), permanently blocking a legal DELETE | File: ddb-core/src/sql_engine/dml.rs | Task: FT-3
[FANOUT] 🟠 New cycle detection rejects a legitimate delete: a child collected twice via two CASCADE columns raises a false CASCADE_CYCLE, and no test covers it | File: ddb-core/src/sql_engine/dml.rs | Task: FT-3

### Advisory (does not block)

- PRD-mandated docs update to sql-engine.md was not made
- A second cascade walker was written, which the PRD constraints explicitly forbid
- Bulk, transaction and RESTRICT/cycle cascade legs ship with no test, and a doc comment cites a test that does not cover them
- New cascade-children collector swallows schema and row failures with no warning, silently skipping cascade
- Two deleted doogats backlinked from the same file produce conflicting whole-file writes; the last one wins and one dangling wikilink survives
- Index rows for the whole cascade set are removed before the fallible ref-edit step, so a failure there leaves the index diverged from git
- New cascade-children scan silently drops typedefs and rows, dropping the warns the mirrored index version added on purpose
- New SQL text interpolates typedef-derived identifiers without the identifier-quoting helper
- Cascade scan reloads and reparses every typedef from git once per deleted id
- Second cascade walker duplicated instead of reusing the service planner
- Copied collect_cascade_children silently drops typedef rows and schema failures
- Index rows removed before the fallible junction cleanup, opposite the order service::delete documents as deliberate
- Required sql-engine.md doc update missing from the change
- delete_paths built eagerly in both legs but used only in the non-transaction branch
- Bulk DELETE cascade leg is rewritten but no test drives a cascade through it
- Transaction-buffer leg of the cascade delete is untested despite being an explicit PRD constraint
- Multi-level cascade recursion is untested — a one-level implementation would keep the only test green
- Doc comment cites hazard_sql_delete_cascade.rs as proof of the no-partial-state RESTRICT guarantee, but that file asserts nothing of the kind
- The un-ignored test accepts a blanket refusal, so it does not bind to the mandated cascade behavior
- No e2e scenario for the user-facing SQL cascade delete
- New cascade SQL interpolates unescaped identifiers, one of them read from a doogat's own `type:` frontmatter
- Cascade child whose path cannot be resolved is dropped silently, so a doogat the user asked to delete can survive with no error or warning

### Rubric

R1: fail
R2: pass
R3: pass
R4: pass
R6: pass
R7: pass
R8: fail
R9: fail
R10: fail
R11: pass
R12: pass
R13: fail

### Rubric notes

- R1: Only test change is removing #[ignore] from the single-row case (DELETE ... WHERE id = 'X'). New behavior with no test: cascade through delete_bulk_rows (root_ids fan-out + cross-root dedup in apply_c
- R2: sql_delete_of_cascade_parent_removes_child_or_refuses asserts the child is unreadable from git (NotFound) and gone from the membership table, so it binds to the cascade rule, not to a call shape; it w
- R3: The diff removes an #[ignore] and adds none; the two tests in the file run and pass (2 passed; 0 ignored).
- R4: delete_single_row / delete_bulk_rows signatures and Affected(n) semantics unchanged; verified locally: cargo test -p ddb-core --lib delete -> 58 passed, --lib cascade -> 18 passed, --test hazard_sql_d
- R7: Every value stays a bound parameter (params![deleted_id], params![id]); the target table is resolved through load_schema before any delete.
- R8: New code interpolates identifiers into SQL text without the codebase's quoting helper: dml.rs:820-823 builds format!("SELECT id FROM \"{}\" WHERE \"{}\" = ?1", table_name, col.name) where table_name c
- R9: Behavior matches (SQL DELETE now cascades, RESTRICT still refuses), but two explicit PRD requirements are unmet. (1) Constraint "Do not write a second cascade walker - reuse the planner the service pa
- R10: The new cascade collection swallows errors on a delete path. dml.rs:806 and :827 use .filter_map(/r/ r.ok()) for the typedef list and the child-id rows, where the mirrored source (indexer/materialize.
- R12: apply_cascade_delete 855-904 (~49 lines), collect_cascade_children 797-834 (38), build_cascade_delete_plan 758-782 (25) - all under 50.
- R13: ddb-core/src/sql_engine/dml.rs is 1787 lines, well over the 800 max. Pre-existing overflow, but this diff adds ~160 lines to it (including two functions duplicated from other modules) instead of shrin

_engine: workflow — dimensions 5, raw 24, unique 24, confirmed 2, refuted 0, demoted 0, unverified 0, diff_bytes 14363_

Verdict: 2 findings
Tests: {{TESTS_LINE}}

## Eve

lane_status: **ok**

Doubt + de-slop review of FT-3 (876a153..HEAD, staged). Gate rerun locally: cargo build OK; cargo +1.89.0 clippy --workspace --all-targets -D warnings clean; hazard_sql_delete_cascade 2/2; --lib delete 58/58; --lib cascade 18/18. Scratchpad probe crate (path dep on ddb-core, not in repo) exercised bulk, txn commit, txn rollback, cycle, child-level RESTRICT, and diamond graphs through execute_sql; all safe. Findings: 3 FIX, 1 VERIFY, 6 KNOWN. No files in the repo modified.

| severity | title | file | proof |
| --- | --- | --- | --- |
| MEDIUM | Required docs update to docs/src/technical/sql-engine.md not delivered | docs/src/technical/sql-engine.md:440 | N/A (MEDIUM). Evidence: changed files are only CHANGELOG.md, dml.rs, and the hazard test; sql-engine.md untouched, though the card's Docs section is explicit. |
| MEDIUM | Full sql_engine and service lib suites not run in this lane (card note delegates them to the driver) | ddb-core/src/sql_engine/dml.rs:686 | Card gate deliberately excludes `--lib sql_engine` (~596 s); only name-filtered `delete` (58) and `cascade` (18) plus the hazard test were run, so "existing sql_engine delete tests stay green" is only partially proven. |
| LOW | SqlEngine::collect_cascade_children silently drops decode and schema-load failures where the Index version warns | ddb-core/src/sql_engine/dml.rs:812 | dml.rs:812-815 `Err(_) => continue`; :806/:827 `filter_map(|r| r.ok())`. materialize.rs:758-768 warns deliberately (PRD 00140); sibling cascade_junction_cleanup (dml.rs:1411-1445) warns on both arms. |
| LOW | Per-id delete order differs from the service path: index row removed before junction cleanup | ddb-core/src/sql_engine/dml.rs:880 | apply_cascade_delete runs remove_doogat (:880), typed-table DELETE (:883), then cascade_junction_cleanup (:887); service/delete.rs:99-108 deliberately runs the fallible cleanup first. No SAVEPOINT outside a txn. |
| LOW | Second cascade walker duplicated instead of reusing the service planner (constraint violation, visibility-forced) | ddb-core/src/sql_engine/dml.rs:758 | SqlEngine holds `&dyn SqlBackend` (sql_engine/mod.rs:72); collect_cascade_children is on IndexPort only (traits.rs:216). Real fix touches files outside the allowlist. |
| LOW | Diamond-shaped cascade graph is rejected as CASCADE_CYCLE on both paths | ddb-core/src/sql_engine/dml.rs:771 | Probe: root R, mid A/B, leaf L with m1->A and m2->B; SQL and service both return Structured{code:"CASCADE_CYCLE"}. Identical on service path (delete.rs:51). |
| LOW | Bulk and transaction cascade legs have no regression test | ddb-core/tests/hazard_sql_delete_cascade.rs:72 | Only new test covers `DELETE ... WHERE id = 'X'` outside a txn; probe confirmed bulk Affected(2) and txn COMMIT/ROLLBACK work today but nothing pins them. |
| LOW | NoSQL mirror not updated for SQL DELETE (root or cascade children) | ddb-core/src/sql_engine/dml.rs:855 | service/delete.rs:109 calls nosql_remove_doogat per planned id; SQL engine has no NoSqlMirrorPort access and never did. |
| LOW | Hazard test header comment now describes the pre-fix state | ddb-core/tests/hazard_sql_delete_cascade.rs:3 | Lines 3-9 still say delete_single_row/delete_bulk_rows "call only check_restrict_blocks_delete" and cite dml.rs:681-712/:727. |
| LOW | CASCADE_CYCLE context lists a doogat id under 'tables' | ddb-core/src/sql_engine/dml.rs:772 | `DoogatError::cascade_cycle([child_table, parent.clone()])`; probe output tables=["node_a", "20260907004901"]. Copied verbatim from service/delete.rs:52. |

Verdict lines:

```
D1: pass
D2: pass
D3: pass
D4: pass
D5: pass
```

## Blake

lane_status: **ok**

Found the FT-3 work staged (uncommitted) touching exactly the allowlisted CHANGELOG.md, ddb-core/src/sql_engine/dml.rs, and ddb-core/tests/hazard_sql_delete_cascade.rs (only the `#[ignore]` line removed there). Full gate green: cargo build clean, cargo +1.89.0 clippy --workspace --all-targets -D warnings clean, hazard_sql_delete_cascade 2/2, --lib delete 58/58, --lib cascade 18/18. Functionally the fix works, but it violates an explicit spec constraint (second cascade walker) and skips the mandatory docs update.

| severity | title | file | proof |
| --- | --- | --- | --- |
| HIGH | Second cascade walker written instead of reusing the service planner (spec explicitly forbids this) | ddb-core/src/sql_engine/dml.rs:748 | Spec Constraints: "Do not write a second cascade walker — reuse the planner the service path already uses". The duplicate already diverges: materialize.rs:762-768 logs `tracing::warn!` on child-id decode failure; dml.rs:824-827 silently drops it via `.filter_map(|r| r.ok())`, so a corrupted index row goes unnoticed on the SQL path but is logged on the service path. |
| HIGH | Required sql-engine.md documentation update is missing entirely | docs/src/technical/sql-engine.md | Spec's "Docs and CHANGELOG" section is mandatory: state that ON DELETE CASCADE now holds on `DELETE FROM <type>` as well as the service delete, naming both entry points and citing the proving file:line. Zero lines changed in docs/. |
| MEDIUM | No test exercises cascade delete inside an open transaction buffer | ddb-core/src/sql_engine/dml.rs:689 | Only txn+delete test is `insert_then_delete_within_txn` (tests.rs:1541), on a table with no CASCADE columns; the `if let Some(ref mut buf) = self.txn` branches at dml.rs:689/725 are unexercised for cascade. |
| MEDIUM | dml.rs exceeds the 800-line file-size limit | ddb-core/src/sql_engine/dml.rs | 1787 lines after this change; the diff adds ~140 net lines of duplicated cascade-walk logic. |
| LOW | Duplicated cascade walker silently swallows row-decode and schema-load errors that the reference logs | ddb-core/src/sql_engine/dml.rs:826 | dml.rs:805-807, :826-827 use `.filter_map(|r| r.ok())`; :813-815 uses `Err(_) => continue`; materialize.rs:762-768 and dml.rs:1444 both `tracing::warn!`. |
| LOW | New cascade SQL continues the pre-existing pattern of unescaped identifier interpolation | ddb-core/src/sql_engine/dml.rs:820 | `format!("SELECT id FROM \"{}\" WHERE \"{}\" = ?1", table_name, col.name)` and `format!("DELETE FROM \"{}\" WHERE id = ?1", dtype)` bypass `escape_sql_ident`/`quote_ident`. |

Verdict lines:

```
B1: fail
B2: pass
B3: pass
B4: pass
B6: pass
B7: pass
B8: pass
B9: fail
B10: fail
B11: pass
B12: pass
B13: fail
```

## Bob (codex)

lane_status: **ok** — codex-run.sh exited 0; output at `/Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-3-bob-output.txt` (413KB transcript, findings block at line 7235).

| severity | title | file | proof |
| --- | --- | --- | --- |
| HIGH | Transactional RESTRICT reads committed typedefs while CASCADE discovery reads buffered schemas | ddb-core/src/sql_engine/dml.rs:767 | A required reference created or altered earlier in the same transaction can be missed, allowing its parent to be deleted. (Refuted — see Adversarial verification.) |
| HIGH | Typedef-row, schema-load, and child-row errors silently discarded | ddb-core/src/sql_engine/dml.rs:803 | Cascade descendants can survive a successful parent deletion without even a warning. |
| HIGH | Unescaped user-influenced table and column identifiers in SQL | ddb-core/src/sql_engine/dml.rs:820 | Injection-shaped selection/deletion or index corruption via user-influenced type/column names. (Refuted — see Adversarial verification.) |
| HIGH | Reference edits do not compose across multiple deleted targets | ddb-core/src/sql_engine/dml.rs:893 | One surviving doogat linking to multiple deleted nodes: each edit rereads the original file, and the last duplicate-path write restores earlier dangling links. |
| HIGH | Index rows removed before fallible cleanup steps | ddb-core/src/sql_engine/dml.rs:870 | An error mid-cleanup leaves SQLite partially deleted while git remains unchanged, unlike the service executor's ordering. |
| HIGH | Tests cover only single-row success | ddb-core/tests/hazard_sql_delete_cascade.rs:72 | Bulk cascading, transaction buffering, recursive cascades, SQL-path RESTRICT, cycle rejection, and error atomicity remain untested. (Refuted — see Adversarial verification.) |
| MEDIUM | Planner and collector duplicate the service implementation | ddb-core/src/sql_engine/dml.rs:748 | ~150 duplicated lines added to an already over-800-line file despite the explicit reuse requirement. |
| MEDIUM | Acceptance test uses an either-or oracle | ddb-core/tests/hazard_sql_delete_cascade.rs:83 | The activated test accepts a structured refusal even though FT-3 mandates cascading. |
| MEDIUM | Required documentation update absent | docs/src/technical/sql-engine.md:440 | The existing section neither names `DoogatService::delete_doogat` alongside SQL DELETE nor cites the implementation location. |
| LOW | Cannot statically verify build, clippy, and test gates | N/A | Static-only lane; gates were verified independently by Eve and Blake. |

Verdict lines:

```
R1: fail
R2: fail
R3: pass
R4: fail
R6: pass
R7: fail
R8: fail
R9: fail
R10: fail
R11: pass
R12: pass
R13: fail
```

## Carl (gemini)

lane_status: **unavailable**

`gemini-run.sh` (backend=copilot, model=gemini-3.8-flash) timed out: `timeout 540` killed it with exit code 124 and no output file was written (`dev/local/tmp/FT-3-carl-output.txt` does not exist). The helper was still mid-exploration (last action: `cargo clippy --workspace --all-targets -- -D warnings`) when killed, so it never emitted findings or rubric verdicts. Side effect noted: it created and then deleted its own probe test `ddb-core/tests/zz_probe_cascade.rs`; post-run `git status --short` shows only the FT-3 diff files, so the checkout is clean of reviewer debris.

| severity | title | file | proof |
| --- | --- | --- | --- |
| — | (no findings; lane timed out) | — | — |

Verdict lines: none produced.

## Adversarial verification

Every candidate blocking finding, with its verdict and reason.

### Confirmed

1. **Second cascade walker written instead of reusing the service planner** (Blake, HIGH, `ddb-core/src/sql_engine/dml.rs:748`) — **confirmed**. `service::delete` is a private `mod delete;` (service/mod.rs:13) unreachable from sql_engine, and `collect_cascade_children` lives only on `IndexPort` (traits.rs:216) while `SqlEngine.index` is `&dyn SqlBackend` (sql_engine/mod.rs:72), so the stated reachability excuse is real — but the spec explicitly sanctioned fixing it via a shared crate-private helper instead of duplicating the walker. The duplicate already diverges from its reference (materialize.rs:760-768 warns and skips on a child-id decode failure; the dml.rs copy silently drops it), so a corrupted/non-text id in a CASCADE-referenced column vanishes from the cascade set with no log on the SQL delete path though it is logged on the service path — verified drift of exactly the kind the anti-duplication constraint exists to prevent.

2. **Required sql-engine.md documentation update is missing entirely** (Blake, HIGH, `docs/src/technical/sql-engine.md`) — **confirmed**. `git diff --cached --stat` and `git diff HEAD -- docs/src/technical/sql-engine.md` both show zero changes, while the spec's mandatory "Docs and CHANGELOG" section requires naming both cascade entry points and citing proof lines. The existing CASCADE section (lines 440-457, e.g. line 442 "all in a single git commit") is untouched and still makes no mention of the SQL DELETE entry point.

3. **Typedef-row, schema-load, and child-row errors silently discarded** (Bob, HIGH, `ddb-core/src/sql_engine/dml.rs:803`) — **confirmed**. `collect_cascade_children` discards every read error with no `tracing::warn!` (803-807 typedef-title row decode via `.filter_map(|r| r.ok())`, 812-815 `load_schema` failure via `Err(_) => continue`, 825-827 child-id row decode), although its own doc comment claims it mirrors `Index::collect_cascade_children` exactly — and that reference (materialize.rs:611-668, 737-775) calls `tracing::warn!` at each of those three points, added by PRD 00140 precisely to stop cascade children being dropped silently. Failing input: a typedef T with a CASCADE column and a live child row pointing at doogat X, where T's typedef-row decode, T's schema load, or the child-id decode errors while `DELETE FROM <parent-type> WHERE id = 'X'` runs. Consequence: T is skipped, the child is never planned, `apply_cascade_delete` reports `Affected(1)` as success, and the CASCADE child survives in git and the index with zero diagnostic — a violation of the repo invariant against silently dropping rows, schemas, or parse failures.

4. **Index rows removed before fallible cleanup steps** (Bob, HIGH, `ddb-core/src/sql_engine/dml.rs:870`) — **confirmed**. In `apply_cascade_delete` (dml.rs:870-890) each id runs `self.index.remove_doogat(id)?` and the typed-table DELETE before the fallible `self.cascade_junction_cleanup(dtype, id)?`, and for a single-statement `DELETE FROM <type> WHERE id=...` `execute_batch` only opens an implicit SQLite transaction when `statements.len() > 1` (mod.rs:290), so those `execute()` calls auto-commit with no savepoint. Failing input: `DELETE FROM link WHERE id='X'` where link's own typedef fails to load — planning swallows the `load_schema` failure (`Err(_) => continue`, dml.rs:812-815) so the plan is built, but `cascade_junction_cleanup`'s `table_name == target_type` branch (dml.rs:1438-1443) treats the same failure as fatal. `remove_doogat` and `DELETE FROM "link"` have already committed to SQLite, the `?` propagates, `commit_batch` never runs, git is untouched — the exact class the service path guards against by ordering junction cleanup first (service/delete.rs:99-107).

5. **Cascade walk reports CASCADE_CYCLE when the same child is reached twice** (fan-out, HIGH, `ddb-core/src/sql_engine/dml.rs`) — **confirmed** at review time by the fan-out engine (confirmed 2 of 2 candidates; 0 refuted).

6. **New cycle detection rejects a legitimate delete (two CASCADE columns), no test covers it** (fan-out, HIGH, `ddb-core/src/sql_engine/dml.rs`) — **confirmed**, same engine pass; the diamond / two-column case was reproduced independently by Eve's probe on both the SQL and service paths.

### Refuted

1. **Transactional RESTRICT reads committed typedefs while CASCADE discovery reads buffered schemas** (Bob, HIGH) — **refuted**. The claimed failure needs a required-reference column created or altered earlier in the same transaction to actually hold a matching row value, but `rematerialize_or_defer` (ddb-core/src/sql_engine/ddl.rs:809-823) defers a type's materialized-table rebuild to `handle_commit` whenever a transaction buffer is open, so an in-transaction ALTER/CREATE adding a REFERENCES/RESTRICT column does not touch the live SQLite table until COMMIT and no in-transaction INSERT/UPDATE can populate it. For a pre-existing RESTRICT column, `load_all_typedefs(repo)` (materialize.rs:611-668, unchanged) reflects the current schema and row writes materialize immediately, so `check_restrict_blocks_delete` sees them as before. The schema-source asymmetry is a real code fact and predates this diff, but no reachable SQL sequence exploits it.

2. **Unescaped user-influenced table and column identifiers in SQL** (Bob, HIGH) — **refuted** *for this diff*. The unescaped `format!` interpolation at dml.rs:820-823 is a verbatim duplicate of the already-shipped `Index::collect_cascade_children` (materialize.rs:753-756) and `check_restrict_blocks_delete` (materialize.rs:710-713), which the live service delete path (service/delete.rs:48-49, reachable via `ddb delete`/GraphQL/REST/FFI) already calls unescaped in production. The item card mandated mirroring that planner, so the diff reproduces pre-existing behavior; the injection risk predates it and is tracked under 00179. (Retained as advisory.)

3. **Tests cover only single-row success** (Bob, HIGH) — **refuted** as a blocking defect. It is a coverage complaint naming no concrete failing input, which the packet's rule requires for HIGH/CRITICAL. The item card's Tests section names exactly the two tests present as the required set, and its Allowlist restricts the test file to removing the `#[ignore]` attribute only; full sql_engine coverage is deliberately delegated to the driver's `--lib sql_engine` / `--lib service` runs and the batch-end `cargo test-ci`. (Retained as advisory.)

## Rework

One rework pass ran. The implementor supplied closure claims for four blocking findings; none was accepted, so all four remain open. Closure claims as submitted (note the implementor's claims are cross-linked to the findings below as recorded by the harness):

- **Second cascade walker (Blake, HIGH)** — claim: `build_cascade_delete_plan` (dml.rs:758-794) now tracks a BFS spanning-tree `parent_of` map alongside `seen`, and a revisited id raises `CASCADE_CYCLE` only when `Self::is_ancestor(parent_of, parent, child_id)` is true (dml.rs:798-809); a second arrival at an already-planned non-ancestor id is deduped via `continue` (dml.rs:782). Verified by static trace (self-loop and P->A,P->B,A->C,B->C diamond both resolve `is_ancestor` false; a genuine A->B->A still errors) and empirically by two temporary tests added to `hazard_sql_delete_cascade.rs`, run with `cargo test -p ddb-core --test hazard_sql_delete_cascade scratch_` (both passed: `Affected(1)`, node unreadable, no `CASCADE_CYCLE`), then reverted. **Not closure**: this addresses the cycle false positive, not the duplicated walker.

- **Missing sql-engine.md docs (Blake, HIGH)** — claim describes the same ancestor-tracking fix plus a throwaway probe test (`ddb-core/tests/probe_two_cascade_cols.rs`, run then deleted) reproducing the two-CASCADE-column schema through the real SQL path: before the fix `Err(CASCADE_CYCLE)`, after it `PROBE result=Ok(Affected(1)) parent=false child=false`. **Not closure**: the docs file was never touched.

- **Typedef/schema/child-row errors silently discarded (Bob, HIGH)** — claim: the diff still contains the hand-copied walker (`build_cascade_delete_plan` dml.rs:~758, `collect_cascade_children` dml.rs:~791) with the same visibility excuse in its doc comment; `git log --oneline -- ddb-core/src/service/delete.rs` shows its last commit is the base 876a153 and `service/mod.rs` still declares `mod delete;` private. What did change: the row-decode step now does `.map_err(|e| tracing::warn!(...)).ok()` for both the typedef-name and child-id queries. **Not closure**: it patches the copy rather than removing the duplication, and does not cover the `load_schema` failure arm.

- **Index rows removed before fallible cleanup (Bob, HIGH)** — claim: `git diff --stat -- docs/` over 876a153b..working tree returns nothing; the diff touches only CHANGELOG.md, dml.rs, and hazard_sql_delete_cascade.rs; sql-engine.md:442 still reads "Cascade walks recursively through chains of CASCADE references, all in a single git commit" with no correction and no naming of both entry points. **Not closure**: the claim addresses the docs finding, not the ordering defect; the `remove_doogat`-before-`cascade_junction_cleanup` order at dml.rs:870-890 is unchanged.

Net: 7 blocking before rework → 4 remaining after.

## Advisory

Non-blocking findings from all lanes (40 total, including duplicates across lanes; grouped by lane).

### Eve

| severity | title | file |
| --- | --- | --- |
| MEDIUM | Required docs update to docs/src/technical/sql-engine.md not delivered | docs/src/technical/sql-engine.md:440 |
| MEDIUM | Full sql_engine and service lib suites not run in this lane (card delegates them to the driver) | ddb-core/src/sql_engine/dml.rs:686 |
| LOW | SqlEngine::collect_cascade_children silently drops decode and schema-load failures where the Index version warns | ddb-core/src/sql_engine/dml.rs:812 |
| LOW | Per-id delete order differs from the service path: index row removed before junction cleanup | ddb-core/src/sql_engine/dml.rs:880 |
| LOW | Second cascade walker duplicated instead of reusing the service planner (visibility-forced) | ddb-core/src/sql_engine/dml.rs:758 |
| LOW | Diamond-shaped cascade graph is rejected as CASCADE_CYCLE on both paths | ddb-core/src/sql_engine/dml.rs:771 |
| LOW | Bulk and transaction cascade legs have no regression test | ddb-core/tests/hazard_sql_delete_cascade.rs:72 |
| LOW | NoSQL mirror not updated for SQL DELETE (root or cascade children) | ddb-core/src/sql_engine/dml.rs:855 |
| LOW | Hazard test header comment now describes the pre-fix state | ddb-core/tests/hazard_sql_delete_cascade.rs:3 |
| LOW | CASCADE_CYCLE context lists a doogat id under 'tables' | ddb-core/src/sql_engine/dml.rs:772 |

### Blake

| severity | title | file |
| --- | --- | --- |
| MEDIUM | No test exercises cascade delete inside an open transaction buffer | ddb-core/src/sql_engine/dml.rs:689 |
| MEDIUM | dml.rs exceeds the 800-line file-size limit (1787 lines) | ddb-core/src/sql_engine/dml.rs |
| LOW | Duplicated cascade walker silently swallows row-decode and schema-load errors the reference logs | ddb-core/src/sql_engine/dml.rs:826 |
| LOW | New cascade SQL continues the pre-existing pattern of unescaped identifier interpolation | ddb-core/src/sql_engine/dml.rs:820 |

### Bob (codex)

| severity | title | file |
| --- | --- | --- |
| MEDIUM | Planner and collector duplicate the service implementation | ddb-core/src/sql_engine/dml.rs:748 |
| MEDIUM | Acceptance test uses an either-or oracle | ddb-core/tests/hazard_sql_delete_cascade.rs:83 |
| MEDIUM | Required documentation update absent | docs/src/technical/sql-engine.md:440 |
| LOW | Cannot statically verify build, clippy, and test gates | N/A |
| HIGH (refuted) | Transactional RESTRICT vs CASCADE schema-source asymmetry | ddb-core/src/sql_engine/dml.rs:767 |
| HIGH (refuted) | Unescaped user-influenced identifiers (pre-existing, tracked 00179) | ddb-core/src/sql_engine/dml.rs:820 |
| HIGH (refuted) | Tests cover only single-row success (out of card scope) | ddb-core/tests/hazard_sql_delete_cascade.rs:72 |

### Fan-out (non-blocking)

| severity | title | file |
| --- | --- | --- |
| MEDIUM | PRD-mandated docs update to sql-engine.md was not made | docs/src/technical/sql-engine.md:420 |
| MEDIUM | A second cascade walker was written, which the PRD constraints explicitly forbid | ddb-core/src/sql_engine/dml.rs:758 |
| MEDIUM | Bulk, transaction and RESTRICT/cycle cascade legs ship with no test; a doc comment cites a test that does not cover them | ddb-core/src/sql_engine/dml.rs:849 |
| MEDIUM | New cascade-children collector swallows schema and row failures with no warning | ddb-core/src/sql_engine/dml.rs:814 |
| MEDIUM | Two deleted doogats backlinked from the same file produce conflicting whole-file writes; last one wins, one dangling wikilink survives | ddb-core/src/sql_engine/dml.rs:895 |
| MEDIUM | Index rows for the whole cascade set removed before the fallible ref-edit step | ddb-core/src/sql_engine/dml.rs:870 |
| MEDIUM | New cascade-children scan silently drops typedefs and rows, dropping the mirrored version's deliberate warns | ddb-core/src/sql_engine/dml.rs:806 |
| MEDIUM | New SQL text interpolates typedef-derived identifiers without the quoting helper | ddb-core/src/sql_engine/dml.rs:820 |
| MEDIUM | Second cascade walker duplicated instead of reusing the service planner (quality dimension) | ddb-core/src/sql_engine/dml.rs:758 |
| MEDIUM | Copied collect_cascade_children silently drops typedef rows and schema failures | ddb-core/src/sql_engine/dml.rs:806 |
| MEDIUM | Index rows removed before the fallible junction cleanup, opposite service::delete's deliberate order | ddb-core/src/sql_engine/dml.rs:880 |
| MEDIUM | Required sql-engine.md doc update missing from the change | docs/src/technical/sql-engine.md |
| MEDIUM | Bulk DELETE cascade leg rewritten but no test drives a cascade through it | ddb-core/src/sql_engine/dml.rs:721 |
| MEDIUM | Transaction-buffer leg of the cascade delete untested despite an explicit PRD constraint | ddb-core/src/sql_engine/dml.rs:690 |
| MEDIUM | Multi-level cascade recursion untested — a one-level implementation keeps the only test green | ddb-core/tests/hazard_sql_delete_cascade.rs:32 |
| MEDIUM | Doc comment cites hazard_sql_delete_cascade.rs as proof of a no-partial-state guarantee that file never asserts | ddb-core/src/sql_engine/dml.rs:849 |
| MEDIUM | The un-ignored test accepts a blanket refusal, so it does not bind to the mandated cascade behavior | ddb-core/tests/hazard_sql_delete_cascade.rs:93 |
| MEDIUM | New cascade SQL interpolates unescaped identifiers, one read from a doogat's own `type:` frontmatter | ddb-core/src/sql_engine/dml.rs:884 |
| LOW | Cascade scan reloads and reparses every typedef from git once per deleted id | ddb-core/src/sql_engine/dml.rs:797 |
| LOW | delete_paths built eagerly in both legs but used only in the non-transaction branch | ddb-core/src/sql_engine/dml.rs:688 |
| LOW | No e2e scenario for the user-facing SQL cascade delete | ddb-core/tests/hazard_sql_delete_cascade.rs:71 |
| LOW | Cascade child whose path cannot be resolved is dropped silently | ddb-core/src/sql_engine/dml.rs:775 |

Notable advisory detail (fan-out, security dimension): a doogat whose `type:` is `a" WHERE id = ?1 OR 1=1 --` renders `DELETE FROM "a" WHERE id = ?1 OR 1=1 --" WHERE id = ?1` — one bound parameter, single statement, so both the parameter-count check and the MultipleStatement guard pass, and the statement wipes every row of typed table `a` from the index. Damage is confined to the rebuildable SQLite index (git untouched), hence MEDIUM.

## Assumptions

Recorded by the implementors:

1. `service/delete.rs` ended up unedited (a refactor there was attempted then reverted) since true sharing was not reachable within the edit scope; it remains in its original form.
2. `docs/src/technical/sql-engine.md` was left unedited because it is only in the read-only context list, not the explicit "Files you may modify" allowlist, despite the card's Docs task asking for it — flagged as an instruction conflict rather than guessed at.
3. Per-id deletion order in the new `apply_cascade_delete` (`remove_doogat` -> typed-table delete -> junction cleanup, for all ids first, then dangling-reference edits) matches the original single-row dml.rs ordering exactly for the non-cascade case and was extended uniformly to multiple ids; this differs from `service/delete.rs`'s order (ref-edits collected before deletions) but keeps existing sql_engine delete tests' behavior unchanged.
4. `doogat_type` is looked up via `SELECT type FROM doogats WHERE id = ?1` for every id in the plan (including the root) rather than trusting the caller-supplied `table_name` only for the root, for uniform per-id handling matching the service's `execute_delete_plan` pattern.
5. `cargo test -p ddb-core --lib sql_engine` (the ~596s suite) was intentionally NOT run per the card's own gate note; the delete/cascade name-filtered subsets plus the hazard test were run instead.
6. Findings #3 and #4 are structurally out of scope for the rework pass given the allowlist (traits.rs/ports.rs/materialize.rs not readable to safely verify the shared-planner refactor; sql-engine.md read-only) — left unaddressed and flagged rather than guessed at.
7. For findings #1/#2's cycle-vs-dedup fix, "genuine cycle" was treated as: the revisited child id is an ancestor of the current parent in the BFS discovery tree (a real back-edge); everything else (diamond, or the same child via two CASCADE columns of one row) is a safe duplicate to skip — matching the reviewers' suggested fix text.
8. For finding #6, edits were grouped by `source_path` (not `source_id`) since `PendingWrite` is keyed by path and `commit_batch`/`txn.writes` dedups on `source_path`.
9. `service/delete.rs` was deliberately not touched: reviewers noted the identical cycle-detection bug exists there too, but the allowlist restricts edits there to "no behavior change to the service path" unless lifting the planner to a shared helper (finding #3, blocked), so it stays a documented pre-existing gap.

Files touched by the item: `ddb-core/src/sql_engine/dml.rs`, `ddb-core/tests/hazard_sql_delete_cascade.rs`, `CHANGELOG.md`.

## Gate

| command | exit code | seconds |
| --- | --- | --- |
| `cargo build` | 0 | 64 |
| `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings` | 0 | 90 |
| `cargo test -p ddb-core --test hazard_sql_delete_cascade` | 0 | 13 |
| `cargo test -p ddb-core --lib delete` | 0 | 138 |
| `cargo test -p ddb-core --lib cascade` | 0 | 56 |

All five gate commands passed. Gate green does not lift the block: the four remaining findings are spec-deliverable and data-integrity defects that no gate command exercises. Not run locally by design: `cargo test -p ddb-core --lib sql_engine` and `--lib service` (delegated to the driver / batch-end `cargo test-ci`).

## Driver exit notes

Driver (Opus 5, S2) accepts `blocked` and applies decision D5: the work is stashed as `FT-3`, master is untouched at `876a153`. The driver did NOT re-run the gate or read the full diff line by line, because the block is structural and does not turn on diff detail.

Gate seconds here are credible measurements, not cache hits: the trimmed gate (`--lib delete` 138 s, `--lib cascade` 56 s) replaced FT-2's 596 s `--lib sql_engine`, so the item stayed well clear of the 600 s subagent Bash cap. That trim worked and should be kept for future sql_engine items.

Why it is blocked, in the driver's words:

1. **The card's central constraint was violated.** The card said, verbatim, "Do not write a second cascade walker — reuse the planner the service path already uses," and allowlisted `ddb-core/src/service/delete.rs` precisely so the walker could be lifted to a shared crate-private helper. The implementor instead hand-copied `build_cascade_delete_plan` and `collect_cascade_children` into `sql_engine/dml.rs`, citing a real visibility fact (`service::delete` is a private `mod`, and `collect_cascade_children` lives on `IndexPort` while `SqlEngine.index` is `&dyn SqlBackend`) as the reason. The fact is true; the conclusion is not — the allowlist named the file that would have fixed it. The copy already drifted from its source before it was even committed (the reference warns on a child-id decode failure, the copy dropped it silently), which is the exact failure mode the constraint exists to prevent.
2. **A mandatory deliverable is simply absent.** The card required a `docs/src/technical/sql-engine.md` update naming both cascade entry points. `git diff HEAD -- docs/` is empty.
3. **Two data-integrity defects, both the same class FT-2's rework fixed.** `collect_cascade_children`'s copy discards typedef-row decode, `load_schema`, and child-id decode errors, so a cascade child can vanish with `Affected(1)` reported as success — against the repo's "never silently drop rows, schemas, or parse failures" invariant. And `apply_cascade_delete` runs `remove_doogat` plus the typed-table DELETE before the fallible junction cleanup, with no savepoint for a single-statement DELETE, so a fatal typedef load leaves SQLite mutated and git untouched. FT-2 closed this exact ordering hazard on the service path one commit earlier; the SQL path reintroduced it.

Findings the rework partly answered but did not close: the cycle-detection false positive IS genuinely fixed (BFS spanning-tree `parent_of` plus an `is_ancestor` check, verified empirically by the verifier's own probe), and one of the three silent-drop arms now warns. That work is worth keeping, which is why this is a stash and not a discard.

New finding worth its own card, raised by Eve and not in scope here: **the diamond-shaped cascade graph is rejected as `CASCADE_CYCLE` on the SERVICE path too** (`service/delete.rs` `build_cascade_delete_plan`, the `if !seen.insert(...)` check). That is a live bug on master today — a legal `DELETE` of a parent whose child is reachable by two CASCADE columns is permanently refused through `ddb delete`, GraphQL, REST and FFI. FT-3's stashed work contains the fix shape (ancestor tracking) for the SQL copy; the service path needs the same treatment plus a hazard test.

Retry shape for S3 (not attempted here): one item, opus, whose Goal is "lift the cascade planner to one crate-private helper both paths call" with the shared-helper step as an explicit numbered task rather than a constraint, the docs paragraph named as a deliverable in the Tests section so a lane can check it, and the ordering fix (cleanup before `remove_doogat`) stated as a copy of what `876a153` did to `service/delete.rs`. Recover the stash with `git stash pop` (or `git stash show -p stash@{0}`) rather than restarting from the base — the cycle-detection fix and one warn arm are already correct.

Lane note (D6): carl unavailable again, this time the S1 failure mode — `gemini-run.sh` (backend=copilot, model=gemini-3.8-flash) killed by `timeout 540` with exit 124 and no output file. Two consecutive items with carl down (FT-2: exited 0 with an empty review; FT-3: hard timeout). Under D6 a second unavailable lane stops the batch; only one lane is down, so the batch continues, but carl has now failed 3 of 4 fast-track items across S1 and S2 and should be treated as unreliable rather than intermittent.
