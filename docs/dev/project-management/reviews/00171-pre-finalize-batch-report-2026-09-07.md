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
