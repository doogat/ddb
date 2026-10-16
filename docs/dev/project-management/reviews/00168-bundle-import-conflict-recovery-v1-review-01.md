---
prd: dev/local/prds/wip/00168-bundle-import-conflict-recovery-v1.md
review: 1
date: 2026-08-16
head_sha: e531306978164c21a9b61a8547667bd930dba45b
codex_thread_id: 01a0083e-22b4-7d02-b497-7781a0714a84
reviewers: alice,blake,bob,carl
agents:
  alice: available
  blake: available
  bob: available
  carl: available
---

# Review: 00168-bundle-import-conflict-recovery-v1

Diff range: `35e594e9a693e2d3bf1a6740bbfcdb8f4fba7797..e531306978164c21a9b61a8547667bd930dba45b`

codex_rung_guard: not fired

## Review Summary

Reviewed: 8 completed tasks
PRDs checked: 00168-bundle-import-conflict-recovery-v1

### Agent Status

- Alice (consensus, Claude subagent): ✅ Available
- Blake (blind lens, PRD-only): ✅ Available
- Bob (doubt + de-slop lens, codex): ✅ Available
- Carl (Gemini via copilot backend): ✅ Available

Consensus engine: `legacy` (single Alice subagent). Eve: not dispatched — the codex doubt-roster guard did not fire (no task carries `implementor: "codex"`), and `doubt_reviewer` resolved to `codex`.

### Process notes (fail-loud)

- `gather-context.sh` with no `--since` produced an **empty diff**: the repo is on `master`, and the script's full-review default diffs against `master`. Re-run as `--since 35e594e9…` (the autopilot `work_start_sha`) to get the PRD's real work range. The review below is against that range, not an empty one.
- The mechanical-facts block is **empty of counts this cycle**: `compute_mech_facts.py` is `ast`-based (Python only) and every changed file is Rust/shell/markdown. It was staged as `dev/local/tmp/review-mechfacts-00168-01.md` and referenced from each implementation-aware prompt rather than appended into the context file (the aegis hook blocks shell redirects into `dev/local/`, and the block carried no information to justify a 281-line rewrite). No finding this cycle was eligible for the "contradicts computed facts" discard.
- Both CLI reviewers ran long (codex ~2h05m wall, copilot ~2m24s of model time after a slow start). The first Watcher round exhausted its 30 polls with both pending; a second Watcher round confirmed both completed. Neither reviewer was treated as failed.
- Engram pack generated: `dev/local/tmp/engram-pack-00168-01.md` (1963 tokens).

## Consolidated Findings

34 findings. Severity split: **1 Critical, 6 High, 16 Medium, 11 Low.**

