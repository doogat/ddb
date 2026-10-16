# Stash recovery

Authorized scope: finish and commit FT-6, then FT-3; validate each; push/check CI;
handoff to a fresh session for independent review. No autopilot PRD execution.

Base: `876a153` on master. The original stashes were removed after their replacement commits passed local gates and were pushed. Original object IDs:

- FT-6: `fd86e90537444c0d589c1892c9047f5139e71bc8`.
- FT-3: `c343e42d61af3399d22989e22f9cd89877262a69`.

## FT-6

Restored with stash apply, not pop. Committed as `2f04f3a` on 2026-09-13.

- Private `TransactionScope::{Exclusive, Shared}`; `open`, `init`, `from_parts`
  default to Exclusive; server actor uses new `open_shared` constructor.
- SQL verb guard applies only to Shared. Transaction detachment rejects only
  newly opened shared SQL buffers; explicitly owned transactions survive
  internal helper calls (schema apply depends on this).
- Added raw FFI commit/rollback regression, exclusive batch continuation,
  shared rollback checks, actor construction and schema-apply regressions.
- Registered error as InvalidInput in the existing app error mapping and
  tested user-safe GraphQL guidance. Updated technical docs and changelog.

Validation evidence so far:

- Raw FFI regression failed against original stash (TRANSACTION_NOT_SUPPORTED
  on BEGIN), then passed after scope restriction. Initial transaction filter:
  19 passed under shell-default Rust 1.97.1; final gates use Rust 1.89.0.
- A first stricter shared-buffer implementation broke schema apply. New actor
  regression reproduced SCHEMA_APPLY_PARTIAL; restoring caller-owned buffer
  handling fixed it. Final actor tests: 2 passed with Rust 1.89.0, log
  `dev/local/reviews/stash-recovery-logs/ddb-ft6-actor-tests.log`.
- Final `cargo +1.89.0 build` passed, log
  `dev/local/reviews/stash-recovery-logs/ddb-ft6-final-build.log`.
- Final Rust 1.89 all-target Clippy with `-D warnings` passed (4m41s);
  log `dev/local/reviews/stash-recovery-logs/ddb-ft6-final-clippy.log`.
- Final test-ci passed: 1828 tests (CLI 42, core 1562, server 224),
  log `dev/local/reviews/stash-recovery-logs/ddb-ft6-final-test-ci.log`.
- Targeted server hazard passed (1 test, 2.58s),
  log `dev/local/reviews/stash-recovery-logs/ddb-ft6-hazard.log`. Initial sandbox run stalled because
  port-selection retries every denied bind; interrupted, reran with approval
  outside sandbox. No production failure.

Environment: `RUSTUP_TOOLCHAIN=1.97.1` overrides rust-toolchain.toml. Use explicit
`+1.89.0`. Global cargo fmt exposed existing formatting debt; its unrelated
changes were restored. Format only touched files with pinned rustfmt and
`--config skip_children=true`.

## FT-3 recovery

Original stash applied excluding CHANGELOG after FT-6 commit. Temporary drafts
were removed after committing the replacement; source and tests are in `c57fa09`.

The stash's BFS ancestry check misses cross-sibling cycles (root -> a,b;
a -> b; b -> a). Use active DFS ancestry instead; keep diamond deduplication.
SQL and service reference edits must compose targets once per surviving file.
Wrap all index effects in a savepoint so later failures restore earlier rows;
propagate typed-row deletion errors (the previous service ignored them).
Preserve existing warn-and-skip schema/child-row resilience, as the recovery
plan requires; every skipped item is logged with structured context. A path
resolution error for a discovered node propagates rather than being swallowed.

Implemented integration: crate-private cascade_delete module; indexer/cascade
module; extract generic crate-private with_savepoint from the Index wrapper;
generalize shared junction cleanup to accept borrowed schema iterators; expose
existing identifier escape helper crate-privately. Move SQL delete code out
of dml.rs into sql_engine/delete.rs, preserving no-match and affected-row rules.
Keep all Git writes through existing write-locked commit_batch.

Integration is now in production sources: new shared planner/reference module,
shared schema scanner, savepoint extraction, SQL delete module, and service
delete rewrite. Added unit tests and a CLI-vs-SQL smoke scenario for CI.
Original-code regression run: 1 passed, 8 failed in 50.52s, demonstrating
missing SQL cascades, false service cycle detection, reference resurrection,
and partial index updates. Log `dev/local/reviews/stash-recovery-logs/ddb-ft3-red.log`.
Targeted replacement cascade run passed: 34 tests in 121.49s. Both original
hazard tests passed in 6.83s. Added a further buffered-typedef/child regression
for the final test-ci gate. Build passed (3m08s); final all-target Clippy with
`-D warnings` passed (8m52s). Test-ci passed: 1846 tests (CLI 42, core 1580,
server 224); compilation 2m41s, core execution 1037.96s. The additional
buffered-typedef/child regression passed. Committed as `c57fa09` on 2026-09-13.

Both commits were pushed to `origin/master` (`c57fa09`). Docs CI passed;
Test CI run 34752919620 passed, including its test and coverage jobs:
https://github.com/doogat/ddb/actions/runs/34752919620.
Both stashes and the already-merged
orphan branch `worktree-agent-a188c62e61e58f6e5` were removed. The working tree
is clean; the fast-track plan remains because its later jobs are unexecuted. Independent review belongs in a fresh session;
the historical FT-3/FT-6 blocked reviews have not been relabeled as passing.

Validation logs are preserved under `dev/local/reviews/stash-recovery-logs/`.
Full-workspace format check finds existing drift in untouched files; touched
Rust files are formatted with pinned rustfmt, including the hazard fixture.

## Independent review completed, 2026-09-13

Fresh Codex session reviewed `876a153..c57fa09` against S3 Jobs A/B and the
original blockers. No functional finding; one misleading DDL/atomicity claim
was corrected in docs, changelog, and GraphQL descriptions. Follow-up commit
`7cea5e3 docs(server): clarify SQL batch rollback guarantees` is pushed to
origin/master. D1a and runtime behavior remain unchanged.

Fresh validation: 35 cascade unit tests; independent temporary-repo probes
covering buffered survivor edits, true diamonds, and cross-sibling cycles on
both deletion paths; build; all-target Clippy with `-D warnings`; 1846 fast
tests; mdbook; changed-file rustfmt; diff checks. All passed. New-push CI was
not awaited. Review and closure evidence:
`dev/local/reviews/stash-recovery-independent-review-2026-09-13.md`.
Jobs C/D and later plan work remain future work.
