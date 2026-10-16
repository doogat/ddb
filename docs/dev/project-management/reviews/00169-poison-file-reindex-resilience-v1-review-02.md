---
prd: dev/local/prds/wip/00169-poison-file-reindex-resilience-v1.md
review: 2
date: 2026-08-16
head_sha: 28dda73c0fac5842fb2bfab7daa4a8bbbb50873e
codex_thread_id: 01a00b4c-fe5f-7b21-bac6-e2062c18d982
reviewers: alice,blake,bob,carl,eve
agents:
  alice: available
  blake: available
  bob: available
  carl: available
  eve: available
---

# Review: 00169-poison-file-reindex-resilience-v1

Diff range: `f00c4839c1a10391f1355c5a01bb3d5f3c376c14..28dda73c0fac5842fb2bfab7daa4a8bbbb50873e`
(incremental — scoped to the cycle-1 rework commits only, 16 files)

codex_rung_guard: fired (4 codex-implemented task(s))

Consensus engine: `legacy` (single Alice subagent). Doubt reviewer resolved to `codex`,
then forced to `fable` in-memory by the codex doubt-roster guard, because four tasks were
implemented by codex — so Eve joined as a fifth lens alongside Bob rather than replacing
him. `state.doubt_reviewer` was deliberately not rewritten.

## Review Summary

Reviewed: 20 completed tasks (6 reworked in cycle 1, tasks 15-20).

All five reviewers independently confirmed **all six cycle-1 rework tasks are CLOSED**:

| Task | Subject | Alice | Carl | Eve |
|------|---------|-------|------|-----|
| 15 | Evict stale rows for a poison-modified file | CLOSED | Fixed | CLOSED |
| 16 | `summarize_reindex_warnings` mislabel/miscount/`ddb doctor` pointer | CLOSED | Fixed | CLOSED |
| 17 | Lock-scope regression test + direct index assertion | CLOSED | Fixed | CLOSED |
| 18 | Triplicated lock acquisition + duplicated parse logic | CLOSED | Fixed | CLOSED |
| 19 | `ddb reindex` silent on paths, no CLI `AppWarning` test | CLOSED | Fixed | CLOSED |
| 20 | Doc/CHANGELOG gaps | CLOSED | Fixed | CLOSED |

Task 18's specific regression risk (narrowing the rebuild-lock guard lifetime during the
`acquire_rebuild_lock` extraction) was checked explicitly by both Alice and Eve: `_guard`
is still bound at the same function scope at all three call sites and still spans drop +
DDL + stamp. `indexer/mod.rs` is 788 lines, under the 800 cap.

### Agent Status

| Agent | Lens | Status | Notes |
|-------|------|--------|-------|
| Alice | consensus (implementation-aware) | ran | 2 findings (1 medium, 1 low) |
| Blake | blind (PRD-only) | ran | 1 high, auto-dismissed against the ledger |
| Bob | doubt + de-slop (codex) | ran | 7 findings (1 high, 5 medium, 1 low); D1-D5 all pass |
| Carl | UI/design + generalist (gemini via copilot) | ran | 2 findings (both medium) |
| Eve | doubt (Fable 5) | ran | 2 FIX (low), 1 VERIFY, 4 KNOWN; D1-D5 all pass |

### Session note (fail-loud)

The cycle-2 review was dispatched in a prior headless session that died after every
reviewer had written its output but before consolidation ran. This session resumed at
step 6 against the five completed output files in `dev/local/tmp/*-00169-c2.txt`
(timestamps 21:28–21:43); no reviewer was re-run and none was skipped.

## Consolidated Findings

11 findings after consolidation. Blake's HIGH was auto-dismissed against the settled
ledger (see below), so it is not counted here.

