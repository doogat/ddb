# Autopilot Batch Report 202607161128

Started: 2026-07-16T11:28:00Z

## 00163-git-tree-construction-correctness-v1.md

- Completed: 2026-07-17T19:04:43Z
- Cycles: 3
- Tasks: 7/7

### Autonomous Decisions

| Cycle | Issue | Severity | Action | Reason |
|-------|-------|----------|--------|--------|
| 0 | Stale/inconsistent state.json from a prior shipped PRD | high | recovered | matched "state file corrupted" recovery row, not a resumable pause |
| 0 | PRD 00163 missing YAML frontmatter | low | auto-fix | took all Phase 0 defaults per shared fallback table |
| design | PRD mixed a P0 lock fix with a harder three-way-merge-tree problem | high | user-decided split | user chose "Split the PRD" from 4 options; new PRD 00200 placed in wip/ to run immediately after 00163 |
| design | Lock-race test as first specified was vacuous | medium | auto-fix | respecified to commit-under-raw-guard + ancestor assertion during design review |
| build | HEAD moved since last catchup (delta = own commits) | low | delta refresh (not full catchup) | verified via git log that the delta was self-caused, no external commits |
| 1 | Doc accuracy: AGENTS.md/git-ops.md/CHANGELOG/sync.md/invariants.md drift on merge-lock scope + reentrancy wording | medium | auto-fix | load-bearing P0 invariants doc self-contradicted invariants.md |
| 1 | `commit_merge`'s separate write-lock wrap has zero contention regression coverage | medium | auto-fix | rules/testing.md mandates regression coverage on a P0 data-loss path |
| 2 | AGENTS.md I1 bullet pointed the bundle-import lock gap at the wrong module (`git_ops/merge.rs` vs `bundle::merge_bundle_and_resolve`) | low | auto-fix | confirmed factual error on a P0 non-negotiable invariants doc |

### Doubt Rubric Verdicts

| Rule | Verdict |
|------|---------|
| R1 | pass |
| R2 | pass |
| R3 | pass |
| R4 | pass |
| R5 | pass |

### Loop Metrics

| Launch phase | Sessions | Wall secs | Model | Cost USD |
|--------------|----------|-----------|-------|----------|
| build | 2 | 4862 | claude-opus-4-8 | 32.45 |
| review | 2 | 7485 | claude-opus-4-8 | 52.19 |
| **Total** | 4 | 12347 | | 84.64 |

### Implementor Mix

| Implementor | Attempts |
|-------------|----------|
| claude | 6 |
| qwen | 1 |

Qwen preflight outcomes: healthy 2
Excluded from qwen: tier 1, files 1, docs-judgment 2, fail-first-verification 1

### Deferred to Batch End

| Issue | Severity | Reason |
|-------|----------|--------|
| tests.rs 963 > 800-line guideline; mod.rs 846 > 800 pre-existing; one ~88-line concurrency test | medium | not a CI gate; concurrency test should not be force-split |
| No tests/smoke.sh or smoke.ps1 scenario for the `resurrected:` CLI line | medium | covered by e2e + integration.sh; smoke scenario is integration-shaped |
| No showboat walkthrough for the CLI `resurrected` addition | low | single-line CLI change, low demo value |
| Reporting drift: test name in writeup vs actual test name | low | cosmetic, coverage intact |

## 00200-make-merge-tree-three-way-v1.md

- Completed: 2026-07-18T05:59:46Z
- Cycles: 3
- Tasks: 6/6

### Autonomous Decisions

| Cycle | Issue | Severity | Action | Reason |
|-------|-------|----------|--------|--------|
| design | Rebuild the conflicted-merge tree as a true three-way merge (Alternative B: rebuild from `merge_commits`, overlay CRDT-resolved blobs) | high | adopted design | maintainer-decided 2026-07-17; empirically proven two-way diff resurrects deletions and reverts non-conflicting edits |
| design | `commit_merge` needs a divergence guard against a HEAD move between resolve and commit | high | auto-fix (dispatch-2 blocker) | `has_conflicts()`-after-overlay insufficient; `conflict_remove` returns `Ok` not `NotFound` for an auto-resolved path |
| design | Binary conflict handling wrote winning bytes to the worktree before commit | high | auto-fix (dispatch-3 cardinal sin) | pre-commit worktree write strands stale bytes on abort; switched to OID overlay + post-commit checkout |
| 1 | Docs task left 4 spots describing pre-00200 (two-way) mechanics | medium | auto-fix | contradicted shipped three-way code in load-bearing P0 docs |
| 2 | Fifth stale `fresh_index()` spot in invariants.md I1 missed by cycle-1 rework | medium | auto-fix | same doc-drift class recurring; ran a full staleness sweep, confirmed no sixth spot |

### Doubt Rubric Verdicts

