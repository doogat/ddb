# FT-8b build evidence — 2026-10-02/03

Base: local `master`, `origin/master` and `git ls-remote origin
refs/heads/master` were all `d7867d236ef3d7bd353bbf4c2b72d8ec237946c0`
(FT-8a) at the start and at the end. The build session ends here. No review,
commit or push has happened. PRD 00204 stays in backlog.

## Preflight

- SHAs matched; no stashes; one worktree; tracked tree clean. The only
  untracked content was the `docs/dev/project-management/` mirror, left
  untouched.
- One other `claude` process (pid 99779, started 2026-09-30 19:56) had this
  checkout as its cwd. Its transcript shows it is the FT-8a review driver: its
  last turn (2026-10-02 20:29Z) handed over this S5b prompt and stopped. It
  had no child processes. Treated as explained, not foreign. A reviewer should
  confirm it is still idle, or closed.

## Card refresh (a3e2dbc -> d7867d2)

All drift is listed in the card under "Refresh 2026-10-02"
(`dev/local/tmp/FT-8b-card.md`; the pre-refresh card is
`FT-8b-logs/FT-8b-card.a3e2dbc.md`). In short:

1. The new GraphQL type is registered in `schema/mod.rs`, which was not in
   the allowlist. Added (one `.register` line).
2. No test module exists under `schema/mutations/`. The schema test lives in
   the existing tests of `schema/mod.rs`.
3. Walkthroughs need a built `ddb`, and the implementor runs none. Moved to
   the driver.
4. Raw-git commits carry no HLC, so the content key picks the add-add winner.
   A frontmatter-less loser must sort below `---` (start with `#`).
5. `ServerGuard` needs a `DdbTestRepo` (use `TwoNodeSetup`). Two tests cannot
   share printed output, so the GraphQL test runs its own CLI pair.
6. `sync.md:115` (step 2) was also stale, and `server.md:292-298` already
   lacked `singletonConflicts*`. The allowlist was widened to those spots.
7. The implementor sees only Tests/Allowlist/Constraints/Read-only context,
   so the exact CLI/GraphQL strings and the FT-8a docs leftovers were copied
   into those sections. CHANGELOG is driver-owned, as in FT-8a.

No scope decision was needed.

## Isolation

- Candidate worktree: `dev/local/tmp/worktrees/00204-ft-8b`, branch
  `fast-track/ft-8b` at d7867d2 (FT-7 location exception).
- Replay worktree: `dev/local/tmp/worktrees/00204-ft-8b-baseline`, detached
  at d7867d2. It holds only the candidate's `tests/e2e/collision.rs` and
  `ddb-server/src/schema/mod.rs`, minus the one `.register(..)` line (the
  type does not exist at base). Replay only; never commit it.
- Main checkout: no tracked changes. Walkthroughs live in the main checkout's
  gitignored `dev/local/walkthroughs/`, not in the patch.

## Fresh implementor

- `autopilot:ivan`, model sonnet (per card). Prompt:
  `dev/local/tmp/FT-8b-impl.txt`. It carries the card's Tests, Allowlist,
  Constraints and Read-only context, plus operating rules. 0 Cargo, 0 `ddb`,
  0 git mutations. Took ~1.5 min.
- Changes:
  - CLI: one `print_kept_verbatim` helper, called by `sync` and bundle import.
  - GraphQL: a new object type and the `SyncResult` field, with the resolver
    mapping the core vector field by field.
  - Tests: two e2e tests and one schema test.
  - Docs: sync.md, invariants.md I1 and server.md.
- Ivan's flagged deviations:
  - The I1 note is a short paragraph, not one sentence.
  - server.md also gained the two nested type lines.
  - `bundle import` printing a non-empty list is not covered by a test. The
    CLI uses the same helper, and walkthrough 00203 never hits it.
- CHANGELOG (driver): one `### Added` line, the card's text verbatim.

## Fail-first replay (d7867d2 runtime, baseline worktree)

The baseline `ddb` was built into the shared target first, then the candidate
CLI was rebuilt over it before the candidate e2e gates.

- `add_add_collision_with_frontmatterless_loser_converges_in_one_sync`: red,
  `Unexpected stdout, failed var.contains(collision losers kept verbatim: 1)`.
  Sync itself succeeded (FT-8a converges).
- `graphql_sync_reports_the_same_kept_verbatim_record_as_the_cli`: red,
  `no verbatim record line in: sync: bidirectional | ...`. It fails at its
  CLI leg, so this replay does not show the GraphQL leg red on its own.
