# FT-8a build evidence — 2026-09-30

Base: local `master`, `origin/master` and `git ls-remote origin
refs/heads/master` were all `a3e2dbc59b340731a4bd46752c6ae52fba80cbff` at
the start and at the end. The build session ends here. No review, commit or
push has happened.

## Cards

This session wrote all four S5 cards: `dev/local/tmp/FT-8a-card.md`,
`FT-8b-card.md`, `FT-9a-card.md` and `FT-9b-card.md`. Only FT-8a was built.

## Card-time drift (driver decision, flagged for review)

- PRD 00204 Phase 1 says to re-point the bundle tests at `:299`/`:335` so the
  frontmatter-less import SUCCEEDS.
- Since FT-7a/7b, that frontmatter-less loser is the shared "real failing
  import" fixture for:
  - 6 `bundle/tests.rs` failure-path tests (reachable ref, clean repo,
    masterless-B, retry + lease, preserves other failed refs, fetch-prune);
  - 3 FFI tests;
  - 3 CLI e2e tests;
  - walkthrough 00203.
- Flipping them to success would have deleted FT-7's failure-path coverage.
- The card instead moves those fixtures to a YAML-invalid loser
  (`"---\nid: [unclosed\ntitle: Bad\n---\nLoser body.\n"`). It still aborts
  through the same `bundle merge failed:` path. One new bundle test asserts
  that a frontmatter-less import succeeds with one verbatim fold.
- The FFI and e2e fixture files were added to the allowlist for this reason.
- Walkthrough 00203 still records the old frontmatter-less failure, so it will
  now diverge. The FT-8b card owns re-recording it.

## Isolation

- Candidate worktree: `dev/local/tmp/worktrees/00204-ft-8a`, branch
  `fast-track/ft-8a` at a3e2dbc. The FT-7 location exception applies.
- Replay worktree: `dev/local/tmp/worktrees/00204-ft-8a-baseline`, detached at
  a3e2dbc. It is replay only and must never be committed. It contains the
  candidate files plus one shim (see below).
- Main checkout: untouched. `git status --porcelain` was empty at the end, and
  there are no stashes. No foreign session was found in the checkout; the only
  process with this cwd was this session.

## Fresh Opus implementor

- `autopilot:ivan`, model opus. Prompt: `dev/local/tmp/FT-8a-opus-impl.txt`.
  It carries the card's Tests, Allowlist, Constraints and Read-only context
  verbatim, plus operating rules. It has no Goal and no driver history. It
  made 0 Cargo calls and 0 git mutations.
- CHANGELOG.md was driver-owned. The driver wrote the two card lines: Fixed
  (sync) and Changed (`GitMerge::commit_merge`).
- Implementation: see the build diff. `has_frontmatter` equals
  `find_frontmatter(..).is_ok()`. `fold_one_loser` returns
  `Option<CollisionLoserKeptVerbatim>`, and on both branches the id is derived
  and inserted into `taken_ids`. The rewrite-failure `Conflict` mapping is
  unchanged.
- Scope note: `sync_manager/mod.rs` also changed `create_conflict_commit`'s
  return type and its one call site (:346). This is needed to thread the
  records; it is slightly wider than the card's cited line ranges.
- Implementor-reported risk, now resolved by gates: a frontmatter-less file
  left in `ddb/` after merge must degrade on the post-merge reindex (I5). The
  sync_manager test and the bundle test both pass through the real reindex.

## Fail-first replay (a3e2dbc runtime, isolated worktree)

- The new tests cannot compile against the old `CommitHash` return. As the
  card allows, the replay therefore used the candidate files plus a one-token
  shim in the baseline copy of `merge.rs`:
  `if true || has_frontmatter(..)`. That forces the pre-FT-8a
  always-rewrite branch, which is exactly the old fold decision.
- Result (`FT-8a-logs/baseline-replay-core.log`): **5 FAILED, 48 passed**.
- Every failure was the wedge (`could not be rewritten: parse: no
  frontmatter opening ---`), not a setup or compile error:
  - frontmatterless_loser_lands_byte_identical_at_its_derived_path
  - frontmatterless_loser_derives_the_same_path_in_either_merge_direction
  - mixed_loser_batch_lands_in_one_commit_and_lists_only_the_verbatim_fold
  - sync_report_lists_every_verbatim_folded_collision_loser
  - frontmatterless_bundle_loser_imports_and_reports_one_verbatim_fold
    (`bundle merge failed: ...`)