| Rule | Verdict |
|------|---------|
| R1 | pass |
| R2 | pass |
| R3 | pass |
| R4 | pass |
| R5 | pass |

### Loop Metrics

| Launch phase | Sessions | Wall secs | Model | Cost USD |
|--------------|----------|-----------|-------|----------|
| build | 5 | 14185 | claude-opus-4-8 | 95.20 |
| review | 2 | 5828 | claude-opus-4-8 | 42.53 |
| **Total** | 7 | 20013 | | 137.73 |

### Implementor Mix

| Implementor | Attempts |
|-------------|----------|
| claude | 4 |
| qwen | 2 |

Qwen preflight outcomes: healthy 2
Excluded from qwen: tier 1, files 1, unknown 2

### Deferred to Batch End

| Issue | Severity | Reason |
|-------|----------|--------|
| Pre-existing file/function-size overages (git_ops/tests.rs ~1525, sync_manager/tests.rs ~926, sync_manager/mod.rs ~898, commit_merge ~80 lines) | high | not a CI gate; already owned by backlog PRD 00183 |
| Divergence-guard comment/doc wording overstates "any HEAD move fails loud" vs actual conflict-path-set comparison | medium | behavior correct and design-documented (Q-1); not worth expanding a P0 diff |
| DRY nits: shared `our.or(their).path` pattern, one test inlines IndexEntry instead of `stage0_entry` | low | cosmetic, fold into 00183 or a future hygiene pass |
| integration.ps1 Pop-Location comment mislabel; no showboat walkthrough | low | cosmetic (Push/Pop balanced); no new user-facing surface for this PRD |

## 00164-unify-id-minting-v1.md

- Completed: 2026-07-18T12:11:50Z
- Cycles: 2
- Tasks: 10/10

### Autonomous Decisions

| Cycle | Issue | Severity | Action | Reason |
|-------|-------|----------|--------|--------|
| design | One repo-aware existence oracle vs patch-each-site vs widening SqlEngine to hold GitBackend | high | adopted design | leaf-module + snapshot-both; patch-each-site is weak unification, widening SqlEngine touches ~250 call sites |
| 1 | Task 6 SM2 (batch-then-raw-create) regression sizing per design's "N=2-3" guidance did not reliably fail against old code | medium | auto-fix | widened to N=5 multi-row INSERT so the reserved window (~4s) exceeds realistic commit latency; verified both SM1/SM2 fail vs old code, pass vs new |

### Doubt Rubric Verdicts

| Rule | Verdict |
|------|---------|
| R1 | pass |
| R2 | pass |
| R3 | pass |
| R4 | pass |
| R5 | pass |

### Loop Metrics

| Launch phase | Sessions | Wall secs | Model | Cost USD |
|--------------|----------|-----------|-------|----------|
| build | 6 | 16757 | claude-opus-4-8 | 120.85 |
| review | 1 | 4720 | claude-opus-4-8 | 34.41 |
| **Total** | 7 | 21477 | | 155.26 |

### Implementor Mix

| Implementor | Attempts |
|-------------|----------|
| claude | 6 |
| qwen | 5 |

Qwen preflight outcomes: healthy 5
Excluded from qwen: tier 1, files 1, contract 1, unknown 2
capability breaker: not tripped

### Deferred to Batch End

| Issue | Severity | Reason |
|-------|----------|--------|
| Parser tightening makes meta.id=None more reachable; two id-less/malformed non-native files could collide at id="" in the index | low | index-only, rebuildable; needs hand-authored non-native files; candidate for 00169 |

## 00165-deterministic-crdt-resolution-v1.md

- Completed: 2026-07-18T14:00:00Z
- Cycles: 3
- Tasks: 5/5

### Autonomous Decisions

| Cycle | Issue | Severity | Action | Reason |
|-------|-------|----------|--------|--------|
| design | Automerge breaks concurrent-edit ties by random actor ID, so two nodes can resolve the same conflict differently | high | adopted design | content-hash `derive_actor` applied via `set_actor` at all 9 doc-build sites + deterministic role-independent `merge_list_fields` ordering (required companion found by codex review) |
| 1 | Scenario 26b + one unit test did not guard the PRD's explicit success metrics | medium | auto-fix | additive test/scenario strengthening; not a capability failure |
| 2 | Test 7 asserted only `a==b`, which passes even if the merge drops both inserts | medium | auto-fix | rules/testing mandates binding tests to intent |
| 3 | Convergence check across all 5 lenses | - | converged | doubt lens (Bob) returned zero findings; 4/5 reviewers fully clean |

### Doubt Rubric Verdicts

| Rule | Verdict |
|------|---------|
| R1 | pass |
| R2 | pass |
| R3 | pass |
| R4 | pass |
| R5 | pass |

### Loop Metrics