| Consensus | Severity | Issue | File | Task | Found By |
|-----------|----------|-------|------|------|----------|
| [1/4] | 🔴 | The PRD's headline reachability guarantee is defeated by `fetch.prune`: `unbundle_git_objects` fetches `refs/heads/*:refs/remotes/bundle/*` with no `--no-prune`, so the NEXT bundle import deletes the `refs/remotes/bundle/master` that a failed import deliberately kept — the offline-synced commits become unreachable and gc-prunable, exactly the loss the PRD closes. Confirmed live: `git fetch <bundle2>` printed `- [deleted] (none) -> bundle/master`; with `-c fetch.prune=false` the ref survived. `fetch.prune=true` is set globally on this machine. | ddb-core/src/bundle.rs | Phase 1 | BLAKE |
| [1/4] | 🟠 | Routing bundle import through the doogat add/add collision path breaks the documented full-bundle bootstrap: with unrelated histories every conflicting path has `ancestor: None`, so `partition_conflicts` (sync_manager/mod.rs:112) sends NON-doogat files into `resolve_add_add_collision`. Confirmed live: a repo whose `.gitignore` differed died with `error: sync: bundle merge failed: conflict: collision loser at .gitignore (old id .gitignore) could not be rewritten: parse: no frontmatter opening ---`, and every retry fails identically (no recovery path). | ddb-core/src/sync_manager/mod.rs | Phase 1 | BLAKE |
| [1/4] | 🟠 | Same misclassification silently FORKS data: importing a bundle into an unrelated history holding the same doogat id reported `imported: conflicts resolved: 1` (success) while duplicating the document to `ddb/66177401297366.md` with `id: 66177401297366` — not a valid 14-digit YYYYMMDDHHmmss id. Confirmed live; no test covers unrelated-history imports (e2e/integration both use cloned repos that share a merge base). | ddb-core/src/git_ops/merge.rs | Phase 1 | BLAKE |
| [1/4] | 🟠 | The PRD requires "merge → resolve → ref-delete runs inside the 00162 write lease"; it does not. `import_bundle` takes the lock three separate times (`merge_remote`, `commit_merge`, `delete_remote_ref`) with unlocked windows between, and `unbundle_git_objects` writes git objects and `refs/remotes/bundle/*` fully outside the lock. | ddb-core/src/bundle.rs | Phase 1 | BLAKE |
| [1/4] | 🟠 | `merge_remote` releases the write lock before conflict resolution, and ref deletion acquires another lock later; a concurrent same-path write can retain the same conflict set and be overwritten by stale resolved blobs, violating the PRD's single-lease requirement. | ddb-core/src/bundle.rs:120 | 3 | BOB |
| [1/4] | 🟠 | "On any error ... return a `Sync` error" is only half implemented: `merge_bundle_and_resolve` maps `apply_merge_result` errors to `Sync` but propagates `merge_remote` errors raw. Confirmed live — a bundle exported from a `main`-branch repo yields `error: not found: refs/remotes/bundle/master` (a `NotFound`, with no hint about the branch name), and a write-lock timeout would surface as `Conflict`. | ddb-core/src/bundle.rs | Phase 1 | BLAKE |
| [1/4] | 🟠 | User-controlled remote name `bundle` disables the unrelated-history guard for ordinary `ddb sync bundle master`; use an explicit bundle-import merge mode instead of inferring policy from the name. | ddb-core/src/git_ops/merge.rs:390 | 3 | BOB |
| [2/4] | 🟡 | `if remote != "bundle"` puts caller-specific merge policy in the git adapter via a magic string: any real remote named `bundle` (`ddb sync bundle master`) silently skips the unrelated-histories guard and unions two unrelated trees. Make the policy explicit (an `allow_unrelated` param / dedicated entry point, or at minimum a shared `pub(crate) const BUNDLE_REMOTE`). | ddb-core/src/git_ops/merge.rs:384-392 | 3 | ALICE, BLAKE |
| [2/4] | 🟡 | Invariant docs overclaim closure: invariants.md I1 now says "Every git write now holds the repo write lock; this invariant HOLDS with no remaining gap" (AGENTS.md and git-ops.md echo it), but the same import flow still runs `git bundle unbundle` + `git fetch ... refs/heads/*:refs/remotes/bundle/*` as unlocked subprocesses (bundle.rs:81-105). Narrow the wording to commit/merge/ref-delete paths and keep the fetch leg named. | docs/src/technical/invariants.md:596-602 | 8 | ALICE, BLAKE |
| [2/4] | 🟡 | FFI parity gap: `DoogatDriver::import_bundle` returns `Result<(), DdbError>` and discards the report, so the "true conflict count" the PRD promises is CLI-only while Transport Impact calls FFI `Guaranteed`. Pre-existing signature, but the count only became meaningful with this change. | ddb-core/src/ffi/driver.rs:262-268 | general | ALICE, BLAKE |
| [1/4] | 🟡 | The retained guard is unpinned: every `merge_remote` test uses `setup_two_repos` clones with a shared base, so deleting `self.repo.merge_base(our_commit.id(), their_commit.id())?` at merge.rs:391 leaves the full suite green. Commit 872f2b5 ("keep the merge-base guard for non-bundle remotes") shipped without the regression test the project's testing rule mandates — add one asserting an `origin` merge of unrelated histories still errors. | ddb-core/src/git_ops/merge.rs:390 | 3 | ALICE |
| [1/4] | 🟡 | ~75 lines of setup are duplicated verbatim between `conflicting_bundle_import_leaves_bundle_ref_reachable_on_merge_failure` (:566-661) and `conflicting_bundle_import_leaves_repo_clean_on_merge_failure` (:669-754); they differ only in the final two asserts. Extract one `fn import_with_unresolvable_collision() -> (TempDir, GitRepo, Result<SyncReport>)` helper. | ddb-core/src/bundle.rs:566-754 | 4 | ALICE |
| [1/4] | 🟡 | Size limits crossed by this diff: bundle.rs went 569 → 828 lines (800 max), and the three new test fns are 96 / 86 / 84 lines (50 max). The dedupe above (~75 lines) plus moving `mod tests` to `bundle/tests.rs` (the `git_ops/tests.rs` precedent) fixes both. | ddb-core/src/bundle.rs | 4 | ALICE |
| [1/4] | 🟡 | The three new bundle tests exceed 50 lines, duplicate nearly identical repository setup, and push `bundle.rs` beyond 800 lines; extract a shared fixture/failure helper and move tests into a separate test module. | ddb-core/src/bundle.rs:475 | 4 | BOB |
| [1/4] | 🟡 | `map_err(\|e\| DoogatError::Sync(...))` collapses every resolver error, including the retryable `DoogatError::Conflict` that `with_write_lock` returns on lock-acquire timeout and that `commit_merge`'s divergence guard returns; a lock timeout inside `merge_remote` (line 120, unwrapped) still surfaces as `Conflict`, so the same failure reports two different classes depending on where it lands. Preserve `Conflict` and wrap only the rest. | ddb-core/src/bundle.rs:121-123 | 3 | ALICE |
| [1/4] | 🟡 | `merge_remote` errors escape through `?` before the `Sync` mapping, so merge-engine failures violate the promised `DoogatError::Sync` contract. | ddb-core/src/bundle.rs:120 | 3 | BOB |
| [1/4] | 🟡 | "A resolution failure here leaves the repo exactly as it was before this call" is false on the clean-merge branch: `perform_normal_merge` commits the merge and force-checks-out the worktree *before* `apply_merge_result` → `validate_clean_merge_or_fallback` runs, and that can fail. No data is lost (the bundle ref survives), but HEAD has moved — the same overclaim is repeated in bundle.md § Conflict Recovery. | ddb-core/src/bundle.rs:110-114 | 8 | ALICE |
| [1/4] | 🟡 | The new trait method is missing from every trait-surface listing: walkthrough.md:341, git-ops.md:13, and the trait header at traits.rs:366 all still read "(add, fetch, push)"; git-ops.md § Merge (:104-116) also documents the merge algorithm with no mention of the merge-base guard or its bundle exception. | docs/src/technical/walkthrough.md:341 | 8 | ALICE |
| [1/4] | 🟡 | `validate_clean_merge_or_fallback` — named by the PRD at `:538-544` as a site that must "stop reporting false success" — is untouched and still `return Ok(0)` when `commit_parent_count < 2` (mod.rs:561). | ddb-core/src/sync_manager/mod.rs | Phase 1 | BLAKE |
| [1/4] | 🟡 | The PRD lists `resolve_post_merge_conflicts` as a required export of the sync_manager module ("honest count from real merge state"); the implementation deleted the `pub fn` instead, with no deprecation note or CHANGELOG mention of the removed library API. `GitRemote` also gained a required `delete_remote_ref` method with no default impl, breaking external implementors. | ddb-core/src/sync_manager/mod.rs | Phase 1 | BLAKE |
| [1/4] | 🟡 | Docs contradict themselves after the fix: `docs/src/technical/sync.md:56` still states "Bundle import's `git merge` subprocess is the one merge path still outside the lock (tracked by 00168)", and `CHANGELOG.md:16` repeats it in the same `[Unreleased]` section that line 26 refutes. sync.md is the exact file the PRD's Phase 2 doc task names. | docs/src/technical/sync.md | Phase 2 | BLAKE |
| [1/4] | 🟡 | The PRD promises CLI + FFI conformance, but the added integration/e2e scenarios exercise only CLI; add a conflicted import through `DoogatDriver::import_bundle` and verify merged data/error propagation. | ddb-core/src/ffi/driver.rs:262 | general | BOB |
| [1/4] | 🟡 | Simplification: redundant `repo` param. Current: `fn merge_bundle_and_resolve(repo: &impl GitBackend, sync_mgr: &mut SyncManager<impl GitBackend>, ...)`. Replacement: `<G: GitBackend>(sync_mgr: &mut SyncManager<G>, ...)` and use `sync_mgr.repo.merge_remote`. | ddb-core/src/bundle.rs:114 | 3 | CARL |
| [1/4] | ⚪ | Both failure tests assert only `matches!(result, Err(DoogatError::Sync(_)))`, but every bundle-import failure is a `Sync` (unbundle and fetch failures included), so the assertion cannot show the *merge* failed. Assert the message contains "bundle merge failed". | ddb-core/src/bundle.rs:642-645 | 4 | ALICE |
| [1/4] | ⚪ | `!repo2.repo.index().unwrap().has_conflicts()` reads the process-cached index that the in-memory merge never writes, so it cannot fail; re-open the repo (`git2::Repository::open(dir2.path())`) to assert against the on-disk index. | ddb-core/src/bundle.rs:750-753 | 4 | ALICE |
| [1/4] | ⚪ | Library-API break not recorded: `pub fn SyncManager::resolve_post_merge_conflicts` was removed and a required method added to the public `GitRemote` trait; CHANGELOG has only the behavioral Fixed entry, no Changed/Removed line. | CHANGELOG.md:26 | 8 | ALICE |
| [1/4] | ⚪ | PRD success metric "asserted by a test that a locale/wording change in git output does not flip the detection" has no test. It is structurally satisfied (no CLI git remains on the path), and the MERGE_HEAD assert is the only proxy guarding reintroduction — worth stating in the design-reconciliation note next to the `git merge --abort` one. | N/A | 4 | ALICE |
| [1/4] | ⚪ | Phase 0 acceptance ("a unit test asserts detection is unaffected by git output wording/locale") has no corresponding test; nothing pins that stderr parsing cannot be reintroduced, only an incidental `.git/MERGE_HEAD` absence assertion. | ddb-core/src/bundle.rs | Phase 0 | BLAKE |
| [1/4] | ⚪ | No `git merge --abort` / index-reset path exists anywhere, so the PRD's explicit abort branch is unimplemented and untestable; the code substitutes "the merge is in-memory" reasoning (permitted by the PRD's Risks mitigation, and empirically clean — no MERGE_HEAD, clean status, HEAD unmoved after a failed import). | ddb-core/src/bundle.rs | Phase 1 | BLAKE |
| [1/4] | ⚪ | Only `refs/remotes/bundle/master` is deleted, but the fetch creates `refs/remotes/bundle/*` for every branch in the bundle; confirmed a stale `refs/remotes/bundle/main` persists forever, pinning objects and polluting the ref namespace. | ddb-core/src/bundle.rs | Phase 1 | BLAKE |
| [1/4] | ⚪ | The shipped proof-of-work walkthrough demonstrates the headline scenario producing mangled content: the merged body renders as `laptdesktopp body` (character-interleaved CRDT output of "laptop body" and "desktop body"). Pre-existing resolver behavior, but this PRD is what puts bundle imports through it. | dev/local/walkthroughs/00168-bundle-import-conflict-recovery.md | Phase 2 | BLAKE |
| [1/4] | ⚪ | `SyncReport.commits_transferred` for bundle import changed meaning (hardcoded 0 before, now 0/1 from the merge result) — a public report-field behavior change with no CHANGELOG or docs note. | ddb-core/src/bundle.rs | Phase 1 | BLAKE |
| [1/4] | ⚪ | Pre-existing, surfaced by this PRD's walkthrough: `GitRepo::open` deletes every `.crdt/temp/*` file, so the frontmatter CRDT state conflict resolution writes (sync_manager/mod.rs:368) is wiped by the next process. Not introduced here; flagging, not asking for a fix in this PRD. | ddb-core/src/git_ops/mod.rs:182 | general | ALICE |
| [1/4] | ⚪ | Pre-existing size violation touched by this diff: git_ops/mod.rs is 849 lines (845 before, +4 here) against the 800 max. | ddb-core/src/git_ops/mod.rs | 1 | ALICE |
| [1/4] | ⚪ | Cannot statically verify: `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci` pass. | N/A | general | BOB |

