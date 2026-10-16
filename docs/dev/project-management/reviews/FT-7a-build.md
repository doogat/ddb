# FT-7a build evidence — 2026-09-27

Base/local/remote master: a739c292748bcf5e35b5c6340eec7a360ceceb0c.
Remote ref freshly verified through GitHub API before worktree creation.
User approved proceeding with the recommended abort deferral. Canonical PRD,
cards and plan record that scope; original PRD retained in designs/.

## Isolation

Created fast-track/ft-7a at dev/local/worktrees/ft-7a. Main checkout remains
on master with unrelated actor/mod.rs, actor/tests.rs and .kiro/ changes.
No stash, commit, push, source edit, or runtime test has occurred.
Trusted the unchanged worktree .mise.toml (only adds dev/bin to PATH).
Verified cargo 1.89.0. Main target cache is 104G; available disk was 57G.
Before a fresh full build, choose/recheck cache usage and avoid concurrent Cargo.

## Fresh Opus test author

Installed Ivan persona: autopilot 0.5.6. Prompt contains test specification,
write/read allowlists and constraints, not Goal/PRD/driver history. Test-only
phase; driver owns Cargo, all Git mutations and later gate execution.

1. Session 171d8592-0076-419a-8e50-5b035d44d3b2, 09:20 CEST: exit 1,
   weekly quota; reset 11am Europe/Prague. Transcript confirms zero tool calls.
   Log: FT-7a-logs/test-author.log.
2. After reset passed (clock 11:02 CEST; usage detector exit 1 = no live limit),
   new session ec18a26e-c09a-4a15-b3fa-e916baec0053 ran the same task.
   Exit 0 with an explicit blocker, NOT a successful implementation. Both Edit
   calls were rejected by enforce_prd_location.py. Log:
   FT-7a-logs/test-author-after-reset.log. Worktree verified clean afterward.

The hook's dev/local vocabulary excludes worktrees/ and explicitly directs
one-off workspaces to dev/local/tmp/worktrees/. AGENTS.md requires
dev/local/worktrees/ and nowhere else. No hook/environment bypass attempted.
A narrow location exception is awaiting the maintainer; concrete proposal:
dev/local/tmp/FT-7a-worktree-location-proposal.md. The destination is absent.

Preserved rejected Edit payloads in dev/local/tmp/FT-7a-blocked-test-draft.json
and a reviewable unified patch in FT-7a-blocked-test-draft.patch. Both payloads
match the existing test source exactly once in sequence. Neither was applied.
The draft's helper silently skips ref enumeration errors; tighten that test
oracle before use. Fail-first behavior and late-rebuild error origin remain
unverified until compilation/execution. Do not mistake the draft for evidence.

## Gates and review

### Approved location correction

The maintainer explicitly selected "Use the hook-approved location
(Recommended)" on 2026-09-27. Moved the clean worktree with git worktree move
to dev/local/tmp/worktrees/00203-ft-7a, preserving fast-track/ft-7a and base.
Trusted its unchanged mise config and updated the driver prompt/state/card.
No hooks or AGENTS.md changed. Fresh test-author session
9f7f7049-baf8-44c1-abd0-0175b224be7c now uses the approved path, with the draft
as input and an explicit instruction to reject ref enumeration errors in tests.

### Baseline proof after test authoring

Session 9f7f7049-baf8-44c1-abd0-0175b224be7c completed with only
ddb-core/src/bundle/tests.rs changed (+199/-1). Snapshot:
dev/local/tmp/FT-7a-baseline-tests.patch. The helper now fails on ref enumeration
errors instead of silently skipping them. Runtime verified unchanged by diff.

Both new tests compiled and FAILED on the intended assertions under Rust 1.89.0:

- stale-A/masterless-B: B returned a successful SyncReport with one commit
  transferred. Test ran 0.35s after 2m56s compilation. The first prune=true
  iteration failed; prune=false will execute once the first iteration passes.
- late rebuild failure: the fast-forward landed and import errored, but no
  bundle refs remained. Test ran 0.20s after 0.35s cached compilation.

Logs: FT-7a-logs/baseline-stale.log and baseline-late-failure.log (exit 101 is
expected fail-first evidence, not a compilation error). Commands read sources
from the isolated worktree and reuse the main target directory as build cache.

Fresh implementor 5cd1e4ca-274f-48f3-aa9b-578adfd57c51 now receives tests,
constraints and allowlists only; no Goal/PRD/driver history. Driver owns gates.

### Candidate and build correction

Implementor 5cd1e4ca-274f-48f3-aa9b-578adfd57c51 completed with exactly the
seven allowlisted files changed. It added payload namespaces, advertised-head
validation, the separate import lease, deferred cleanup, eight new tests
(including the two baseline regressions), adapted prior shared-ref assertions,
and docs. Log: FT-7a-logs/implementor.log. Six additional tests were authored
in the implementation session; they do not have independent baseline-red proof.

Driver formatted the two Rust files and ran the bundle filter: 36 passed,
0 failed/ignored, 7.00s after 2m44s compilation. Both prune settings of the
stale-master test now run. Log: candidate-bundle-initial.log. This is an
intermediate result, not the final gate.