- `sync_result_exposes_collision_losers_kept_verbatim`: red, `SyncResult must
  expose collisionLosersKeptVerbatim`. This carries the GraphQL red.
- The 3 existing collision tests stayed green.
- Logs: `baseline-1-cli-build.log`, `baseline-2-e2e-collision.log` (4 passed,
  2 failed), `baseline-3-server-schema.log`.

## Gates (driver, sequential, Rust 1.89.0, `--locked`, `--manifest-path` worktree, shared `--target-dir`)

Logs: `dev/local/reviews/FT-8b-logs/`.

1. build: pass (`gate-1-build.log`).
2. clippy `--workspace --all-targets -- -D warnings`: pass (`gate-2-clippy.log`).
3. Fast tier, the exact test-ci expansion, in the background as the only
   Cargo process: 42 + 1621 + 227 = **1890 passed**, 0 failed, 0 ignored.
   That is FT-8a's 1889 plus the new server test (`gate-3-test-ci-expanded.log`).
4. `-p ddb-server sync_result_exposes_collision_losers_kept_verbatim`: 1
   passed (`gate-4-server-schema.log`).
5. `-p ddb-core --test sync_test`: 6 passed (`gate-5-sync-test.log`).
6. `build -p ddb-cli`: pass (`gate-6-cli-build.log`). The shared
   `target/debug/ddb` is now the candidate.
7. e2e `collision`: **6 passed** (4 existing + 2 new). e2e `bundle`: 8 passed
   (`gate-7a-...`, `gate-7b-...`).
8. Full `ddb-e2e`: **574 passed**, 0 failed, 0 ignored, first run
   (`gate-8-full-e2e.log`). That is 572 + 2.
9. `dev/bin/safe-showboat-verify` on 00203 and 00204: **2 verified, 0 failed**
   (`gate-9-showboat-verify.log`).
10. `mdbook build` (worktree docs, output to the scratchpad): pass.
11. rustfmt: all six touched `.rs` files are clean at base and in the candidate
    (stdin `--check`, no child-module resolution; `fmt-check.sh`). 0 hunks
    applied. `git diff --check`: clean.

## Walkthroughs (driver, showboat CLI only)

- 00203: the old file was moved aside (`FT-8b-logs/00203-bundle-import-ref-namespace.pre-FT-8b.md`).
  It was re-recorded with `record-00203.sh`, which is the `showboat extract`
  command list with three edits:
  - the losing A file is the FT-7 YAML-invalid fixture
    (`---\nid: [unclosed\ntitle: Bad\n---\nLoser body.\n`);
  - the local repair commits the same bytes;
  - the notes say "id cannot be rewritten" and point to 00204.
  Failing A now reads `could not be rewritten: yaml: id: invalid type:
  sequence, ...`. Every other block matches the old output.
- 00204 (new, `record-00204.sh`):
  - two nodes and a bare remote;
  - `ddb sync` prints `collisions reassigned: 1`, then `collision losers kept
    verbatim: 1` and `ddb/20260310000000.md -> ddb/20811107183254.md`;
  - `cmp` confirms the loser bytes are unchanged;
  - the second sync is `up-to-date` with no extra lines, and both nodes hold
    the same files;
  - `reindex --strict` exits 1 and names the file; after frontmatter is added
    and committed it exits 0.

## Candidate

- `dev/local/tmp/FT-8b-build-candidate.patch` (`git diff --binary`, no new
  files), SHA-256
  `e166bc93402c4a5a38d8272f374fc4e22b4f2d98aa45a5d9e1b6d4ad1056e227`.
- 10 files, +223/-5.

## Points for the reviewer

- The derived id in 00204 (`20811107183254`) is content-addressed, not a
  clock. It looks future-dated. This is pre-existing `derive_content_id`
  behavior from 00167, not new here.
- The GraphQL list is `[CollisionLoserKeptVerbatim!]!` (non-null), while the
  neighbouring `singletonConflicts` is a nullable list. Per the card.
- `oldId` is never printed by the CLI. The e2e test pins it to the colliding
  id directly.

## Not done (by design)

- Full fresh review roster, Victor, at most one rework, closure; commit;
  push; exact-HEAD CI.
- PRD 00204 move to done (after landing); FT-9a/9b; release.
- Remove both FT-8b worktrees and the branch after the review lands.