| Launch phase | Sessions | Wall secs | Model | Cost USD |
|--------------|----------|-----------|-------|----------|
| build | 3 | 8418 | claude-opus-4-8 | 49.54 |
| review | 1 | 5047 | claude-opus-4-8 | 42.08 |
| **Total** | 4 | 13465 | | 91.62 |

### Implementor Mix

| Implementor | Attempts |
|-------------|----------|
| claude | 3 |
| qwen | 2 |

Qwen preflight outcomes: healthy 2
Excluded from qwen: tier 1, unknown 2
capability breaker: not tripped

### Deferred to Batch End

| Issue | Severity | Reason |
|-------|----------|--------|
| `crdt_resolver.rs` 1310 > 800-line guideline (pre-existing); `merge_frontmatter`/`merge_list_fields` > 50-line guideline | low | not a CI gate; surgical determinism PRD, out of scope; candidate for 00199 or a dedicated split PRD |

## 00166-make-hlc-load-bearing-v1.md

- Completed: 2026-07-19T03:36:21Z
- Cycles: 3
- Tasks: 11/11

### Autonomous Decisions

| Cycle | Issue | Severity | Action | Reason |
|-------|-------|----------|--------|--------|
| design | Machine-local HlcClock (`.git/ddb-hlc`) injected into GitRepo; one `create_commit` chokepoint stamps all 7 non-merge + both merge write sites; unified `lww_pick` at all 3 tie-break sites | high | adopted design (Alt 2) | 3 design dispatches (1 claude, 2 codex) found and fixed 9 blockers; 0 open at exit |
| 1 | `MAX_REVWALK_DEPTH` still 100 (PRD scope addition unmet) | medium | auto-fix | design specified 1000; additive, no signature/schema change |
| 1 | Dead `SyncManager::recv_hlc` (zero callers, pub so clippy misses it) | low | auto-fix | design said "removed as dead"; build kept it |
| 1 | Three stale comment/doc spots (hlc_clock.rs header, sync_manager tie-break comments, sync.md recovery wording) | low | auto-fix | described pre-refactor behavior |
| 1 | Blind (Blake): HlcClock-field-of-GitRepo layer concern | low | no-action | accepted Alt-2 design, re-raised every cycle |
| 1 | Reviewer degradation: codex Fact-Forcing-Gate block, gemini stall, qwen unreachable | - | degraded gracefully | all 3 core lenses produced parseable output; no pause |
| 2 | Regression from [D1] rework: sync.md:172 named deleted `recv_hlc`; sync.md:170 overclaimed no-trailer-less-commit-path | medium | auto-fix | additive doc-only fix, no signature/schema change |
| 2 | Blind (Blake): HlcClock-field-of-GitRepo (re-raised) | low | no-action | unchanged from cycle 1 |
| 2 | Sandbox (Bob): could not statically verify revwalk regression test | low | no-action | orchestrator verified first-hand: cargo test-ci green, fail-first recorded |
| 2 | Reviewer degradation: codex Fact-Forcing-Gate recovered, qwen unreachable | - | degraded gracefully | all 3 core lenses + Carl produced parseable output |
| 3 | Blind (Blake): 3 findings — layer concern, spec-vs-impl naming, revwalk legacy case | low | no-action | all settled/expected, none drive rework |
| 3 | Rework cap (3) reached without full convergence (4 findings, all 1/4 minority, all low) | - | converged-with-deferrals | no unresolved critical/high; both cycle-2 [D2] findings verified resolved |

### Doubt Rubric Verdicts

| Rule | Verdict |
|------|---------|
| R1 | pass |
| R2 | pass |
| R3 | pass |
| R4 | pass |
| R5 | pass |

### Loop Metrics

| Launch phase | Sessions | Wall secs | Model | Cost USD |
|--------------|----------|-----------|-------|----------|
| build | 7 | 23684 | claude-opus-4-8 | 178.98 |
| review | 4 | 17155 | claude-opus-4-8 | 62.84 |
| **Total** | 11 | 40839 | | 241.82 |

### Implementor Mix

| Implementor | Attempts |
|-------------|----------|
| claude | 9 |
| qwen | 3 |

Qwen preflight outcomes: healthy 3
Excluded from qwen: tier 4, backend_down 4
capability breaker: not tripped

### Deferred to Batch End

| Issue | Severity | Reason |
|-------|----------|--------|
| Pre-existing oversized files grown by this additive PRD (git_ops/mod.rs 844, sync_manager/mod.rs 904, git_ops/tests.rs 1982) | low | not a CI gate; already owned by backlog PRD 00183 |
| `HlcClock::persist()` uses a fixed shared temp name; unlocked load-time repair could momentarily interleave with a concurrent process | low | self-mitigated (write-lock + max(file,HEAD) reseed + in-memory floor); no data-safety risk |
| Undocumented double-trailer footgun at the `create_commit` chokepoint | low | no production caller pre-stamps a trailer; documented in design, code lacks a comment |
| sync.md:168 intro sentence overclaims trailer coverage vs the scoped bullet 2 lines below | low | cosmetic doc-consistency nit; cap-out deferral, no correctness impact |