| Consensus | Severity | Issue | File | Task | Found By |
|-----------|----------|-------|------|------|----------|
| [1/5] | 🟠 | Poison eviction derives the ID from the filename, but supported frontmatter IDs can differ; the stale row then survives, while a newly added poison filename can evict an unrelated row with that ID | ddb-core/src/indexer/rebuild.rs:107 | 15 | BOB |
| [1/5] | 🟡 | `incremental_reindex` is 90 lines (53-142), over the 50-line guideline; task 15's 13-line poison-id filter (lines 107-117) could be extracted into a named helper the same way task 18 extracted `evict_doogats`/`parse_changed_path` from this file | ddb-core/src/indexer/rebuild.rs | 15 | ALICE |
| [1/5] | 🟡 | Poison eviction can report success and advance HEAD despite incomplete materialized cleanup because type lookup errors are converted to `None` and `unmaterialize_data_doogats` ignores query/delete errors | ddb-core/src/indexer/rebuild.rs:148 | 15 | BOB |
| [1/5] | 🟡 | The new schema-version drop loop duplicates `drop_all_tables`, silently discards row-decoding failures, and interpolates unescaped identifiers from `sqlite_master` | ddb-core/src/indexer/schema_version.rs:44 | 18 | BOB |
| [1/5] | 🟡 | Skip-path deduplication uses quadratic `Vec::contains` despite explicitly supporting large corrupt corpora; a `HashSet` preserves the first description while simplifying membership checks | ddb-core/src/app_contract/output.rs:41 | 16 | BOB |
| [1/5] | 🟡 | The new task-15 regression tests are roughly 150–180 lines each, exceeding the 50-line limit and repeating fixture construction and survival assertions | ddb-core/src/indexer/tests/mod.rs:6247 | 15 | BOB |
| [1/5] | 🟡 | Strict incremental reindex still applies genuine deletions before discovering a poison file and returning `Err` | ddb-core/src/indexer/rebuild.rs:97 | 15 | BOB |
| [1/5] | 🟡 | Fold evict_doogats calls after batch_index_changes to prevent partial state on strict abort | ddb-core/src/indexer/rebuild.rs | 15 | CARL |
| [1/5] | 🟡 | Extract table-drop loop to a free function shared by Index::drop_all_tables and schema_version | ddb-core/src/indexer/mod.rs | general | CARL |
| [1/5] | ⚪ | `ddb reindex`'s new per-line warning print (lines 65-70) iterates raw undeduplicated `report.warnings`, so one malformed file shows as two near-identical lines | ddb-cli/src/commands/maintenance.rs | 19 | ALICE |
| [1/5] | ⚪ | The newly public `describe_consistency_warning` API has no rustdoc describing its stable transport-facing purpose | ddb-core/src/app_contract/mod.rs:21 | 19 | BOB |

### Auto-dismissed (ledger)

- [BLAKE] 🟠 "PRD Phase 1 acceptance bullet 'after a poison file lands, a create AND a
  search both succeed and each carries a skip AppWarning' is unmet: `search` /
  `search_paginated` / `search_paginated_filtered` call `self.ensure_fresh()?;` and
  discard the returned warnings; the CLI `search` command and the GraphQL `search` query
  never call `write_warnings`/`forward_warnings`."
  | File: ddb-core/src/service/search.rs

  **Dismissed** against the cycle-1 ledger entry that settled the identical finding:
  confirmed real and deliberately narrowed at design time, owned by backlog PRD 00176
  (route-search-through-contract), and — as Eve proved in cycle 1 — unimplementable as
  literally written, because the warning is consume-once (the first verb's `ensure_fresh`
  reindexes and leaves the index fresh, so a create AND a subsequent search cannot both
  carry it for one poison event). The overstated `invariants.md` I5 wording was corrected
  in this cycle's task 20. Blake is blind by construction and cannot see the ledger; this
  is the mechanical `--ledger-dismiss BLAKE` filter working as designed, not a
  disagreement.

## Orchestrator verification of the one HIGH

Bob's HIGH (filename-derived eviction identity) was verified directly against the code
rather than accepted or deflated on reviewer authority:

- The new poison-eviction filter calls `crate::parser::extract_id_from_path`
  (`ddb-core/src/indexer/rebuild.rs:112`).
