---
prd: dev/local/prds/wip/00169-poison-file-reindex-resilience-v1.md
review: 1
date: 2026-08-16
head_sha: f00c4839c1a10391f1355c5a01bb3d5f3c376c14
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

Diff range: `6bb16174fd533b94db0f45016019aa4626b5a16e..f00c4839c1a10391f1355c5a01bb3d5f3c376c14`
(41 files, +3484/-161, 4534 diff lines)

codex_rung_guard: fired (2 codex-implemented task(s))

Consensus engine: `legacy` (single Alice subagent). Doubt reviewer resolved to `codex`,
then forced to `fable` in-memory by the codex doubt-roster guard, because tasks 5 and 11
were implemented by codex — so Eve joined as a fifth lens alongside Bob rather than
replacing him. `state.doubt_reviewer` was deliberately not rewritten.

## Review Summary

Reviewed: 14 completed tasks
PRDs checked: 00169-poison-file-reindex-resilience-v1

### Agent Status

- Alice (Claude, consensus): ✅ Available
- Blake (Claude, blind/PRD-only): ✅ Available
- Bob (codex, doubt + de-slop): ✅ Available
- Carl (gemini/copilot, generalist here — no frontend surface): ✅ Available
- Eve (Fable 5, doubt lens): ✅ Available

All five emitted complete, well-formed verdict lines on the first dispatch. No format
retries and no fallbacks were needed.

### Note on consolidation arithmetic

Consensus denominators are `/5`, but **Eve contributes no rows to the table by design**:
her contract is the FIX/VERIFY/KNOWN bucket format, not the `[EVE] emoji | File: | Task:`
issue-line format that `consolidate_findings.py` parses. Her findings are carried into the
decision gate directly from her buckets (reproduced in full below), so a `[1/5]` row that
Eve also raised in her buckets has real agreement of 2 or more. Where that happens it is
stated explicitly in the gate decisions.

## Consolidated Findings