Driver found a 300ms negative-observation window in the concurrency test
and stale docs claiming no Git output parsing. Fresh Opus build correction
97daf5c5-cc02-4acd-a2e9-c01e0ba62d44 changed only bundle/tests.rs, a comment
in bundle/mod.rs, and technical/bundle.md. Log: build-correction.log.
The test now runs the real import in a separate process: it must complete
the expected Conflict assertion while the parent still holds the lease,
then a second child imports after release and preserves a sibling ref.
A child completion marker prevents zero-selected-test false positives;
bounded process wait kills/reaps on deadline. Driver owns validation.
This is build correction, not the plan's independent review/rework cycle.

### Validation progress and host blocker — 2026-09-28

Corrected cross-process test passed (1 selected, 0 ignored, 0.77s; compile
2m50s): corrected-cross-process.log. Build passed in 4m16s: final-build.log.
Strict workspace/all-target Clippy passed in 7m43s: final-clippy.log; its
completion was collected after the overnight pause. Pinned Rust 1.89.0.
Source inventory before the pause: seven allowlisted files, +1053/-93;
all 15 original bundle tests retained, eight added, none ignored.
Snapshot dev/local/tmp/FT-7a-build-candidate.patch SHA-256:
09d7869d1fb374f7d83b5c2cb655a0f9049d7e4ae483ab62ffbf1d0434916fa5.

On 2026-09-28 the host rejected both read-only Git rechecks before execution:
the configured /Users/bob/.codex/hooks/block_devlocal_redirects.py is missing.
Post-pause candidate/main-checkout contents therefore remain unverified.
No hook changes or bypass. Maintainer asked to restore the hook/configuration.
Full recovery note: dev/local/tmp/FT-7a-build-blocked-20260928.md.

Remaining gates: test-ci, final bundle filter, sync integration, CLI build,
bundle e2e, full e2e, final rustfmt check, mdbook and diff check. Independent
review NOT STARTED. After these gates, END the build session and hand off to
a fresh full review roster. No FT-7b, other item, commit, stash, push or release.

### Handoff refresh — 2026-09-28 09:42 CEST

Shell access works again; the retired hook was not restored. Fresh Git checks
confirm both HEADs/base, two worktrees, empty stash list and exactly seven
modified candidate files. Binary diff matches the saved snapshot byte-for-byte;
the two unrelated main actor files still have their original checksums.
git diff --check passed freshly. No Cargo gates were rerun for this handoff.
State phase/next_phase are build; the old blocker note is historical.
Paste-ready continuation: docs/dev/tmp/00203-handoff-20260928-0942.md.
The maintainer requests autonomous progress without repeated tool approvals;
host permission mode must be selected before the new session starts.

### Final build validation — 2026-09-28 10:49 CEST

No source or documentation file changed during this validation. The seven-file
binary diff still matches `dev/local/tmp/FT-7a-build-candidate.patch` byte for
byte (SHA-256 `09d7869d1fb374f7d83b5c2cb655a0f9049d7e4ae483ab62ffbf1d0434916fa5`).
The earlier build and strict all-target Clippy pass were not rerun.

Sequential foreground gates, pinned to Rust 1.89.0 with `--locked` and the
shared target directory:

- Fast tier: `cargo test-ci` alias could not start tests because Cargo loads
  `.cargo/config.toml` twice from this nested worktree and duplicates the
  `--workspace` option. The exact expanded alias command passed: 42 + 1600 +
  224 = 1866 tests, none failed or ignored. Logs: `final-test-ci.log` (parser
  failure) and `final-test-ci-expanded.log` (pass).
- Bundle unit filter: 36 passed, none ignored (`final-bundle.log`).
- Core sync integration: 6 passed (`final-sync-test.log`).
- CLI build passed (`final-cli-build.log`).
- Bundle e2e: initial run failed 5/5 at setup because test binary discovery
  used the worktree's default `target/`, while the required CLI binary was
  built in the shared target directory. With `CARGO_TARGET_DIR` set to that
  same directory, all 5 passed. Logs: `final-bundle-e2e.log` and
  `final-bundle-e2e-target-dir.log`.
- Full e2e: initial run passed 568/569; one GraphQL request in
  `integration_45_r10_restrict_blocks_sql_and_graphql_delete` had a connection
  reset. The exact test passed isolated, then a full repeat passed 569/569,
  none ignored. This matches the setup race already noted in the fast-track
  plan; cause of the individual reset is not proven. Logs:
  `final-full-e2e.log`, `final-full-e2e-isolated-retry.log`,
  `final-full-e2e-repeat.log`.
- Touched-file rustfmt check, mdbook build and `git diff --check` passed.
  Logs: `final-rustfmt-check.log`, `final-mdbook.log`; diff result rechecked
  alongside the candidate comparison.

All named logs are under `dev/local/reviews/FT-7a-logs/`. Main checkout remains
at `a739c292` with the same unrelated actor edits and `.kiro/`; two worktrees,
no stashes. No review, commit, push or current remote/CI check has occurred.
State `next_phase` is review. Continue only in a fresh review session using
`docs/dev/tmp/00203-FT-7a-fresh-review-handoff-20260928.md`.