## Alice

Consensus lens, implementation-aware. 15 findings (0 Critical, 0 High, 8 Medium, 7 Low). Alice ran real verification: `cargo test -p ddb-core --lib bundle::tests` 8 passed, `delete_remote_ref` 2 passed, `sync_manager::` 37 passed, the new e2e test 1 passed, `cargo clippy --workspace --all-targets` clean. She started `cargo test-ci` but did not see it finish and explicitly declined to claim it (the orchestrator ran it to completion — see the `Tests:` line).

Her per-rule verdicts:

```
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
R13: fail
```

## Blake

Blind lens — PRD-only prompt, no diff, no file list, no design doc. He located the code himself and, notably, ran **live CLI experiments in `/tmp`** rather than reading alone. That is what produced the cycle's only Critical and three of its Highs; each of those carries a reproduced command and its output. 16 findings (1 Critical, 4 High, 6 Medium, 5 Low).

His per-rule verdicts:

```
B1: fail
B2: pass
B3: fail
B4: pass
B5: fail
B6: fail
B7: pass
B8: pass
B9: pass
B10: pass
B11: pass
B12: fail
B13: pass
B14: fail
B15: fail
B16: fail
B17: pass
B18: pass
B19: pass
```

Blake's B6 (`no new functionality beyond the PRD`) fail is the `if remote != "bundle"` guard exemption, which is also his 🟡 and Alice's 🟡 — the one place both lenses converged on the same undeclared behavior change.