| Consensus | Severity | Issue | File | Task | Found By |
|-----------|----------|-------|------|------|----------|
| [2/5] | 🟡 | `indexer/mod.rs` is now 837 lines and `configure_connection` exceeds 50 lines; extract schema-version detection and locked upgrade setup into a focused module/helper | ddb-core/src/indexer/mod.rs:164 | 8 | BOB, CARL |
| [1/5] | 🔴 | Phase 1's acceptance criterion "a create AND a search both succeed and each carries a skip AppWarning ... reaches CLI stderr and the GraphQL warnings extension" is unmet for search (and read/sql/discovery/batch/delete/utility): every verb except create/update/apply_schema calls `self.ensure_fresh()?;` bare and discards the warnings. `SearchCommand`/`AppSearchResult` exist but are never constructed | ddb-core/src/service/search.rs | general | BLAKE |
| [1/5] | 🔴 | R1/R2: No regression test for the CRITICAL rebuild-lock guard scope bug. The "indistinguishable" gap assumption is false: the loser's drop deletes tables the winner is already using, causing observable data loss or `no such table` panics | N/A | 8 | CARL |
| [1/5] | 🟠 | The CRITICAL lock-scope fix (0537b18) shipped with no regression test, and the accepted-gap justification rules out a *deterministic* test but not a *probabilistic* one; the fix itself is correct (verified by trace), so this is a testing-policy gap, not a live defect | ddb-core/src/indexer/mod.rs:161-243 | 8 | ALICE |
| [1/5] | 🟠 | A modified doogat that becomes malformed is skipped without removing its previously indexed base/materialized rows, leaving stale content queryable after HEAD is marked current | ddb-core/src/indexer/rebuild.rs:120 | 2 | BOB |
| [1/5] | 🟠 | Search discards `ensure_fresh()` warnings and returns no `AppOutput`, so the required CLI/GraphQL read-side warning path and mutation-plus-query conformance scenario are absent | ddb-core/src/service/search.rs:15 | 5 | BOB |
| [1/5] | 🟠 | ReadPool bypasses freshness and rebuild-lock coordination, so reads can still observe tables while another connection has dropped them | ddb-server/src/read_pool.rs:241 | 7 | BOB |
| [1/5] | 🟠 | The 256 KiB check runs only after the entire blob is decoded, split into a line vector, and frontmatter is copied; adversarial input can still consume unbounded read/scan memory | ddb-core/src/parser/mod.rs:126 | 10 | BOB |
| [1/5] | 🟡 | `docs/src/technical/indexer.md`'s `incremental_reindex` section is stale: signature missing `strict: bool`, and the section never describes the collect-and-skip/strict behavior — the PRD's headline feature | docs/src/technical/indexer.md:118 | 11 | ALICE |
| [1/5] | 🟡 | PRD metric "reindex cost stays bounded" only partially met: `MAX_FRONTMATTER_BYTES` bounds parse cost but `split_zones` reads the whole file first | ddb-core/src/parser/mod.rs:210-219 | 10 | ALICE |
| [1/5] | 🟡 | `indexer/mod.rs` crossed the 800-line cap (749 → 837) and `configure_connection` tripled (26 → ~80 lines); `batch_index_changes` (~82) and `incremental_reindex` (~92) are also over 50 | ddb-core/src/indexer/mod.rs | 8 | ALICE |
| [1/5] | 🟡 | The rebuild-lock acquisition block is duplicated verbatim three times (`configure_connection`, `locked_rebuild`, `locked_explicit_rebuild`) | ddb-core/src/indexer/rebuild.rs | 7 | ALICE |
| [1/5] | 🟡 | `invariants.md` I5 describes the skip-warning as reaching "the verb envelope" generally, without qualifying that only 3 of ~10 `ensure_fresh`-gated verb families wire it | docs/src/technical/invariants.md | general | BLAKE |
| [1/5] | 🟡 | The schema-upgrade lock-scope fix has no discriminating regression test | ddb-core/src/indexer/mod.rs:190 | 8 | BOB |
| [1/5] | 🟡 | The cold-start E2E compares reported IDs only with Git HEAD and never directly verifies the final SQLite row count; a CLI read-back would self-heal, direct DB inspection would not | tests/e2e/concurrent_writes.rs:150 | 14 | BOB |
| [1/5] | 🟡 | `batch_index_changes` duplicates the read/parse/strict/warning state machine across batch and singleton branches and exceeds 50 lines | ddb-core/src/indexer/rebuild.rs:180 | 2 | BOB |
| [1/5] | 🟡 | The new resilience tests add ~1,680 repetitive lines to an already monolithic 6,034-line module | ddb-core/src/indexer/tests/mod.rs:4354 | general | BOB |
| [1/5] | 🟡 | R12: Function `batch_index_changes` exceeds the 50-line limit (~80 lines) | ddb-core/src/indexer/rebuild.rs | 2 | CARL |
| [1/5] | 🟡 | Remove duplicated read/parse logic by using the batch loop for single files too | ddb-core/src/indexer/rebuild.rs | 2 | CARL |
| [1/5] | 🟡 | Extract duplicate rebuild lock acquisition into a `with_rebuild_lock` helper | ddb-core/src/indexer/rebuild.rs | 7 | CARL |
| [1/5] | ⚪ | `ConsistencyWarning.error` and FFI `RebuildWarningRecord.message` carry unbounded strings from untrusted synced content; the count is capped, the size is not | ddb-core/src/ffi/driver.rs | 6 | ALICE |
| [1/5] | ⚪ | Phase 1 text specifies `From<ConsistencyWarning> for AppWarning`; the implementation uses free summarizing functions instead | ddb-core/src/app_contract/output.rs | general | BLAKE |
| [1/5] | ⚪ | `indexer.md` never mentions the schema-version stamp or the 256 KiB frontmatter cap, both core Phase 1 features | docs/src/technical/indexer.md | general | BLAKE |
| [1/5] | ⚪ | The indexer documentation still shows the obsolete two-argument `incremental_reindex` signature | docs/src/technical/indexer.md:118 | 11 | BOB |
| [1/5] | ⚪ | Cannot statically verify: Tier 1 gates, PowerShell scenarios, or the E2E flake attribution | N/A | general | BOB |
| [1/5] | ⚪ | Pre-existing `! $DDB ...` smoke assertions retain the errexit false-pass hazard, outside this diff's surgical scope | tests/smoke.sh:54 | general | BOB |

## Orchestrator verification of contested claims

Three claims were checked directly against source before the gate acted on them, because
each would have driven rework on its own.

**1. `indexer/mod.rs` is over the 800-line cap — CONFIRMED.** `wc -l` reports 837.
`rebuild.rs` is 617 (under). `indexer/tests/mod.rs` is 6034, but was already 4354 at the
base commit, so this diff widened a pre-existing violation rather than creating one.