- That function is a bare `Path::file_stem` — `ddb-core/src/parser/mod.rs:972-977`.
- The **pre-existing delete leg** derives its IDs through the *same call*:
  `ddb-core/src/indexer/rebuild.rs:190`.

**Verdict: real in shape, pre-existing in class.** The new code inherits the repo's
long-standing filename-is-identity model rather than introducing a new one. Eve
independently reached the same conclusion and placed it in KNOWN, noting a full rebuild
self-heals it. Reaching the defect needs a hand-authored `ddb/{stem}.md` whose frontmatter
id differs from its stem; every ddb write path names the file after the id. Consequences
are bounded — the index is a derived cache with git as source of truth.

Deferred rather than reworked because the honest fix repairs **both** legs together
(capture each changed path's indexed `(id, type)` before parsing, evict only that captured
row). Patching the poison leg alone would leave the two legs disagreeing about identity,
which is worse than the current consistent-but-wrong model.

## Alice

Verified all six rework tasks CLOSED with file/line evidence (see the Review Summary
table). Two new observations from this cycle's diff, both minor and both introduced or
newly grown by the rework itself:

- 🟡 `incremental_reindex` (rebuild.rs:53-142, 90 lines) grew further past the 50-line
  guideline when task 15 inlined the 13-line poison-id filter. Task 18 already extracted
  `evict_doogats`/`parse_changed_path`/`acquire_rebuild_lock` from this same file for the
  identical reason; `let poisoned = Self::poisoned_ids(&warnings);` would bring it back
  under the limit with no behavior change.
- ⚪ `ddb reindex`'s new per-warning print loop (maintenance.rs:65-70) iterates the raw,
  undeduplicated `report.warnings`. `docs/src/technical/indexer.md:108-114` already
  documents that one malformed file produces two warnings (`parallel_parse` +
  `collect_consistency_warnings`), so a single poison file now prints as two
  near-identical visible lines. Root cause is pre-existing and already disclosed in docs,
  so this is cosmetic, not a correctness regression.

R1-R11, R13: pass. R12: fail.

## Blake

Blind lens (PRD-only, no diff, no design doc, no review history — as designed).

Summary: "a well-built, thoroughly tested implementation of most of PRD 00169 — the
lenient collect-and-skip incremental path, `--strict` opt-in, the cross-process rebuild
lock with double-checked staleness re-verification, the schema-version stamp, and the
frontmatter size cap are all correctly implemented and unit-tested, and the cold-start
concurrency e2e (with the warm-up workaround removed) is present."

One finding (🟠, the search-verb AppWarning gap) — auto-dismissed against the ledger, see
above. Blake explicitly recorded that the gap is honestly self-documented in
`invariants.md` and `CHANGELOG.md` rather than hidden.

✅ No other issues found.

B1: fail, B2: pass, B3: fail, B4-B14: pass, B15: fail, B16-B19: pass.

## Bob

Doubt lens + de-slop (codex, static-only sandbox), resumed on thread
`01a00b4c-fe5f-7b21-bac6-e2062c18d982`.

Seven findings: 1 high (the eviction-identity finding verified above), 5 medium, 1 low —
all listed in the consolidated table. Bob filed the strict-deletion-ordering item under
his own KNOWN section with the reasoning that no production caller enables incremental
strict mode and that a safe fix must preserve rename-shaped delete/add ordering.

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Carl

Confirmed all six rework tasks fixed, with the same task-18 guard-lifetime check Alice and
Eve ran. Two medium simplification findings, both duplicates of Bob's from the other
file's side:

- 🟡 Fold `evict_doogats` calls after `batch_index_changes` (same as Bob's
  strict-deletion-ordering item — refuted as unsafe by Eve, see below).
