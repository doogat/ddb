---
item: FT-2
date: 2026-09-06
base_sha: 4a248cb0c1792d922b5e56600330c34e532e7ab8
head_sha: 4a248cb0c1792d922b5e56600330c34e532e7ab8
outcome: ready
implementor_model: sonnet
reworks: 1
lanes:
  fanout: APPROVE
  eve: ok
  blake: ok
  bob: ok
  carl: failed
---

## Summary

Outcome: **ready**. Two HIGH blocking findings were raised before rework (both from the bob lane) and both were closed by the single rework cycle; nothing blocking remains.

- Blocking before rework: 2
- Remaining blocking: 0
- Refuted candidates: 0
- Advisory (non-blocking) findings: 27

Lane status:

| Lane | Status | Detail |
| --- | --- | --- |
| Fan-out (Alice-engine) | APPROVE | No issues found; 12 advisory items, rubric R1 and R13 fail |
| Eve | ok | Doubt + de-slop; core fix correct, residue advisory |
| Blake | ok | Blind/PRD-only; one LOW file-size finding |
| Bob (codex, gpt-5.6-sol) | ok | Ran to completion, exit 0 |
| Carl (gemini via copilot) | failed | Exited 0 after ~45s with no review body; no findings, no verdicts |

## Fan-out (Alice-engine)

---
prd: 
review: 1
date: 2026-09-06
head_sha: 4a248cb0c1792d922b5e56600330c34e532e7ab8
reviewers: fanout
---

## Fanout

[FANOUT] ✅ No issues found

### Advisory (does not block)

- Service delete gains an unrequested fatal path when the deleted doogat's own typedef fails to load, aborting after the index row is already gone
- Hazard test module doc still describes the bug as present after the pin was activated
- Owner-side schema fallback makes a previously-successful delete fail after the index rows are already gone
- IndexPort trait doc still describes junction cleanup as target-side only
- New single-caller schema loader duplicates load_all_typedefs and turns a warn-skip into a mid-delete hard failure
- Needless full-schema clone where an immutable borrow works
- Shared junction sweep lands in an already-oversized indexer/mod.rs instead of the module owning junction logic
- GraphQL delete docs still describe one-direction junction cleanup
- New load_single_typedef_schema fallback (both branches) has no test
- Only a CI-only e2e pins the reverse direction of the refactored service cleanup
- Hazard test's module doc still asserts the hazard is live and unpinned
- Shared junction-cleanup helper interpolates typedef-derived SQL identifiers without escape_sql_ident

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
R12: pass
R13: fail

### Rubric notes

