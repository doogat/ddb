---
prd: dev/local/prds/wip/00168-bundle-import-conflict-recovery-v1.md
review: 2
date: 2026-08-16
head_sha: 6bb16174fd533b94db0f45016019aa4626b5a16e
codex_thread_id: 01a0083e-22b4-7d02-b497-7781a0714a84
reviewers: alice,blake,bob,carl
agents:
  alice: available
  blake: available
  bob: available
  carl: available
---

# Review: 00168-bundle-import-conflict-recovery-v1

Diff range: `e531306978164c21a9b61a8547667bd930dba45b..6bb16174fd533b94db0f45016019aa4626b5a16e`

codex_rung_guard: not fired

## Review Summary

Reviewed: 15 completed tasks (8 original plan + 7 cycle-1 rework `[D1-n]` tasks)
PRDs checked: 00168-bundle-import-conflict-recovery-v1

Incremental review of the cycle-1 rework only. The full implementation was reviewed
in cycle 1 (`35e594e..e531306`); this cycle scopes to the 15 rework commits.

### Agent Status

- Alice (consensus, Claude subagent): ✅ Available
- Blake (blind lens, PRD-only): ✅ Available
- Bob (doubt + de-slop lens, codex, resumed thread `01a0083e`): ✅ Available
- Carl (Gemini via copilot backend): ✅ Available

Consensus engine: `legacy` (single Alice subagent). Eve: not dispatched — the codex
doubt-roster guard did not fire (no task carries `implementor: "codex"`; the
implementors this PRD used were `claude` and `qwen`), and `doubt_reviewer` resolved
to `codex`.

### Process notes (fail-loud)

- The mechanical-facts block again carries **no `ast` function counts**:
  `compute_mech_facts.py` is Python-only and all 17 changed files are Rust/Markdown.
  Rather than ship an information-free block, the orchestrator measured the changed
  Rust files with `wc -l` at HEAD and appended those as the authoritative counts to
  `review-context-00168-02.md`. Carl and Alice both used them for R12/R13.
- Engram pack generated: `dev/local/tmp/engram-pack-00168-02.md` (3310 tokens).
- Both CLI reviewers finished fast this cycle (Carl ~4m24s of model time; Bob resumed
  his cycle-1 codex thread rather than re-reviewing from zero). The Watcher confirmed
  both outputs complete on its 4th poll. Neither was treated as failed.
- Alice's `rustfmt --check` was blocked by warden, so **formatting is unverified** this
  cycle. Per the project's standing note, workspace `cargo fmt` is not run here anyway.
- Blake and the orchestrator each ran live CLI experiments in `/tmp` (never the repo
  root). Those reproductions are what produced this cycle's Critical and two of its
  Highs.

## Consolidated Findings

31 findings. Severity split: **1 Critical, 3 High, 16 Medium, 11 Low.**