## 00167-sync-collision-reassignment-atomicity-v1.md

- Completed: 2026-08-15T20:56:44Z
- Cycles: 2
- Tasks: 21/21

### Autonomous Decisions

| Cycle | Issue | Severity | Action | Reason |
|-------|-------|----------|--------|--------|
|  | Autopilot paused: task tools (TaskCreate/List/Update/Get) absent - CC 2.1.233 deliberately gates them off on Fable 5 |  |  |  |
|  | Pat per-task review of task 4 (fold loser reassignment into commit_merge) flagged CRITICAL (sync_manager production caller still hardcodes losers=&[] and routes through the old two-commit reassign_collision_losers path) and HIGH (old dead-path functions still present). |  |  |  |
|  | Final verification (step 7): cargo test -p ddb-e2e (mandated by CLAUDE.md after task 5 deleted the dead reassignment code path) reported 1 failure: typed_where_filter::id_op_vs_junction_some_diverge_on_multi_reference (Connection reset by peer against a local GraphQL test server). |  |  |  |
|  | Cycle 1 review (5 lenses: Alice, Blake, Bob/codex, Carl/gemini, Eve/fable) returned 18 consolidated findings including 4 CRITICAL and 5 HIGH. |  |  |  |
|  | Cycle 2 review (5 lenses: Alice, Blake, Bob/codex, Carl/gemini, Eve/fable) returned 27 consolidated findings: 0 CRITICAL, 5 HIGH, 17 MEDIUM, 5 LOW. Cycle 2 == rework_cap 2, so the cap gate applies. |  |  |  |

### Doubt Rubric Verdicts

| Rule | Verdict |
|------|---------|
| D1 | pass (codex) / pass (fable) |
| D2 | pass (codex) / pass (fable) |
| D3 | pass (codex) / pass (fable) |
| D4 | pass (codex) / pass (fable) |
| D5 | pass (codex) / pass (fable) |

### Loop Metrics

| Launch phase | Sessions | Wall secs | Model | Cost USD |
|--------------|----------|-----------|-------|----------|
| build | 20 | 18242 | claude-opus-4-8, claude-sonnet-5[1m] | 133.94 |
| review | 8 | 21816 | claude-opus-5[1m] | 211.32 |
| **Total** | 28 | 40058 | | 345.26 |

### Implementor Mix

| Implementor | Attempts |
|-------------|----------|
| claude | 14 |
| qwen | 5 |
| codex | 2 |

Qwen preflight outcomes: healthy 5
Excluded from qwen: contract 2, unknown 10 (plan-time); dispatch-time reroutes: memory_pressure 4
codex probe: healthy (backend: codex)
capability breaker: not tripped

### Deferred to Batch End