## Bob

Doubt + de-slop lens (codex, static-only sandbox). 6 findings (0 Critical, 2 High, 3 Medium, 1 Low).

Bucketed residual findings:

```
FIX:
- Bundle merge, resolution, and ref deletion do not share one write lease — ddb-core/src/bundle.rs:120 — add an outer repo-write transaction spanning the required sequence, relying on existing reentrancy for nested merge/commit/ref operations
- Remote name `bundle` bypasses the unrelated-history safety guard for user-triggered sync — ddb-core/src/git_ops/merge.rs:390 — preserve the guard in ordinary `merge_remote` and expose an explicit bundle-import-only merge mode
- Merge-engine errors are not normalized to `DoogatError::Sync` — ddb-core/src/bundle.rs:120 — map `merge_remote` failures before applying `?` and add a merge-phase error regression
- FFI conflict-recovery conformance is missing — ddb-core/src/ffi/driver.rs:262 — drive the conflicted-bundle workflow through `DoogatDriver::import_bundle` and verify recovery plus failure propagation
- New tests violate function/file-size limits and duplicate setup — ddb-core/src/bundle.rs:475 — extract a reusable two-node bundle fixture and move bundle tests into a separate module
VERIFY:
- Tier 1 validation was not executable in this sandbox — run `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`
KNOWN:
- `git bundle unbundle`/`git fetch` still prepare objects and the bundle tracking ref outside the write lock — the supplied design explicitly defers this side-ref-only preparation leg as outside PRD 00168's master merge→resolve→ref-delete scope
```

His per-rule verdicts:

```
R1: fail
R2: pass
R3: pass
R4: pass
R6: pass
R7: fail
R8: pass
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

Bob's `VERIFY` item is discharged by the orchestrator: `cargo test-ci` ran to completion (exit 0) and `cargo clippy --workspace --all-targets` was clean in Alice's run.

## Carl

Gemini via the copilot backend (`gemini-3.1-pro-preview`). 1 finding (0 Critical, 0 High, 1 Medium) — a signature simplification. No frontend surface in this diff, so he reviewed as a generalist, correctly declining to invent frontend findings.

His per-rule verdicts:

```
R1: pass
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
R13: pass
```

Carl's all-pass R12/R13 contradict Alice's and Bob's measured `bundle.rs` = 828 lines and 96/86/84-line test functions. Treated as a shallow pass, not as counter-evidence: Alice and Bob both cite specific line counts, Carl cites none.

Verdict: 34 findings
Tests: 1729 passed, 0 failed, 0 skipped