- These passed on the shim:
  - the controls `loser_with_frontmatter_is_still_id_rewritten` and
    `loser_with_unparseable_yaml_still_aborts_the_whole_batch`;
  - the re-pointed `commit_merge_commits_nothing_when_one_loser...`;
  - all re-pointed bundle failure-path tests;
  - the three FFI bundle tests.
- Replay limits:
  - The shim changes the runtime of the baseline copy by one token. It is a
    shape adapter, not the untouched a3e2dbc binary.
  - The re-pointed CLI e2e fixtures were not replayed against the pre-FT-7
    runtime (a739c29). Their FFI twins passed on baseline, which shows the
    YAML-invalid import still fails.

## rustfmt

- rustfmt 1.89.0 was run whole-file on the five touched files that were
  already clean at a3e2dbc (bundle/tests.rs, ffi/tests.rs, traits.rs,
  types/doogat.rs, the e2e file).
- The other seven touched files (merge.rs, git_ops/tests.rs, parser/tests.rs,
  sync_manager/tests.rs, parser/mod.rs, git_ops/mod.rs, sync_manager/mod.rs)
  were formatted hunk by hunk. The helper
  (scratchpad `fmt_touched.py`, stdin rustfmt, no child-module resolution)
  applied only hunks that touch lines added since a3e2dbc. It left 40
  pre-existing drift hunks alone (3 + 32 + 1 + 4), because merge.rs,
  git_ops/tests.rs, parser/tests.rs and sync_manager/tests.rs were already
  drifted at base.
- A final rerun applied 0 hunks. Whole-file `--check` is clean on the five
  files that were clean at base.

## Gates (driver, sequential, Rust 1.89.0, `--locked`, `--manifest-path` worktree, shared `--target-dir`)

All logs are in `dev/local/reviews/FT-8a-logs/`.

1. build: pass, 4m22s (`gate-1-build.log`).
2. clippy `--workspace --all-targets -- -D warnings`: pass, 6m05s
   (`gate-2-clippy.log`).
3. Fast tier, the exact test-ci expansion, run in the background as the only
   Cargo process: 42 + 1616 + 226 = **1884 passed**, 0 failed, 0 ignored
   (`gate-3-test-ci-expanded.log`). Against FT-7b's 1872: +10 core tests from
   this item, and +2 server tests from eec6884 (00172 phase 0).
4. Filtered core: one libtest run with the card's filters (`has_frontmatter`,
   the git_ops tests, `sync_report_lists...`, `bundle`,
   `ffi::tests::bundle_import`): **53 passed** (`gate-4-filtered-core.log`).
5. `--test sync_test`: 6 passed (`gate-5-sync-test.log`).
6. `build -p ddb-cli`: pass. The shared `target/debug/ddb` is now the
   candidate (`gate-6-cli-build.log`).
7. e2e `collision`: 4 passed. e2e `bundle`: 8 passed, including FT-7b's 3
   re-pointed CLI scenarios (`gate-7-e2e-collision-bundle.log`).
8. Full `ddb-e2e`: **572 passed**, 0 failed, 0 ignored, on the first run
   (`gate-8-full-e2e.log`). CLAUDE.md requires this because the item replaces
   the abort code path.
9. `git diff --check`: clean.

## Candidate

- `dev/local/tmp/FT-8a-build-candidate.patch` (`git diff --binary`, no new
  files), SHA-256
  `51f59851fe87a8f501c041b8ad7c69d5d83440c8fa77ac2b0617a895caff766d`.
- 13 files, +618/-72.

## Not done (by design)

- Full fresh review roster, Victor, at most one rework, closure; commit; push;
  exact-HEAD CI.
- FT-8b (renders, e2e scenarios, docs, walkthroughs 00203 re-record + 00204),
  FT-9a/9b.
- Remove both FT-8a worktrees and the branch after the review lands.