| Consensus | Severity | Issue | File | Task | Found By |
|-----------|----------|-------|------|------|----------|
| [1/4] | 🔴 | Import merges hardcoded `refs/remotes/bundle/master`, not the refs this bundle fetched: with a stale bundle ref present, importing a different bundle reports `imported: conflicts resolved: 0` while importing the OLD bundle's data and leaving the requested bundle's commits unreachable (reproduced) | ddb-core/src/bundle/mod.rs | Phase 1 | BLAKE |
| [1/4] | 🟠 | `delete_all_bundle_refs` deletes the whole `refs/remotes/bundle/*` namespace including refs a previous FAILED import deliberately kept, so a later successful import orphans that data — defeats the PRD reachability metric (reproduced, `git fsck` unreachable) | ddb-core/src/bundle/mod.rs | Phase 1 | BLAKE |
| [1/4] | 🟠 | Routing import through libgit2 `checkout_head(force)` dropped CLI git's "local changes would be overwritten" refusal: an uncommitted edit to a doogat is silently destroyed by a fast-forward import that exits 0 (reproduced; old path errored and preserved it) | ddb-core/src/git_ops/merge.rs | Phase 1 | BLAKE |
| [1/4] | 🟠 | Fetch refspec lacks `+`, so a bundle ref kept by a failed import blocks every later import of a diverged bundle with `! [rejected] master -> bundle/master (non-fast-forward)`; no ddb command clears it and docs never mention it (reproduced) | ddb-core/src/bundle/mod.rs | Phase 1 | BLAKE |
| [1/4] | 🟡 | `bundle.md` import-steps list still documents pre-rework behavior: step 4 says `git fetch` with no mention of the now load-bearing `--no-prune`, step 8 says "Delete `refs/remotes/bundle/master`" though `delete_all_bundle_refs` now clears the whole namespace, and step 5 names `GitRepo::merge_remote` though the call is `merge_remote_allowing_unrelated` — task [D1-5] edited this same file two lines below without updating them | docs/src/technical/bundle.md | 9 | ALICE |
| [1/4] | 🟡 | `derive_content_id` renders the hash as `year = 2000 + (n / (86_400 * 28 * 12)) % 100`, i.e. uniform over 2000-2099, so ~73% of reassigned collision ids are future-dated calendar stamps; `invariants.md` I3 still states this path is "repo-aware by construction (never `exists = |_| false`, never a future-dated timestamp)", and no new test bounds the year | ddb-core/src/id_minting.rs | 10 | ALICE |
| [1/4] | 🟡 | `is_doogat_path` is introduced at `sync_manager/mod.rs:96` but the two literal copies of the identical predicate at `:645` and `:650` were left inline; switching them to the new helper is behavior-preserving and removes in-file duplication the diff created | ddb-core/src/sync_manager/mod.rs | 10 | ALICE |
| [1/4] | 🟡 | `delete_all_bundle_refs` adds a third CLI `git` subprocess (`git for-each-ref`) with its own stderr error-mapping block to the path this PRD exists to move onto libgit2; a `GitRemote` ref-listing method (following the `delete_remote_ref` precedent this PRD just set) or `references_glob` — which the test itself uses — keeps the cleanup in-process and drops ~15 lines | ddb-core/src/bundle/mod.rs | 9 | ALICE |
| [1/4] | 🟡 | Spec requires merge → resolve → ref-delete inside ONE 00162 write lease; implementation takes three separate leases, so HEAD can move mid-sequence and the divergence guard only compares conflict-path SETS, not content | ddb-core/src/bundle/mod.rs | Phase 1 | BLAKE |
| [1/4] | 🟡 | Spec-mandated `git merge --abort` (or index reset) on error is absent entirely, and the clean-merge leg admits HEAD stays moved when post-merge validation fails, so "repo is left clean on error" holds only for the conflicted leg | ddb-core/src/bundle/mod.rs | Phase 1 | BLAKE |
| [1/4] | 🟡 | Spec's sync_manager export `resolve_post_merge_conflicts` was deleted outright with no deprecation instead of made honest; `merge_bundle_and_resolve` is private, not an export as the module contract states | ddb-core/src/sync_manager/mod.rs | general | BLAKE |
| [1/4] | 🟡 | Scope creep outside the PRD's two named modules: collision-loser ID derivation re-specified to a calendar-shaped stamp, changing COMMITTED file paths with an explicit cross-version interop break and no migration | ddb-core/src/id_minting.rs | general | BLAKE |
| [1/4] | 🟡 | Scope creep: `GitMerge::merge_remote_allowing_unrelated` and `GitRemote::delete_remote_ref` added as required trait methods (breaking every external `GitBackend`), plus conflict partitioning changed to path-shape classification — none specified by the PRD | ddb-core/src/traits.rs | general | BLAKE |
| [1/4] | 🟡 | Phase 0 acceptance test (detection unaffected by git output wording/locale) does not exist; docs state it is "satisfied structurally, not by a dedicated test", so nothing fails loudly if stderr parsing returns | docs/src/technical/bundle.md | Phase 0 | BLAKE |
| [1/4] | 🟡 | `unbundle_git_objects` runs `git bundle unbundle` + `git fetch` (which writes `refs/remotes/bundle/*`) outside the repo write lock, violating the project's own "every git write holds the lock, bundle import included" P0 invariant | ddb-core/src/bundle/mod.rs | Phase 1 | BLAKE |
| [1/4] | 🟡 | Lock documentation still overclaims: I1 remains `HOLDS` and git-ops says every write is locked despite the acknowledged unlocked fetch leg; git-ops also names `merge_remote` instead of `merge_remote_allowing_unrelated` | docs/src/technical/invariants.md:8 | 13 | BOB |
| [1/4] | 🟡 | Splitting `bundle.rs` left stale module paths in bundle.md, invariants.md, architecture/overview.md, and walkthrough.md | docs/src/technical/bundle.md:2 | 14 | BOB |
| [1/4] | 🟡 | The two new unrelated-history integration tests duplicate repository, bundle, registration, manager, and index setup; extract a shared two-node fixture used by both tests | ddb-core/src/bundle/tests.rs:563 | 10 | BOB |
| [1/4] | 🟡 | `is_calendar_shaped` manually reimplements incomplete calendar validation and accepts dates such as February 31; replace it with the existing chrono parser | ddb-core/src/types/doogat.rs:19 | 10 | BOB |
| [1/4] | 🟡 | Replace `read_to_string` + `write` with `std::fs::copy(entry.path(), &dest)` in `import_node_registrations` for simpler and more efficient copying | ddb-core/src/bundle/mod.rs | general | CARL |
| [1/4] | ⚪ | A successful import now deletes bundle refs for branches it never merges — only `bundle/master` is merged — so a full bundle's non-master heads are unbundled into the object store and then silently discarded with no warning; `successful_import_deletes_every_bundle_ref_not_just_master` creates `feature` at the same commit as `master`, so it cannot observe the orphaning | ddb-core/src/bundle/mod.rs | 9 | ALICE |
| [1/4] | ⚪ | Stale source paths after commit a3bc99e deleted `ddb-core/src/bundle.rs`: `bundle.md:3`, `overview.md:154`, and `invariants.md:14` — the last was rewritten in this very diff | docs/src/technical/invariants.md | 13 | ALICE |
| [1/4] | ⚪ | `DoogatId::is_calendar_shaped` is `#[cfg(test)]`, so the calendar-shape guarantee this cycle adds is enforced only by tests; production write paths still validate ids with the weaker 14-digit `is_valid_shape` | ddb-core/src/types/doogat.rs | 10 | ALICE |
| [1/4] | ⚪ | After dropping the `repo` parameter, `import_bundle` unbundles/fetches into `repo` but merges from `sync_mgr.repo` — two independently-typed generic parameters; the sole caller passes the same repo, so no live bug, but the signature no longer enforces the coupling the function now assumes | ddb-core/src/bundle/mod.rs | 15 | ALICE |
| [1/4] | ⚪ | `unrelated_history_import_resolves_conflicting_non_doogat_file` asserts `gitignore.contains("target/") \|\| gitignore.contains("node_modules/")`, which also passes if both sides are interleaved into one mangled file; it never asserts absence of conflict markers or that exactly one side survived | ddb-core/src/bundle/tests.rs | 10 | ALICE |
| [1/4] | ⚪ | With `--no-prune`, a `bundle/master` kept by a failed import is retained, and `merge_bundle_and_resolve` always merges `refs/remotes/bundle/master` regardless of what the current bundle carried — so importing a later bundle with no `master` can merge the *previous* bundle's data and report success; `kept_bundle_ref_survives_a_later_import_under_fetch_prune` builds exactly this setup and ignores the second import's result | ddb-core/src/bundle/mod.rs | 9 | ALICE |
| [1/4] | ⚪ | `kept_bundle_ref_survives_a_later_import_under_fetch_prune` discards the second import's result (`let _ =`) and passes only because that import also fails; it cannot catch the case where a later SUCCESS deletes the kept ref | ddb-core/src/bundle/tests.rs | Phase 2 | BLAKE |
| [1/4] | ⚪ | Bundle manifest is parsed into `_manifest` and discarded: `format_version` is never checked (a future v2 bundle imports as v1) and `target_node` is never matched against the local node | ddb-core/src/bundle/mod.rs | general | BLAKE |
| [1/4] | ⚪ | Doc drift: docs cite `ddb-core/src/bundle.rs` (now `bundle/mod.rs`) and say import deletes `refs/remotes/bundle/master`, while the code deletes the whole namespace; the required sync.md recovery section is one clause, the real content lives in bundle.md | docs/src/technical/bundle.md | Phase 2 | BLAKE |
| [1/4] | ⚪ | The PRD's own proof artifact ships a mangled merge body (`laptdesktopp body`) from character-level CRDT interleaving, presented as the successful conflict-recovery outcome | dev/local/walkthroughs/00168-bundle-import-conflict-recovery.md | Phase 2 | BLAKE |
| [1/4] | ⚪ | `import_node_registrations` writes `.nodes/*.toml` into the worktree uncommitted (check-then-act on `dest.exists()`), while `list_nodes` reads the HEAD tree — imported registrations are invisible yet the import reports success (pre-existing) | ddb-core/src/bundle/mod.rs | general | BLAKE |