**2. Bob's stale-rows finding — CONFIRMED, and it is the most consequential finding in
this review.** `partition_changes` (`rebuild.rs:158-172`) routes both `DiffKind::Added`
and `DiffKind::Modified` into `to_index_paths`; only `DiffKind::Deleted` reaches
`remove_doogat` (`:116`). So when a *modified* doogat fails to read or parse,
`batch_index_changes` skips it (`continue`, `:202`/`:215`), its previously-indexed rows in
`doogats` and in the materialized type tables are never evicted, and `store_head(&new_head)`
(`:137`) then marks the index current. The result is stale pre-edit content served as
current, with the index claiming freshness. This behavior is *introduced* by the leniency
work: before this PRD the whole batch aborted with `?`, so HEAD never advanced and the
index stayed stale-but-honest. The PRD's own Risk section assumed skipped rows would be
**absent** from query results ("skipping a bad file means its rows are absent"); the
implementation makes them **stale-present**, which the PRD never sanctioned. Only 1 of 5
reviewers found it.

**3. Carl's CRITICAL — REFUTED at severity, kept as a MEDIUM/HIGH testing gap.** Carl
asserts the loser's drop deletes tables the winner is using. Verified at
`ddb-core/src/indexer/mod.rs:197-241`: `_guard` is bound at function scope and held across
the drop loop, the `SCHEMA_DDL` recreate, and the `user_version` stamp. That is exactly
what commit `0537b18` changed, so Carl describes the pre-fix shape. Eve independently
traced the durable-data consequence and refuted it too: drops are stamp-gated and row
population is both lock- and stamp-gated, so even the buggy guard scope could not destroy
populated rows. The *missing regression test* half of his finding is real, carries the
review's highest agreement (Alice, Bob, Carl, Eve = 4/5), and is being fixed this cycle.

**4. Eve's `summarize_reindex_warnings` finding — CONFIRMED on all three counts.**
At `output.rs:44`, `describe_consistency_warning(&warnings[0])` takes the first warning
regardless of variant; on the cold-start/destructive branch `ensure_fresh` receives a full
`rebuild()` report, whose warnings include advisory `CrossZoneDuplicate`/`MissingRequired`
entries for files that *were* successfully indexed — those get labelled
`REINDEX_SKIPPED_FILES`. At `:48`, `warnings.len() - 1` has no dedup, so the known
double-report (each malformed file counted once by `parallel_parse` and again by
`collect_consistency_warnings`) makes a single poison file render
`"(+1 more, see ddb doctor)"`, naming a second file that does not exist. And `ddb doctor`
is not a command — PRD 00181, which would add it, is still in backlog.

## Alice

Six findings (1 high, 4 medium, 1 low), reproduced in the table above. Her three lead
rulings:

- **Lead 1 (task 8's untested CRITICAL):** traced `git show 0537b18` directly. The
  "indistinguishable end state" argument holds for permanent row-data loss, but overreaches
  by treating "no test is possible" as proven when what is shown is "no *deterministic* test
  is possible". A probabilistic contention test of the shape already committed as
  `two_racing_cold_start_rebuilds_run_the_destructive_work_only_once` — and of the task-14
  e2e that *did* catch the sibling `locked_rebuild` bug at roughly a 1-in-3 detection rate —
  was never attempted here. The cycle-2 reviewer proposed exactly this and was overruled on
  flakiness-aversion, not soundness.
- **Lead 2 (e2e flakiness):** attribution holds, and she named the mechanism —
  `tests/e2e/common.rs` (untouched by this diff) allocates ports with a bind-then-release
  `TcpListener` probe, a TOCTOU window under 451-way parallelism. Both flaking tests live in
  files this PRD never touched.
- **Lead 3 (false-pass class):** swept the new diff and found no fourth instance. The
  lenient e2e test documents its deliberately-omitted read-back, the GraphQL sections assert
  against the first mutation only with an explicit no-warm-up comment, and smoke section 32
  captures exit codes explicitly in both the `.sh` and `.ps1` ports.

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
R13: fail

## Blake

Blind lens, PRD-only — no diff, no design doc, no task list, no review history. Four
findings (1 critical, 1 medium, 2 low). His CRITICAL is the search-verb warning gap; the
severity was adjudicated down to HIGH at the gate (see Decision Gate below), on the grounds
that the three reviewers who read the implementation all rated it lower, the skip is not
silent at the log level, and the fix is owned by backlog PRD 00176. His finding is
otherwise accepted as correct and is deferred, not dismissed.

Blake independently noticed that `CHANGELOG.md:13` names only `create`, `update` and
`apply_schema` — the project's own changelog confirms the narrower scope that actually
shipped.

B1: fail
B2: pass
B3: fail
B4: pass
B5: pass
B6: pass
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

Twelve findings (4 high, 5 medium, 3 low) plus his doubt-lens buckets. He is the only
reviewer who found the stale-rows bug (verified above), which alone justified his lane this
cycle. His de-slop pass produced the batch/singleton duplication and file-size findings.

Doubt buckets: 10 FIX, 1 VERIFY, 1 KNOWN.

R1: fail
R2: fail
R3: pass
R4: fail
R6: pass
R7: fail
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

Five findings (1 critical, 4 medium). No frontend surface in this diff, so he reviewed as a
generalist, as instructed. His CRITICAL was refuted at severity (see verification 3 above);
his four medium findings on file/function size and lock-acquisition duplication corroborate
Alice and Bob and are being fixed.

R1: fail
R2: fail
R3: pass
R4: pass
R6: pass
R7: pass
R8: pass
R9: pass
R10: pass
R11: pass
R12: fail
R13: fail

## Eve

Doubt lens, Fable 5. Seven FIX, three VERIFY, eleven KNOWN. Her lead rulings were the most
load-bearing analysis in the cycle:

- She **upheld** the task-8 accepted gap on its merits, tracing why durable data cannot be
  destroyed even under the buggy guard scope (drops are stamp-gated; row population is both
  lock- and stamp-gated; once any process stamps, no further drop fires). She noted the
  "no test can discriminate" claim is technically overstated — a deterministic mid-window
  probe via a rusqlite authorizer/progress hook exists — but that rusqlite is built with
  `features = ["bundled", "vtab"]`, no `hooks`, so it would need a feature addition.
- She then found the **adjacent** half nobody else isolated: the post-lock `read_needs_drop`
  re-check at `indexer/mod.rs:212-217` has no test and **is** deterministically testable
  today with no new dependency. That is now a rework task.
- On lead 3 she found the mirror-image gap: no test anywhere asserts the **CLI** `AppWarning`
  leg (`write_warnings` → stderr on `ddb create`). Smoke section 32 binds the `reindex`
  command's *tracing* output instead, so the PRD metric "asserted present on at least CLI +
  GraphQL" is half-met.

D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

### Eve — FIX

- `summarize_reindex_warnings` mislabels and miscounts on the cold-start full-rebuild path — `ddb-core/src/app_contract/output.rs:168` — filter to `UnreadableFile|MalformedYaml` and dedup by path before counting
- Warning message says "see ddb doctor" but no doctor command exists — `ddb-core/src/app_contract/output.rs:48` — reword to a true pointer and update the two message-shape tests
- `configure_connection`'s post-lock `needs_drop` re-check is untested and a deterministic test needs no new dependency — `ddb-core/src/indexer/mod.rs:212` — hold the lock, upgrade+stamp+sentinel out-of-band, release, assert the waiter skipped the drop
- The CLI `AppWarning` leg is unasserted everywhere; smoke 32 binds `reindex`'s tracing stderr, not the verb envelope — `tests/smoke.sh:713` — add a `ddb create` scenario asserting `REINDEX_SKIPPED_FILES` on stderr, in both `.sh` and `.ps1`
- `ddb reindex` prints only "N warning(s)"; skipped paths reach the user only via tracing stderr, so `RUST_LOG=error` names nothing — `ddb-cli/src/commands/maintenance.rs:62` — print one line per report warning
- Doc drift: `incremental_reindex` signature still shown as two-arg — `docs/src/technical/indexer.md:118`
- CHANGELOG omits the compile-breaking public shape change (`ConsistencyWarning` gained `UnreadableFile`) and the poison-file Fixed bullet lacks its PRD tag — `CHANGELOG.md:29`

### Eve — VERIFY

- E2E flake attribution unproven — run `cargo build -p ddb-cli && cargo test -p ddb-e2e` twice at base commit `6bb1617` and confirm the same sporadic 1-of-451 failures occur pre-PRD
- `integration.sh` section 63 has never executed locally — confirm the first nightly `full-validation.yml` run on the pushed master passes it (check headSha + duration, since the run can be skip-guarded)
- `integration.ps1` §63 and `smoke.ps1` §32 have never run anywhere — confirm the first Windows CI execution passes, specifically `$LASTEXITCODE` after `& $DDB reindex --strict` and the guarded teardown

### Eve — KNOWN

- Task 8's fix shipped without a fail-first regression test — durable end state provably converges; the deterministic mid-window discriminator needs rusqlite's `hooks` feature; partially compensated by the re-check test in FIX
- The "create AND search both carry a skip AppWarning" AC is unmet for search — blocked on 00176; bolting an envelope on now would duplicate that PRD's reshape
- Consume-once semantics: only the FIRST verb after a poison commit carries the warning, so "create AND search each carries" is unimplementable as written
- `MAX_FRONTMATTER_BYTES` bounds parse cost, not read cost — inherent to git2's blob model
- Two unlocked destructive-rebuild routes remain (bundle import's direct `index.rebuild`, ReadPool's `skip_stale_check`) — both documented accepted gaps in invariants.md I10
- One malformed file is reported twice by the full rebuild, so `ddb reindex` prints "2 warning(s)" for one file — documented wart; per-file dedup deferred to 00181
- Pre-existing `! $DDB` errexit-exempt idioms in older smoke.sh sections — left untouched under surgical-changes
- The `strict` parameter threaded through `incremental_reindex`/`batch_index_changes` is production-dead (`rebuild_if_stale` always passes false) — but the threading is PRD-mandated and unit-tested
- The in-transaction diff-failure fallback can still reach `locked_rebuild` inside an open `BEGIN IMMEDIATE` — pre-existing, doubly-degenerate, untouched by this diff
- Findings-precedent check (00164 id-less collision at `id=""`): verified this PRD does not widen it

## Decision Gate (Phase 5, cycle 1 of cap 2)

Cap check: `cycle 1 < rework_cap 2`, and unresolved HIGH findings remain, so rework is
allowed and this cycle does **not** converge.

Twelve findings were settled without rework and are recorded with verified reasons in
`dev/local/reviews/00169-poison-file-reindex-resilience-v1-ledger.json` (10 settled
deferrals, 2 discards). Four are also logged to the batch deferred record. The rest are
routed to rework as six `[D1-*]` tasks.

No blocking escalation, so the batch is not stalled and no PAUSE is warranted.

## Follow-up Tasks Created

1. `[D1-1]` Evict stale rows for modified files that fail read/parse (M) — 🟠 high — addresses the confirmed stale-present regression
2. `[D1-2]` Fix `summarize_reindex_warnings` mislabel/miscount and the dangling `ddb doctor` pointer (S) — 🟡 — addresses Eve FIX 1 and 2
3. `[D1-3]` Add the missing lock-scope regression coverage (M) — 🟡, 4/5 consensus — discharges the PRD's known-unmet fail-first obligation, plus the e2e row-count check
4. `[D1-4]` Extract the triplicated rebuild-lock acquisition and the duplicated batch/singleton read-parse logic; bring `indexer/mod.rs` back under 800 lines (M) — 🟡, 3/5 consensus
5. `[D1-5]` Make `ddb reindex` name the skipped paths and assert the CLI `AppWarning` leg in smoke (S) — 🟡
6. `[D1-6]` Correct the docs: `invariants.md` I5 scope, `indexer.md` signature + collect-and-skip prose + schema stamp + frontmatter cap, CHANGELOG breaking-change entry (S) — 🟡, 3/5 consensus

Verdict: 26 findings
Tests: 1804 passed, 0 failed, 0 skipped

> **Provenance of the `Tests:` line — read this before treating it as a fresh result.**
> These are the build gate's step-7 `cargo test-ci` counts, taken at this exact HEAD
> (`f00c483`) at the close of the build session. The suite was **not** re-run inside this
> review cycle: `cargo test-ci` takes roughly 16 minutes on this host, exceeds the
> foreground command ceiling, and no source file changed between that run and this review.
> Backgrounding it would also have contended on the cargo build lock with the four reviewers
> that run cargo. Separately, `cargo test -p ddb-e2e` was 450/451 in that session, with the
> 1-of-451 flake described above and deferred as its own PRD.