- 🟡 Extract the table-drop loop to a free function shared by `Index::drop_all_tables` and
  `schema_version` (same as Bob's duplication item).

R1-R13: pass.

## Eve

Doubt lens (Fable 5), fifth voice added by the codex doubt-roster guard. Verified all six
rework tasks CLOSED with independent line-level evidence, including two checks neither
Alice nor Carl ran: that the task-17 waiter test is genuinely discriminating (peer stamps
+ sentinel; reverting the re-check kills the sentinel, and the anti-vacuity `assert_parked`
runs before *and* after the peer's upgrade), and that `schema_version.rs` is a faithful
line-for-line move of the removed code.

### Eve — FIX

- ⚪ CHANGELOG bullet for the `ddb reindex` per-warning print sits under "### Changed";
  the changelog rule maps `feat` → Added — CHANGELOG.md:21.
- ⚪ Newly-public `describe_consistency_warning` ships with no rustdoc despite being
  advertised in the CHANGELOG as the one core warning-rendering definition for transports
  — ddb-core/src/app_contract/mod.rs:80. (Same defect Bob raised.)

### Eve — VERIFY

- `tests/smoke.ps1` section 33 (including the `$reindexQuietExit = $LASTEXITCODE` guard
  through the `| Out-String` pipeline) was reviewed but never executed on this host — no
  `pwsh` resolves here. Run the Windows smoke job in CI against pushed `28dda73` and
  confirm both new section-33 `pass` lines appear.

### Eve — KNOWN

- **Strict reindex ordering — the proposed fix is UNSAFE.** Folding both evictions after
  `batch_index_changes` (Carl's and Bob's suggestion) would delete a just-indexed row when
  a deleted `ddb/_typedef/X.md` and an added `ddb/X.md` share a file stem. The applied
  deletion mirrors git truth, stored HEAD stays put, and the next lenient pass converges.
  Documented in the CHANGELOG parenthetical.
- Poison-file eviction keys off the bare file stem — pre-existing class shared with the
  delete leg, full rebuild self-heals. (This is the HIGH verified above.)
- Junction-table rows referencing an evicted poisoned doogat are not cleaned:
  `unmaterialize_data_doogats` defers to service-layer `cascade_junction_cleanup`, which
  no incremental path calls. Identical residue shape to the long-standing sync-delete leg;
  orphans are inert for inner joins and the next full rebuild clears them.
- `Index::drop_all_tables` remains a near-copy of
  `schema_version::drop_tables_if_still_outdated`'s drop loop — judged right to leave: the
  copies differ in FK-pragma placement and transactional context (PRD 00157 comment), so
  unifying risks the destructive path for no behavioral gain.

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

Eve's own closing line: "No Critical or High findings; nothing here stalls the PRD."

## Decision Gate (Phase 5, cycle 2 of cap 2)

**Cap reached.** `state.cycle` (2) >= `state.rework_cap` (2), and one unresolved HIGH
remains (Bob's eviction-identity finding), so this cycle does **not** meet the strict
convergence test. Zero CRITICAL findings were raised by any reviewer, so the loop-mode
cap-out branch applies: all 11 unresolved findings are deferred to batch end as
`cap-overflow` and the PRD finalizes as **converged-with-deferrals**. No third rework
cycle, and no tail sweep (the sweep runs only on the clean-convergence path).

Every deferral carries a verified reason in
`dev/local/reviews/00169-poison-file-reindex-resilience-v1-ledger.json` (11 new cycle-2
entries, 23 total) and is surfaced for the batch-end walkthrough as 3 records in
`dev/local/autopilot/deferred/202607161128-deferred.json`.

Follow-ups worth their own PRD, in priority order:

1. Make both eviction legs key off the captured indexed `(id, type)` rather than the file
   stem, and stop `evict_doogats` swallowing type-lookup and unmaterialize errors
   (findings 1 and 3 — one coherent change).
2. Add rustdoc to the newly-public `describe_consistency_warning` (finding 11 — one line).

## Follow-up Tasks Created

None. The rework cap was reached, so no `[C2]`/`[D2]` tasks were queued — creating tasks
that `/work` would never run would misrepresent the state. The findings live in the
deferred JSON and the ledger instead.

Verdict: 11 findings
Tests: 1813 passed, 0 failed, 0 skipped