### Auto-dismissed (ledger)

- [BLAKE] 🟡 FFI is marked Guaranteed but `DoogatDriver::import_bundle` returns `Result<(), DdbError>` and discards the SyncReport, so the PRD's headline "true conflict count" is unavailable on that transport | File: ddb-core/src/ffi/driver.rs — Pre-existing FFI signature, explicitly out of scope in the 00168 design doc. Widening the FFI return type is a public-interface change requiring the compatibility/deprecation review AGENTS.md mandates; a separate PRD, not a rework item. Real gap, tracked at batch end.

Blake's three other re-raises of settled items (the one-write-lease requirement, the
unlocked `unbundle_git_objects` leg, and the mangled walkthrough body) were NOT
auto-dismissed by the matcher — it matches on issue text plus file, and Blake worded
them differently and attributed two of them to `bundle/mod.rs` rather than the ledger's
`bundle.rs`. They are listed in the table above but are settled: ledger entries 1 and 6.

## The cycle-1 rework did land

Recorded so the un-park does not re-do finished work. Alice verified each in code and
the orchestrator confirmed the test evidence:

- The cycle-1 **CRITICAL** (`fetch.prune` deleting the kept bundle ref) is fixed by
  `--no-prune` and pinned by `kept_bundle_ref_survives_a_later_import_under_fetch_prune`.