| Issue | Severity | Reason |
|-------|----------|--------|
|  |  |  |
|  |  |  |
| Unrecoverable sync deadlock: an add-add conflict whose losing content has no YAML frontmatter makes rewrite_id_field fail, fold_one_loser abort, and commit_merge commit nothing; the next sync recomputes the identical merge and fails identically, with no --skip, no --force and no documented recovery. Pre-00167 the winner commit landed so sync converged lossily; now it never converges. | high |  |
| partition_conflicts routes EVERY non-reference/, ancestor-less, both-sides-present conflict into the add-add bucket with no ddb/ prefix guard, so a non-doogat file (.gitignore, README.md, any docs/*.md) is treated as a doogat ID collision: old_id becomes its file stem and the loser bytes are relocated to ddb/{14-digit}.md, fabricating a bogus doogat while the real path keeps only the winner. | high |  |
| The independent-resolution test can false-green against the old wall-clock minter because both universes may resolve within the same second; it never pins the loser path to a specific derive_content_id output. Assert each universe loser path equals derive_content_id(old_id, losing_blob_oid). | high |  |
| 18 medium/low cycle-2 findings deferred without a sweep pass, enumerated in full in dev/local/autopilot/deferred/202607161128-deferred.json. Most notable: the task-16 audit warn fires BEFORE the commit exists (merge.rs:201-207), raised by both Eve and Bob, so an aborted batch logs losers as reassigned though HEAD never moved; CHANGELOG lines 23-25 present three never-released regressions as user-facing fixes; and the error-names-the-document half of task 21 is untested. |  |  |

## 00169-poison-file-reindex-resilience-v1.md

- Completed: 2026-08-16T20:46:22Z
- Cycles: 2
- Tasks: 0/0

### Autonomous Decisions

| Cycle | Issue | Severity | Action | Reason |
|-------|-------|----------|--------|--------|
|  |  |  |  |  |
|  |  |  |  |  |
|  |  |  |  |  |
|  | DoogatService::reindex calls self.index.rebuild where self.index is a generic I: IndexPort, so rebuild is a TRAIT method (declared traits.rs:169, impl indexer/ports.rs:138). Routing reindex to the new locked_rebuild needs a new IndexPort method plus its impl, and neither traits.rs nor indexer/ports.rs was in task 7 file allowlist. Widen the allowlist, or drop the reindex leg? |  |  |  |
|  | service/search.rs::reindex calls rebuild through a generic I: IndexPort, so a pub(crate) inherent Index::locked_rebuild is unreachable. How should reindex reach the locked path? |  |  |  |
|  | Task 7 routed DoogatService::reindex through Index::locked_rebuild, whose post-lock re-check (check_integrity AND NOT is_stale) short-circuits and returns RebuildReport::default(). Does the explicit ddb reindex command still rebuild? |  |  |  |
|  |  |  |  |  |

### Doubt Rubric Verdicts

| Rule | Verdict |
|------|---------|
| D1 | pass (codex) / pass (fable) |
| D2 | pass (codex) / pass (fable) |
| D3 | pass (codex) / pass (fable) |
| D4 | pass (codex) / pass (fable) |
| D5 | pass (codex) / pass (fable) |

### Loop Metrics

| Launch phase | Sessions | Wall secs | Model | Cost USD |
|--------------|----------|-----------|-------|----------|
| build | 8 | 33750 | claude-sonnet-5[1m], claude-opus-5[1m] | 277.60 |
| review | 5 | 15496 | claude-opus-5[1m] | 153.58 |
| **Total** | 13 | 49246 | | 431.18 |

### Implementor Mix

no implementor data

### Deferred to Batch End

| Issue | Severity | Reason |
|-------|----------|--------|
|  |  |  |
|  |  |  |

## 00170-id-throughput-ceiling-decision-v1.md

- Completed: 2026-08-16T22:15:08Z
- Cycles: 2
- Tasks: 0/0

### Autonomous Decisions

| Cycle | Issue | Severity | Action | Reason |
|-------|-------|----------|--------|--------|
| 1 |  |  |  |  |
| 1 |  |  |  |  |
| 2 |  |  |  |  |

### Doubt Rubric Verdicts

| Rule | Verdict |
|------|---------|
| D1 | pass |
| D2 | pass |
| D3 | pass |
| D4 | pass |
| D5 | pass |

### Loop Metrics

| Launch phase | Sessions | Wall secs | Model | Cost USD |
|--------------|----------|-----------|-------|----------|
| build | 2 | 2081 | claude-sonnet-5[1m] | 16.80 |
| review | 2 | 3143 | claude-opus-5[1m] | 35.82 |
| **Total** | 4 | 5224 | | 52.62 |

### Implementor Mix

no implementor data

### Deferred to Batch End

| Issue | Severity | Reason |
|-------|----------|--------|
| The chosen scheme is not collision-free: distinct UUIDs can share the truncated 32-bit node suffix, while the planned test only requires distinct full UUIDs | critical |  |
| The malformed-node fallback creates a second node-identity rendering: IDs hash invalid values while the explicitly read-only HlcClock still uses raw truncate_node, contradicting the recorded rationale for keeping one shared representation | high |  |
| The promised structured malformed-node warning has no propagation plan: DoogatService::unique_id and SqlEngine::unique_ids return IDs without warnings, and several create paths do not return AppOutput | high |  |
| The deterministic hash fallback does not pin a hash algorithm, domain separator, or byte-to-hex rule, so the requirement that every process and device derive the same field is not implementation-stable | medium |  |

## 00171-migrate-shell-scenarios-to-e2e-v1.md

- Completed: 2026-09-07T16:30:13Z
- Cycles: 2
- Tasks: 16/16

### Autonomous Decisions

| Cycle | Issue | Severity | Action | Reason |
|-------|-------|----------|--------|--------|
| 1 | Review diff/commit range for PRD 00171 bundles unrelated PRD 00202 HLC-logic commits (git_ops/read.rs, tests.rs) — a process/traceability issue, not a defect in 00171's own work | low | discard | Git history identifies interleaved, separately authored PRD 00202 commits; not scope creep by 00171. Full work_start_sha is intentionally preserved. |
| 1 | [FIX] Per-push CI coverage dropped: `test.yml`'s new `Smoke test (e2e)` step runs only the 30 `smoke_*` tests (of 565), yet ~26 of smoke.sh's 56 sections were mapped to pre-existing modules outside that filter (delete_backlinks, tags, checkboxes, folder_namespace, search_links, link_kinds, rename, sql_lifecycle, alter_table_rename, cli_create_fk_freshness, inline_zone, discover, fix, sequence, help, cascade_delete, sql_expressions_test, set_flag, date_default, alter_column_type, schema_apply, poison_file_reindex, cli_create_unregistered_type, cli_update_app_error, sync_crdt); smoke.sh ran those on every push, now they run nightly only, contradicting the PRD's "CI keeps identical coverage" — .github/workflows/test.yml:45-48 — replace the `smoke_` step and the five `--exact` steps with one `cargo test -p ddb-e2e` step (the full suite took 223 s in the 5cf4c36 linux-e2e job), drop the `DDB_BIN` env (nothing under tests/ reads it), and fix the test.yml sentence at AGENTS.md:116. | high | auto-fix | Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change. |
| 1 | [FIX] §42 totalCount assertions dropped: bash 1505/1517 asserted `totalCount == 1`; the cited "ALREADY COVERED" tests request the field but never read it — tests/e2e/serve.rs:1408-1410 (`filter_base_field_title_eq`) and :1443-1445 (`filter_base_field_title_contains`) — add `assert_eq!(result["data"]["tasks"]["totalCount"].as_i64().unwrap(), 1)` to each. | medium | auto-fix | Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change. |
| 1 | [FIX] §44.E1 JOIN pin degraded to single-character containment: `row_str.contains('a')`/`contains('1')` also pass on `["abc","10"]`; bash 1699-1700 grepped the quoted tokens — tests/e2e/integration_junction_sync.rs:35-43 — parse `rows[0]` as JSON and `assert_eq!(row, json!(["a","1"]))`. | medium | auto-fix | Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change. |
| 1 | [FIX] §59.A cross-process read-back dropped: bash 2667-2677 read the typed column back through a separate `ddb query` process (its comment names that as the point); the Rust target reads through the same server's executeSql — tests/e2e/rest.rs:153-154 — add `repo.ddb().args(["query", "SELECT category FROM item WHERE id = '<id>'"]).assert().success().stdout(contains("books"))`. | medium | auto-fix | Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change. |
| 1 | [FIX] §18d2 eq-exactness fixture lost: bash 341-346 seeded `https://example.com/page` so a prefix/LIKE `eq` would fail; the "ALREADY COVERED" test seeds two identical `https://example.com` plus `https://other.org`, so a prefix match still passes — tests/e2e/serve.rs:1830-1857 (`search_where_filter_materialized_column_eq`) — change Link B's url to `https://example.com/page` and expect 1 hit / totalCount 1. | medium | auto-fix | Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change. |
| 1 | [FIX] §38b2b raw-scalar pin with multiple junction rows lost: bash 1260-1265 linked three categories and asserted `rlcat_id == <first-linked>`; the cited test links one — tests/e2e/reference_raw_id.rs:112-143 — add a three-category variant asserting `category_id` equals the first-linked id. | medium | auto-fix | Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change. |
| 1 | [FIX] §17.J2 numeric-id assertion weakened: bash 217 required `"message":"[0-9]+"`, Rust asserts `!config_id.is_empty()` — tests/e2e/integration_graphql_jink.rs:158 — assert a 14-digit numeric id. | medium | auto-fix | Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change. |
| 1 | PRD checklist unchecked and file still in wip/ despite verifiably complete, CI-green implementation | low | discard | PRD is intentionally wip until review converges and a fresh finalize session accepts it. |
| 1 | [FIX] Slop: `stdout()` copy-pasted in smoke_core.rs:6, smoke_sql.rs:4, smoke_search.rs:4; the 14-digit-id check re-implemented as `assert_doogat_id` (smoke_sql.rs:12), `is_14_digit_id` (integration_id_poison.rs:40) and inline at integration_nosql_cli.rs:40,88, integration_singleton_parity.rs:61, integration_singleton_races.rs:87,111 — tests/e2e/common.rs — move one `stdout()` and one `assert_doogat_id()` there and call them (also closes the §17.J2 item). | low | auto-fix | Cycle 1: bounded additive test/CI coverage repair or specified helper deduplication; no product contract change. |
| 1 | [VERIFY] Tier 2 has not run on the deletion commit: the green manual dispatch (run 33991344845, 2026-09-05 20:51-21:48, `565 passed; 0 failed; 0 ignored` on linux-e2e and windows-full) is on 5cf4c36, one commit before c8b72bd; only test.yml (run 34016441899, 21 min, green) has run on c8b72bd — after the next nightly run `gh run list --repo doogat/ddb --workflow=full-validation.yml --limit 1 --json headSha,conclusion,createdAt,updatedAt`; it must show headSha c8b72bd or later, conclusion success, a real duration (≥45 min), and `gh run view --job <linux-e2e id> --log` must contain `test result: ok. 565 passed; 0 failed; 0 ignored`. | low | discard | Live GitHub confirmed the contract-required full validation before deletion and Test after deletion. A fixed duration and old exact test count are not validity gates. Rework will be pushed for fresh CI. Not queued: command shape. |
| 1 | [KNOWN] §54.D/§55.D psql-absent skip semantic not ported — moot: the PgWire legs use tokio-postgres (`pgwire_singleton`, `integration_55_d_cli_vs_pgwire_singleton_insert_race`), so no client-absent path exists to degrade; recorded so nobody re-adds a skip. | low | discard | Rust PgWire client is a declared dependency; every platform runs the assertions, so no missing-client branch remains. |
| 1 | [KNOWN] The mapping table cites three reserved slugs that resolve to no test (`integration_49_structured_error_propagation`, `integration_50_alter_table_rename`, `integration_50_cross_process_fk_freshness`; mapping.md:230,235,340,465,538) — each row documents the slug as intentionally unused for a banner-only section whose assertions map to sub-rows; every other cited target (179 qualified, 112 bare) resolves against `cargo test -p ddb-e2e -- --list` (565 tests). | low | discard | Mapping explicitly labels these as non-executable banner sections and maps their executable subrows to registered tests. |
| 2 | Blake re-raised four cycle-1 settled decisions (PRD still in wip/ with unchecked boxes; §54.D psql-absent skip not ported; no #[serial] markers) plus Bob raised three of the same settled class (serve.rs 2011 lines; rest.rs either-or hedge outside the diff) | high/medium/low | discarded or settled-deferral per the ledger | Cycle 2 decision gate: --ledger-dismiss BLAKE matched none of them because his wording changed, so they were dismissed by gate judgment on issue text plus file and appended to 00171-migrate-shell-scenarios-to-e2e-v1-ledger.json with reasons. Blake is blind to the ledger by construction. |
| 2 | Bob: obsolete DDB_BIN env survives on the linux-e2e step of full-validation.yml; and Bob VERIFY: 14-digit-id consolidation completeness unverified outside the touched modules | low | discarded - contradicts computed facts / answered by gate-run evidence | Cycle 2 decision gate ran both checks itself. rg -n DDB_BIN over the repo excluding dev/local returns zero hits, so the DDB_BIN claim is refuted. rg -n over tests/e2e for the 14-digit shapes shows survivors only in modules PRD 00171 never touched, matching Eve KNOWN 2 and the surgical-change rule; every 00171-authored site named in cycle 1 is consolidated. Bob had no grep in his sandbox and said so. |
| 2 | DDB_BIN cycle-2 dismissal superseded after hidden-directory search correction; Eve input isolation repaired in a fresh native Fable child | low | restored finding; review input corrected | Direct rg of .github/workflows/full-validation.yml finds DDB_BIN at line 61. The old search omitted hidden directories. False discard removed from active ledger, full old ledger archived. Corrected Eve raw response and re-consolidation retained; original findings preserved. No code edits, acceptance or state resume; maintainer decision pending. |
| 2 | Explicit maintainer acceptance of all 11 remaining 00171 findings and waiver of the remaining tail sweep |  | Accept 00171 at 876a153 with deferred exceptions; run fresh finalization and stop before 00172 | I accept 00171 at `876a153` with the remaining assertion gaps, cleanup/documentation findings, and unreproduced macOS flake explicitly deferred. I waive the remaining tail sweep for this PRD. Record those acceptance exceptions, run the fresh finalize gate, mark 00171 done, and stop. No further implementation or review cycles. Explicit reply: yes, that is what I want to do, proceed |

### Doubt Rubric Verdicts

| Rule | Verdict |
|------|---------|
| D1 | pass (codex) / pass (fable) |
| D2 | pass (codex) / pass (fable) |
| D3 | pass (codex) / pass (fable) |
| D4 | pass (codex) / pass (fable) |
| D5 | pass (codex) / pass (fable) |

### Loop Metrics

| Launch phase | Sessions | Wall secs | Model | Cost USD |
|--------------|----------|-----------|-------|----------|
| build | 18 | 30566 | claude-sonnet-5[1m] | 212.54 |
| review | 2 | 2553 | claude-opus-5[1m] | 28.00 |
| **Total** | 20 | 33119 | | 240.54 |

### Implementor Mix

no implementor data

### Deferred to Batch End

| Issue | Severity | Reason |
|-------|----------|--------|
| Multi-process race tests not marked #[serial] as PRD explicitly requires (mitigated by per-test port/tempdir isolation) | medium | Existing reviewed design deliberately uses independent TempDirs/ports and order-independent race results instead of serial markers. Retain parallel execution; record the stale PRD wording. |
| Several newly-ported multi-process race test functions run well past the 50-line guideline (integration_61_b_concurrent_cli_and_graphql_write_lock ≈145 lines, integration_55_d_cli_vs_pgwire_singleton_insert_race ≈152 lines) | low | Keep cohesive multi-process scenario orchestration and HEAD durability checks; shared operations are already helpers. The project Rust instructions have no hard function-size limit, and broad test fragmentation is outside this coverage repair. |
| The mechanical fail-first/tautology finder for this cycle reports "Checked 0 test function(s)" and "replay: skipped (the diff touches no test function)" despite ~60 new/changed e2e test files — the automated tautology net did not fire for this diff | low | Installed mechanical checks are explicitly Python-only. Extending shared tooling is outside 00171; this cycle used Rust source audits and same-HEAD runtime evidence. |
| [KNOWN] PRD says the §55/§61 race sections are "marked serial"; no serial marker exists (no serial_test, no shared mutex) — design doc lines 92-100 and 289 supersede this with per-test tempdir + unique-port isolation, and every race outcome is order-independent (SINGLETON forces exactly one winner; 61.B requires every write to land), so a serial marker would only slow the suite. | low | Existing reviewed design deliberately uses independent TempDirs/ports and order-independent race results instead of serial markers. Retain parallel execution; record the stale PRD wording. |
| [KNOWN] PRD spawn helper "polls /health"; `ServerGuard::start` (tests/e2e/common.rs:81-102) waits on the stderr "listening on" lines plus a 50 ms sleep, no /health poll — design lines 84-89 reused the existing guard unchanged, the health round trip is pinned by `serve::health_returns_ok`, and the internals are 00188's to change. | low | Design deliberately reuses existing ServerGuard readiness; health_returns_ok tests its lifecycle. Internals are reserved for 00188, outside this test migration. |
| Nightly macOS ConnectionReset in a migrated e2e test, cause unconfirmed. integration_warnings_update::integration_58_a_graphql_update_warnings_empty failed once at c8b72bd in Nightly 34020494801 at tests/e2e/common.rs:186. At the reviewed SHA it passed on all three platforms with no code change, no retry, no skip. Eve traced the readiness path and found it sound for HTTP; cause remains unconfirmed. Alice established that task 15 moved the full e2e suite from nightly-only to every push, so this flake exposure increased as a direct result of this PRD rework, with no resilience added. Severity contested: Blake (blind) 1/4 rates it high; Alice, Bob and Eve rate it low with PRD 00188 named as the durable home. | high | Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md. |
| Section 38b2b link order does not exclude a MAX(id) resolver. reference_raw_id.rs:164 mints alpha, beta, gamma ascending so category_ids[2] is the largest id; :195 links [2],[0],[1] and :215 asserts category_ids[2]. Confirmed by reading. Fix: link the middle id first and assert category_ids[1], which excludes min-id, max-id and last-linked at once. | medium | Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md. |
| Section 38b2b object resolver unpinned on multiple junction rows. The deleted bash asserted BOTH the scalar and the object resolver on three junction rows; the new test queries only category_id, so an object resolver drifting from the scalar row is unpinned. Bears on the PRD No lost semantics success metric. | medium | Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md. |
| The new 73-line test duplicates the seed loop at reference_raw_id.rs:320-338 verbatim apart from table and variable names, and exceeds the 50-line guideline. Confirmed by reading. One seed_labeled_rows helper closes both. | medium | Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md. |
| Section 18d2 pins the hit count but never which row survived. serve.rs:1854-1860 asserts hits.len()==1 and totalCount==1 and never reads hits[0][title], so an eq returning the wrong single row passes. Confirmed by reading. | low | Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md. |
| common::stdout is a noun name for a newly crate-wide action helper that runs ddb and asserts success; it reads as std::io::stdout or the assert_cmd stdout assertion. Rename to run_stdout or capture_stdout and update the three importers. | low | Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md. |
| Redundant predicate in integration_nosql_cli.rs: the contains predicate is strictly implied by the new line-equality check on the same captured output, and the clone exists only to feed it. | low | Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md. |
| Unused DDB_BIN remains at .github/workflows/full-validation.yml:61; the initial zero-hit search skipped hidden directories. The cycle-2 discard is superseded; restore this low-severity cleanup. | low | Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md. |
| Corrected Eve: strengthen the two sibling section 18z8 nonempty-ID assertions with the existing assert_doogat_id helper. | medium | Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md. |
| Corrected Eve: assess redundant seeding sleeps; preserve the known repo-aware ID throughput ceiling. Claimed three-second improvement is unverified; no minting-code change is authorized. | low | Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md. |
| Corrected Eve first-reference-order documentation check ran. data-model.md and indexer.md lack the explicit rule; server.md does not state multi-reference singular scalar/object selection order. Clarify existing behavior without changing policy. | low | Explicit maintainer acceptance on 2026-09-07 at 876a153; remaining tail sweep waived. Deferred and unfixed, not refuted or repaired. Authority: 00171-acceptance-2026-09-07.md. |