- R1: The primary new behavior (owner-side junction cleanup on the service delete path) is covered by the un-ignored hazard test, which passes. But the diff also introduces new error semantics with no test:
- R2: The un-ignored test asserts owner-side rows count 0 after delete_doogat; the pre-change code (visible in the diff) only swept the reverse direction, so the assertion could not have passed before the f
- R3: The diff removes an #[ignore]; no test is skipped or weakened. 376 sql_engine and 266 indexer lib tests ran with 0 ignored, 0 failed.
- R4: Index::cascade_junction_cleanup keeps its signature, so service/delete.rs:107, indexer/ports.rs:202-208, traits.rs:223 and service/mock_index_tests.rs:188 are untouched; the SqlEngine leg delegates to
- R7: deleted_id and type_name are bound parameters; target_type originates from the indexed doogat type, not raw user text.
- R8: All values are bound parameters. Identifier interpolation (\"{jt}\", \"{col_id}\") is moved verbatim from both prior sites with no change in quoting; it still does not route through escape_sql_ident, 
- R9: Both directions now run from one shared sweep; the SqlEngine asymmetric error policy (fatal on target's own typedef, warn-and-skip otherwise) is preserved verbatim and mirrored on the indexer side; th
- R12: Largest touched function is SqlEngine::cascade_junction_cleanup at 54 raw lines (dml.rs:1262-1315), of which 10 are in-body comments -> 44 code lines; it was 71 lines before this diff. All new indexer
- R13: ddb-core/src/indexer/mod.rs grows 788 -> 883 lines, crossing the 800-line limit as a direct result of this diff (about 40 of the added lines are doc comments). ddb-core/src/sql_engine/dml.rs is 1646 l

_engine: workflow — dimensions 5, raw 12, unique 12, confirmed 0, refuted 0, demoted 0, unverified 0, diff_bytes 13809_

Verdict: converged
Tests: {{TESTS_LINE}}

## Eve

lane_status: **ok**

Doubt + de-slop review of FT-2 (4a248cb..HEAD, staged). Verified locally: cargo build OK; cargo +1.89.0 clippy --workspace --all-targets -D warnings clean; hazard_delete_owner_junction_rows 1/1; --lib indexer 266/266; --lib sql_engine 376/376; --lib service 179/179; ddb-e2e cascade 8/8. Core fix is correct: the owner direction now runs on the service path and the shared sweep exists once. Residue: the new fatal/no-op typedef-load branches on the service path are unpinned, a missing-junction-table edge can abort the delete after remove_doogat already ran, and the new shared fn re-derives junction names and takes a redundant parameter with needless clones.

| Severity | Title | File | Proof |
| --- | --- | --- | --- |
| MEDIUM | New asymmetric typedef-load error handling on the service path has no test | ddb-core/src/indexer/mod.rs:528 | n/a (MEDIUM) |
| MEDIUM | Tier-1 gate (cargo test-ci) not run for this item | ddb-core/src/indexer/mod.rs:503 | n/a (VERIFY) |
| MEDIUM | Owner-direction DELETE can abort the service delete after the index row is already gone if the junction table is missing | ddb-core/src/service/delete.rs:107 | n/a (VERIFY) |
| LOW | Shared sweep re-derives junction names instead of using the single-source helpers | ddb-core/src/indexer/mod.rs:859 | (no proof recorded) |
| LOW | owner_schema parameter is redundant and forces clones in both callers | ddb-core/src/indexer/mod.rs:848 | (no proof recorded) |
| LOW | Restating doc comments and a paragraph-length CHANGELOG line | CHANGELOG.md:34 | (no proof recorded) |
| LOW | Interpolated junction identifiers are quoted but not escaped in the new shared fn | ddb-core/src/indexer/mod.rs:859 | (no proof recorded; KNOWN, PRD 00179) |
| LOW | Hazard test header and assertion messages still describe the hazard as live with stale line refs | ddb-core/tests/hazard_delete_owner_junction_rows.rs:1 | (no proof recorded; KNOWN, allowlist) |
| LOW | IndexPort trait doc still describes cascade_junction_cleanup as reverse-only | ddb-core/src/traits.rs:222 | (no proof recorded; KNOWN, allowlist) |

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

Found and reviewed the staged (uncommitted) diff for FT-2: CHANGELOG.md, ddb-core/src/indexer/mod.rs, ddb-core/src/sql_engine/dml.rs, ddb-core/tests/hazard_delete_owner_junction_rows.rs — exactly the allowlisted files, nothing extra. Verified the two-direction sweep was consolidated into one shared pub(crate) fn `delete_junction_rows_for_cascade` in indexer/mod.rs, that both cleanup entry points delegate to it (no third copy), that the asymmetric fatal/warn-and-skip error handling is preserved, that trait signatures are untouched, and that the CHANGELOG entry is correctly placed. Ran all 5 gate commands.

| Severity | Title | File | Proof |
| --- | --- | --- | --- |
| LOW | indexer/mod.rs now exceeds the repo's 800-line file-size cap | ddb-core/src/indexer/mod.rs:883 | N/A (code-quality/file-size finding, not a functional failure) |

Verdict lines:

```
B1: pass
B2: pass
B3: pass
B4: pass
B6: pass
B7: pass
B8: pass
B9: pass
B10: pass
B11: pass
B12: pass
B13: fail
```

## Bob (codex)

lane_status: **ok** — codex-run.sh completed (exit 0, gpt-5.6-sol); output at `/Users/bob/git/src/github.com/doogat/ddb/dev/local/tmp/FT-2-bob-output.txt`

| Severity | Title | File | Proof |
| --- | --- | --- | --- |
| HIGH | Fatal owner-schema loading occurs after index rows are deleted | ddb-core/src/service/delete.rs:99 | A malformed owner typedef returns an error before the Git commit but leaves SQLite missing the doogat while HEAD-based freshness checks still consider the index current. |
| HIGH | Schema-derived identifiers interpolated without escape_sql_ident | ddb-core/src/indexer/mod.rs:858 | A quote-bearing type/column name permits quote-based SQL corruption in the interpolated DELETE statements. |
| MEDIUM | Only the happy path is tested | ddb-core/tests/hazard_delete_owner_junction_rows.rs:45 | (no proof recorded) |
| MEDIUM | Junction names reconstructed with format! instead of existing helpers | ddb-core/src/indexer/mod.rs:858 | (no proof recorded) |
| MEDIUM | SQL path clones the owner schema only to borrow it | ddb-core/src/sql_engine/dml.rs:1307 | (no proof recorded) |
| MEDIUM | indexer/mod.rs grows to 883 lines, over the 800-line limit | ddb-core/src/indexer/mod.rs:477 | (no proof recorded) |
| LOW | Cannot statically verify build/clippy/test gates | N/A | (no proof recorded) |

Verdict lines:

```
R1: fail
R2: pass
R3: pass
R4: pass
R6: pass
R7: fail
R8: fail
R9: fail
R10: pass
R11: pass
R12: pass
R13: fail
```

## Carl (gemini)

lane_status: **failed** — gemini-run.sh (backend=copilot, model=gemini-3.8-flash) exited 0 after ~45s but produced no review: the output file contains only the tool-read trace ("Read FT-2-fast-track.diff / FT-2-card.md / FT-2-no-pack.md") and no issue lines and no R-rule verdict lines. The model emitted 158 output tokens and stopped before writing the review body.

| Severity | Title | File | Proof |
| --- | --- | --- | --- |
| — | (no findings produced) | — | — |

Verdict lines: none produced.

## Adversarial verification

Two blocking candidates entered verification; both were confirmed and neither was refuted.

| Candidate | Lane | Result | Reason |
| --- | --- | --- | --- |
| Fatal owner-schema loading occurs after index rows are deleted (ddb-core/src/service/delete.rs) | bob | confirmed | Independently corroborated by the fan-out advisory items ("Owner-side schema fallback makes a previously-successful delete fail after the index rows are already gone") and by Eve's VERIFY item on the same call ordering: `remove_doogat` at delete.rs:99 and the typed-table DELETE precede the `?`-propagating cleanup at delete.rs:107, with `commit_batch` never reached. Index/git divergence is reachable from a malformed typedef. |
| Schema-derived identifiers interpolated without escape_sql_ident (ddb-core/src/indexer/mod.rs) | bob | confirmed | Corroborated by the fan-out security dimension finding at mod.rs:862 and by Eve's KNOWN item at mod.rs:859: the interpolated `{jt}` / `{col_id}` / `{parent_id_col}` identifiers are quoted but never routed through `escape_sql_ident` (indexer/filter.rs), against the repo's explicit identifier-escaping invariant. |

No candidate was refuted this cycle (refuted count: 0).

## Rework

One rework cycle ran and closed both blocking findings.

| Closure | Lane of origin | Status |
| --- | --- | --- |
| Fatal owner-schema loading occurs after index rows are deleted (ddb-core/src/service/delete.rs) | bob (HIGH) | closed by rework |
| Schema-derived identifiers interpolated without escape_sql_ident (ddb-core/src/indexer/mod.rs) | bob (HIGH) | closed by rework |

Remaining blocking after rework: none.

## Advisory

All 27 non-blocking findings, grouped by lane of origin.

### Eve

| Severity | Title | File:Line | Fix |
| --- | --- | --- | --- |
| MEDIUM | New asymmetric typedef-load error handling on the service path has no test | ddb-core/src/indexer/mod.rs:528 | Add two indexer unit tests beside materialize_tests.rs:733: own typedef unreadable -> Err; unrelated typedef unreadable -> Ok with the owner-side junction row gone. Requires extending the allowlist to that test file. |
| MEDIUM | Tier-1 gate (cargo test-ci) not run for this item | ddb-core/src/indexer/mod.rs:503 | Run `cargo test-ci` in the foreground (600000 ms) and confirm 0 failed. |
| MEDIUM | Owner-direction DELETE can abort the service delete after the index row is already gone if the junction table is missing | ddb-core/src/service/delete.rs:107 | Reproduce with a typedef that gains a `references:` column without rematerialize, then delete; if it errors, guard the junction DELETEs with a sqlite_master existence check as rebuild.rs:360-369 does. |
| LOW | Shared sweep re-derives junction names instead of using the single-source helpers | ddb-core/src/indexer/mod.rs:859 | Replace the four `format!` calls with `junction_table_name` / `junction_ref_id_column` / `junction_parent_id_column` (materialize.rs:77-89). |
| LOW | owner_schema parameter is redundant and forces clones in both callers | ddb-core/src/indexer/mod.rs:848 | Drop the parameter; have the Index caller insert the retried schema into the owned map and let the shared fn do `schemas.get(target_type)`. |
| LOW | Restating doc comments and a paragraph-length CHANGELOG line | CHANGELOG.md:34 | Trim to one user-visible sentence; keep the direction/asymmetry prose once, on the shared fn. |
| LOW | Interpolated junction identifiers are quoted but not escaped in the new shared fn | ddb-core/src/indexer/mod.rs:859 | Out of scope here; leave to PRD 00179 (sql-identifier-escaping), which should include this fn in its sweep. |
| LOW | Hazard test header and assertion messages still describe the hazard as live with stale line refs | ddb-core/tests/hazard_delete_owner_junction_rows.rs:1 | Out of scope by allowlist; follow-up rewrite of the header as a regression pin, dropping line numbers. |
| LOW | IndexPort trait doc still describes cascade_junction_cleanup as reverse-only | ddb-core/src/traits.rs:222 | Out of scope by allowlist; one-line doc follow-up covering both directions. |

### Blake

| Severity | Title | File:Line | Fix |
| --- | --- | --- | --- |
| LOW | indexer/mod.rs now exceeds the repo's 800-line file-size cap | ddb-core/src/indexer/mod.rs:883 | Move `delete_junction_rows_for_cascade` + `load_single_typedef_schema` into a small `indexer/cascade.rs` submodule. |

### Bob (codex)

| Severity | Title | File:Line | Fix |
| --- | --- | --- | --- |
| MEDIUM | Only the happy path is tested | ddb-core/tests/hazard_delete_owner_junction_rows.rs:45 | Add regression coverage for fatal-owner and warn-and-skip-unrelated typedef behavior, including absence of partial deletion. |
| MEDIUM | Junction names reconstructed with format! instead of existing helpers | ddb-core/src/indexer/mod.rs:858 | Use the `junction_table_name` / `junction_parent_id_column` / `junction_ref_id_column` single-source helpers. |
| MEDIUM | SQL path clones the owner schema only to borrow it | ddb-core/src/sql_engine/dml.rs:1307 | Pass `schemas.get(target_type)` directly. |
| MEDIUM | indexer/mod.rs grows to 883 lines, over the 800-line limit | ddb-core/src/indexer/mod.rs:477 | Move the new cleanup logic into a focused indexer submodule. |
| LOW | Cannot statically verify build/clippy/test gates | N/A | Covered by the Gate section below (all five commands exit 0). |

### Fan-out

| Severity | Title | File:Line | Fix |
| --- | --- | --- | --- |
| MEDIUM | Service delete gains an unrequested fatal path when the deleted doogat's own typedef fails to load, aborting after the index row is already gone | ddb-core/src/indexer/mod.rs:512 | Scope the fatal semantics to the SQL path, or wrap the cleanup call in `execute_delete_plan` in a savepoint so an Err rolls back `remove_doogat`. |
| MEDIUM | Owner-side schema fallback makes a previously-successful delete fail after the index rows are already gone | ddb-core/src/indexer/mod.rs:511 | Resolve the owner schema before any index mutation, or wrap the per-id block in `Index::with_savepoint`; add tests for both fallback branches. |
| MEDIUM | New single-caller schema loader duplicates load_all_typedefs and turns a warn-skip into a mid-delete hard failure | ddb-core/src/indexer/mod.rs:510 | Delete `load_single_typedef_schema` and pass the batch-loaded schema straight through, keeping failure behavior unchanged. |
| MEDIUM | New load_single_typedef_schema fallback (both branches) has no test | ddb-core/src/indexer/mod.rs:528 | Add two indexer unit tests using the MockSource recipe at materialize_tests.rs:733-777 (fatal own-typedef; Ok(None) no-op). |
| MEDIUM | Shared junction-cleanup helper interpolates typedef-derived SQL identifiers without escape_sql_ident | ddb-core/src/indexer/mod.rs:862 | Wrap the interpolated identifiers with `escape_sql_ident`; values already use bound params. Track under 00179. |
| LOW | Hazard test module doc still describes the bug as present after the pin was activated | ddb-core/tests/hazard_delete_owner_junction_rows.rs:4 | Update the module doc to state the owner direction now runs through the shared sweep and that this file is the regression pin. |
| LOW | IndexPort trait doc still describes junction cleanup as target-side only | ddb-core/src/traits.rs:220 | State both directions in the trait doc, matching the impl doc at indexer/mod.rs:477-501. |
| LOW | Needless full-schema clone where an immutable borrow works | ddb-core/src/sql_engine/dml.rs:1307 | Pass `schemas.get(target_type)` directly as the `Option<&TableSchema>` argument. |
| LOW | Shared junction sweep lands in an already-oversized indexer/mod.rs instead of the module owning junction logic | ddb-core/src/indexer/mod.rs:848 | Move the free function to indexer/materialize.rs (or a new indexer/junctions.rs) beside the sibling junction helpers. |
| LOW | GraphQL delete docs still describe one-direction junction cleanup | docs/src/technical/server.md:303 | Reword to cover both directions, matching docs/src/technical/sql-engine.md:422-424. |
| LOW | Only a CI-only e2e pins the reverse direction of the refactored service cleanup | ddb-core/src/indexer/mod.rs:507 | Extend the hazard test (or add an indexer unit test) so the reverse leg is covered by a Tier-1 command. |
| LOW | Hazard test's module doc still asserts the hazard is live and unpinned | ddb-core/tests/hazard_delete_owner_junction_rows.rs:4 | Rewrite the module doc as a fixed-and-pinned statement; drop stale line references from assert messages. |

## Assumptions

Recorded by the implementors:

1. Moved the shared two-direction sweep into `indexer/mod.rs` as a `pub(crate)` free function (`delete_junction_rows_for_cascade`) called by both `Index::cascade_junction_cleanup` and `sql_engine::dml::SqlEngine::cascade_junction_cleanup`, rather than adding a new `SqlBackend`/`IndexPort` trait method. This avoids any trait-signature change (the card said "prefer keeping it unchanged") and avoids regressing the SQL DELETE path's transaction-aware, read-your-writes-safe schema loading via `SqlEngine::load_schema` (verified: `read_your_writes_within_txn` still passes).
2. For the service path's owner-direction schema resolution, when the deleted doogat's own type has no `_typedef` row at all (as opposed to a load failure), treated that as a silent no-op rather than fatal. This matches the SQL DELETE path's actual behavior: its outer loop only iterates over type_names that already exist as `_typedef` rows, so its fatal branch only fires on a genuine read/parse failure of an existing typedef, never on a missing one.
3. `docs/src/technical/indexer.md` does not state the cleanup is reverse-only anywhere (verified via search), so no doc correction was needed per the card's conditional instruction.
4. Moving `cascade_junction_cleanup` earlier in the loop (before `remove_doogat` and the typed-table DELETE) is safe because that function only reads typedef schemas from Git and writes junction-table rows keyed by the deleted id; it does not depend on the doogat's own doogats-table row or typed-table row still existing.
5. Escaping `table_name` / `col.name` / `target_type` individually before concatenating them into the compound junction-table identifier (e.g. `{table}_{col}`) is equivalent to escaping the final concatenated identifier, since `_` is not a quote character and `escape_sql_ident` is a simple quote-doubling replace.

Files touched: `ddb-core/src/indexer/mod.rs`, `ddb-core/src/sql_engine/dml.rs`, `ddb-core/tests/hazard_delete_owner_junction_rows.rs`, `CHANGELOG.md`.

## Gate

| Command | Exit code | Seconds |
| --- | --- | --- |
| `cargo build` | 0 | 29 |
| `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings` | 0 | 118 |
| `cargo test -p ddb-core --test hazard_delete_owner_junction_rows` | 0 | 9 |
| `cargo test -p ddb-core --lib sql_engine` | 0 | 596 |
| `cargo test -p ddb-core --lib indexer` | 0 | 3 |

All five gate commands exit 0. No cargo command was run by this review pass; the numbers above are as reported by the implementor and gate runs.

## Driver exit notes

Driver (Opus 5, S2) read the final staged diff (309 lines, 5 files) and re-ran the gate independently before committing:

- `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings` -> clean (0.57 s; a cache hit on the gate agent's real 118 s run, so it proves nothing was invalidated, not that clippy ran cold).
- `cargo test -p ddb-core --test hazard_delete_owner_junction_rows` -> 1 passed, 0 failed, 0 ignored.
- `cargo test -p ddb-core --lib indexer` -> 266 passed, 0 failed.
- The gate agent's `--lib sql_engine` figure (596 s, exit 0) IS a real measurement, not a cache hit, and it sits 4 s under the 600 s subagent Bash cap. A future card must not put `--lib sql_engine` in a subagent gate without expecting a timeout.

Diff verdict: matches the card. The shared `delete_junction_rows_for_cascade` free function is the single two-direction sweep; both callers keep their own schema-loading strategy, which is the right seam (the SQL path's `load_schema` is transaction-aware and read-your-writes safe). The rework's reordering in `service/delete.rs` (cleanup before `remove_doogat`) is correct and closes the index/git divergence bob raised.

Residuals the driver carries forward (none blocking; not fixed here, surgical-changes rule):

1. **`ddb-core/src/indexer/mod.rs` is now 883 lines, over the repo's 800-line cap.** Raised independently by Blake (LOW) and Bob (MEDIUM). Fix: move `delete_junction_rows_for_cascade` + `load_single_typedef_schema` into an `indexer/cascade.rs` (or beside the junction helpers in `materialize.rs`). Deferred to a card.
2. **The new sweep re-derives junction identifiers with `format!` instead of the single-source helpers** `junction_table_name` / `junction_ref_id_column` / `junction_parent_id_column` (`indexer/materialize.rs:77-89`). Three lanes agree. Behavior-identical today; a drift risk if the naming scheme ever changes. Same card as (1).
3. **The two `load_single_typedef_schema` branches have no test** (own typedef unreadable -> Err; no typedef row -> Ok(None) no-op). Three lanes agree; the MockSource recipe at `materialize_tests.rs:733-777` is the template. Same card as (1).
4. **Stale docs the allowlist forbade touching**: `ddb-core/src/traits.rs:220` still calls the cleanup target-side only, and `docs/src/technical/server.md:303` still describes one-direction junction cleanup. Both belong in the S6 `docs-corrections` item.
5. **The hazard test's module doc still describes the hazard as live** with line references that have moved. It is now a regression pin and should say so. Same card as (1) or the S6 docs item.
6. **Pre-existing, untouched**: `ddb-core/src/service/delete.rs:104` still interpolates `dtype` into `DELETE FROM "{}"` without `escape_sql_ident`. Outside this item's diff; belongs to PRD 00179 (sql-identifier-escaping).

Lane note (D6): carl failed differently from S1. The copilot/gemini backend exited 0 after ~45 s having read all three input files and emitted 158 output tokens, then stopped without writing a review body - not the 540 s timeout seen in S1. Recorded as one lane down; the batch continues per D6.