- Both cycle-1 **HIGHs** on unrelated-history misclassification are fixed —
  `is_doogat_path` gating in `partition_conflicts`, plus calendar-valid id derivation —
  each with tests.
- The raw-error escape is fixed: both merge calls route through `bundle_merge_error`.
- The `remote != "bundle"` magic string is gone, replaced by
  `merge_remote_allowing_unrelated`, with three pinning tests (deleting the guard now
  fails two of them).
- The weak asserts, duplicated setup, redundant `repo` param, trait-surface listings,
  API-break CHANGELOG entries, and the invariants/sync.md overclaims are all closed.

**Every cycle-2 Critical and High is a NEW defect introduced by, or newly exposed by,
the cycle-1 rework** — not an unfixed cycle-1 item. Three of the four trace to the same
root cause: keeping `refs/remotes/bundle/*` alive across failures (the cycle-1 CRITICAL
fix) turned a previously-transient ref namespace into durable state, and neither the
fetch, the merge target, nor the cleanup was re-designed for a namespace that now
persists.

## Alice

Consensus lens, implementation-aware. 10 findings (0 Critical, 0 High, 4 Medium, 6 Low).

She ran, foreground and watched to completion: `cargo clippy --workspace --all-targets`
(clean), `cargo test -p ddb-core --lib bundle::` (15 passed), `... sync_manager::`
(40 passed), `... id_minting::` (13 passed), `... git_ops::tests::merge_remote`
(3 passed), and `cargo build -p ddb-cli` + `cargo test -p ddb-e2e` (449 passed) as the
CLAUDE.md deletion safety net, since this diff deletes `ddb-core/src/bundle.rs`. She did
not run `cargo test-ci`, the sql_engine tier, `integration.sh`/`.ps1`, or the walkthrough,
and said so.

She marked R12/R13 pass on the basis that every *production* function in the diff is
under 50 lines and `bundle/mod.rs` is 413 — the over-limit items are test functions and
test files, closed by ledger entries 5 and 9.

Her per-rule verdicts:

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

## Blake

Blind lens — PRD-only prompt, no diff, no file list, no design doc, no review history.
17 findings (1 Critical, 3 High, 8 Medium, 5 Low), the most severe set this cycle.

As in cycle 1, his value came from **live reproduction** rather than reading: he built
throwaway repos in `/tmp/blake168a` and drove real `ddb` bundle imports through them.
All three of his top findings carry a reproduced command and its observed output:

1. Importing a sidecar-branch bundle with a stale `bundle/master` present printed
   `imported: conflicts resolved: 0`, exit 0, wrote the **old** bundle's doogat into
   `ddb/`, and left the requested bundle's commits unreachable per `git fsck`.
2. A failed `main`-branch import kept `refs/remotes/bundle/main`; the next successful
   import deleted it, orphaning commit `a2e30fc`.
3. An uncommitted hand edit to `ddb/<id>.md` was silently destroyed by a fast-forward
   import that exited 0. He then verified the *replaced* CLI path refuses exactly this
   (`error: Your local changes … would be overwritten by merge`, exit 1) — so it is a
   regression against the PRD's own "no regression" success metric.

His per-rule verdicts:

```
B1: fail
B2: fail
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
B13: fail
B14: fail
B15: fail
B16: pass
B17: pass
B18: pass
B19: pass
```

## Bob

Doubt + de-slop lens (codex, static-only sandbox), resumed on his cycle-1 thread
`01a0083e-22b4-7d02-b497-7781a0714a84`. 4 findings (0 Critical, 0 High, 4 Medium, 0 Low).

Resuming paid off: he did not re-file any of his five cycle-1 FIX items, correctly
treating the settled ones as settled and the reworked ones as closed. His `VERIFY`
bucket is empty this cycle because the orchestrator's verification block was supplied
in the prompt.

Bucketed residual findings:

```
FIX:
- Lock docs still claim full coverage while documenting an unlocked fetch gap, and git-ops names the obsolete merge entry point — docs/src/technical/invariants.md:8 — mark I1 PARTIAL and qualify git-ops to commit/merge/ref-delete paths using `merge_remote_allowing_unrelated`
- Bundle module split left stale `bundle.rs` references — docs/src/technical/bundle.md:2 — update module-source and architecture references to `bundle/mod.rs` and `bundle/tests.rs` where applicable
- Unrelated-history tests duplicate substantial setup — ddb-core/src/bundle/tests.rs:563 — extract one shared unrelated-history bundle fixture for the success and collision cases
- Calendar validation duplicates standard functionality incompletely — ddb-core/src/types/doogat.rs:19 — implement the test helper with `chrono::NaiveDateTime::parse_from_str`
VERIFY:
- (none)
KNOWN:
- (none)
```

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
R12: fail
R13: pass
D1: pass
D2: pass
D3: pass
D4: pass
D5: pass
```

Bob's R12 fail is the settled 84-line test-function holdout (ledger entry 9); he did not
re-file it as a finding, only as a verdict. Alice passed the same rule by scoping it to
production functions. The disagreement is bookkeeping, not substance — both agree on the
underlying line counts.

## Carl

Gemini via the copilot backend (`gemini-3.1-pro-preview`). 1 finding (0 Critical,
0 High, 1 Medium) — a simplification. No frontend surface in this diff, so he reviewed
as a generalist and again correctly declined to invent frontend findings.

Unlike cycle 1, his R12/R13 pass is now backed by the mechanical-facts block he was
pointed at, rather than by impression.

**Orchestrator note on Carl's finding:** it does not survive verification as in-scope.
`import_node_registrations` is byte-identical to its pre-PRD form
(`git show 35e594e:ddb-core/src/bundle.rs` lines 141-157 vs `bundle/mod.rs:187-204`);
it appears in the diff only because commit a3bc99e renamed the file. Under the
surgical-changes rule it is untouched pre-existing code. The suggestion is sound in
itself — `fs::copy` is simpler, avoids the wasted read when `dest` exists, and drops
the UTF-8 requirement — but it belongs to a separate change, not to 00168.

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

## Orchestrator verification

Independent checks run at HEAD `6bb1617`, recorded because two of them change how a
finding should be read.

**The CRITICAL and the fetch-rejection HIGH share one root cause, and the obvious fix
for one reintroduces the cycle-1 CRITICAL.** Probe in `/tmp/ddb-ffprobe-a`, git 2.55.0:

```
=== fetch bundle TWO (unrelated history, same branch name) ===
 ! [rejected] master     -> bundle/master  (non-fast-forward)
exit=1

=== a bundle with NO master branch at all ===
 * [new branch]      main       -> bundle/main
exit=0
refs present:
  refs/remotes/bundle/main   c72362ca…
  refs/remotes/bundle/master 45d7c509…   <- stale, from the earlier bundle
```

The refspec `refs/heads/*:refs/remotes/bundle/*` (`bundle/mod.rs:97`) has no leading
`+`, so it is not forced, and `unbundle_git_objects` returns `Err(Sync(...))` on a
non-zero fetch (`:101-106`). `delete_all_bundle_refs` runs only after a successful merge
(`:223-227`). Together: once any import fails, the kept `bundle/master` permanently
blocks every later import of a diverged peer's bundle, with no `ddb` command to clear
it — and if a later bundle carries no `master`, the hardcoded merge target picks up the
**stale** ref and reports success. Both confirmed.

Adding `+` to the refspec is NOT the fix: forcing would overwrite the ref a failed
import kept, which is precisely the cycle-1 CRITICAL. The fix has to make the merge
target and the cleanup scope follow *this* import's fetched refs — per-import ref
namespacing, or recording the delivered ref and merging/deleting only that one.

**Alice's future-dated-id finding is confirmed against the P0 invariant text.**
`derive_content_id` (`id_minting.rs:85`) computes
`year = 2000 + (n / (86_400 * 28 * 12)) % 100`, uniform over 2000-2099.
`invariants.md:32` says of this exact path: "never `exists = |_| false`, **never a
future-dated timestamp**", and AGENTS.md I3 states "No mint path may … allocate
future-dated timestamps." The code now contradicts both. Note the day field is capped
at 28, so `is_calendar_shaped`'s Feb-31 looseness (Bob's finding) cannot currently mask
a real defect — it would only fail to catch a future implementation that produced one.

**Test-count composition.** `cargo test-ci` and the unfiltered
`cargo test -p ddb-core --lib` both exceed the 10-minute per-call ceiling in this repo
because of the `sql_engine` tier, so the fast tier was run as two commands.

Verdict: 31 findings
Tests: 1551 passed, 0 failed, 376 skipped
