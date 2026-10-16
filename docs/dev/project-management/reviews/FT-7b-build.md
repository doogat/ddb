# FT-7b build evidence — 2026-09-28

Base, local and remote `master`: `3b2700229de7858c164523b3f1f701d07d0e99f2`
(FT-7a, reviewed and committed). `git ls-remote` checked before the worktree
was created. The build session ends here; no review, commit, push, hold/00168
note or PRD move has happened.

## Isolation

- Candidate worktree: `dev/local/tmp/worktrees/00203-ft-7b`, branch
  `fast-track/ft-7b` at 3b27002. This is the location exception the
  maintainer approved for FT-7.
- Replay worktree: `dev/local/tmp/worktrees/00203-ft-7b-baseline`, detached at
  a739c29 (pre-FT-7). It holds only the three new test files, copied verbatim.
  Kept so the review can reproduce the replay; remove it after the review.
- Main checkout: untouched. The actor file SHA-256s were rechecked at exit
  and still match `dev/local/tmp/FT-7b-state.json`. `.kiro/` is untouched and
  there are no stashes.
- New untracked `docs/dev/project-management/**/.migration-placeholder` files
  appeared in the main checkout between 22:14 and 22:25 CEST. This driver did
  not create them and has not touched them (probably a hook or another
  session). Flagged for the maintainer.

## Fresh Opus test author

- Dispatch: `autopilot:ivan`, model opus, a fresh subagent with no session
  context.
- Prompt: `dev/local/tmp/FT-7b-opus-tests.txt`. It holds the card's Tests,
  Allowlist, Constraints and Read-only context verbatim, plus operating rules.
  It has no Goal, no Docs section and no driver history.
- Write scope: the three test files. Docs, the walkthrough and the notes were
  marked driver-owned. The author made no Cargo calls and no git mutations.
- Tests written:
  - CLI, in `tests/e2e/integration_bundle_import_ref_namespace.rs`
    (registered in `main.rs`): `bundle_import_masterless_bundle_never_consumes_failed_bundle_refs`,
    `bundle_import_success_preserves_sibling_and_legacy_refs`,
    `bundle_import_retry_cleans_only_its_payload_namespace`.
  - FFI, in `ddb-core/src/ffi/tests.rs`: the three `bundle_import_ffi_*`
    tests with the same names.
- Every test loops over `fetch.prune` true and false. Operations under test
  go only through the `ddb` CLI or `DoogatDriver`. The fixtures use git, git2,
  `GitRepo` and `Hlc` only, and nothing from `crate::bundle`.
- Author-reported assumptions:
  - A carries an extra doogat (`20260303000000`, "Imported A") so a stale
    merge is directly visible.
  - The retry asserts that A's master is an ancestor of HEAD, not an exact
    OID.
  - The CLI derives B's payload id from B's new 64-hex namespace, because the
    e2e crate has no sha2/tar dependency. The FFI hashes `objects.bundle`
    directly.
- Driver formatting: one rustfmt pass on the two new leaf files, which only
  touched added lines.

## Fail-first replay (pre-FT-7 runtime a739c29, isolated worktree)

- FFI: 3/3 FAILED at `ffi/tests.rs:1359` with "masterless B must fail as
  DdbError::Git ... got Ok(())". The failed-A setup checks passed on the old
  runtime (A fails, keeps a ref to A's master, is mergeable after the repair).
  3m06s compile, 0.67s run. Log: `FT-7b-logs/baseline-ffi.log`.
- CLI: the baseline `ddb` was built from a739c29 into the shared target
  (`baseline-cli-build.log`). 3/3 FAILED at
  `integration_bundle_import_ref_namespace.rs:276`: the old binary exited 0
  and printed `imported: conflicts resolved: 0` for masterless B. Log:
  `FT-7b-logs/baseline-cli.log`.
- Replay limits:
  - Every failure was on erroneous success, not on a compile error or a ref
    spelling.
  - Only the `prune=true` iteration ran, since each test stops at its first
    failure. `prune=false` has no baseline-red proof (the same limit FT-7a
    recorded).
  - The replay used the author's pre-rustfmt text. rustfmt changed layout
    only.
- The source checkout was never reset. The shared `target/debug/ddb` was
  rebuilt from FT-7b before any FT-7b CLI run (`precheck-cli-build.log`,
  `gate-7-cli-build.log`). The new CLI tests pass only on the new runtime, so
  a stale binary would show up as failures.

## Runtime unchanged

The candidate diff has no runtime file: 5 files, +982/-1 (ffi/tests.rs,
e2e module, main.rs, bundle.md, ffi.md). No FT-7a correction was needed.
Pre-checks on 3b27002 before the gates: FFI 3/3 and CLI 3/3 passed
(`precheck-ffi.log`, `precheck-cli.log`).

## Walkthrough

`dev/local/walkthroughs/00203-bundle-import-ref-namespace.md`, written only
through the showboat CLI (init/note/exec, one `pop`).

- Flow: A fails and keeps `bundle/<payload-A>/master` → local repair → masterless
  B refused with `bundle merge failed: bundle payload <payload-B> delivered no master
  branch (delivered: ["sidecar"])`, HEAD unchanged, A's doogat absent → C imports
  (`conflicts resolved: 0`) with A's and B's refs kept → retry of A succeeds, A is
  merged, only B's ref remains → reimport is a no-op.
- Isolation: every block `cd`s into a `mktemp` dir recorded in
  `/tmp/ddb-walk-00203.path`, so no block runs in the checkout and none uses `rm`.
- Stable output: logs are off, ids, OIDs and UUIDs are normalized, and ref
  listings are sorted.
- Why the one `pop`: the first B block printed refs in hash order, which is
  random. It was popped, the fixture rebuilt from the recorded blocks 1-4, and
  the block re-recorded with a sort.
- Verification, only through `dev/bin/safe-showboat-verify`: 1 passed,
  0 failed, run twice (`FT-7b-logs/walkthrough-verify.log`). The wrapper
  removed its worktree afterwards.

## Docs

- `docs/src/technical/bundle.md`: new "Recovery workflow and interface
  conformance" section covering safe retry, kept refs, the CLI/FFI outcome
  table with test locations, no parity claim for the four other transports,
  and the limits (no abort, no auto-GC, the lease does not close the write-lease
  gaps).
- `docs/src/technical/ffi.md`: the Specialized "Bundle export/import" entry
  now states the `import_bundle` guarantee. There is no signature change.
- No CHANGELOG entry, per the card.

## Gates (driver, sequential, Rust 1.89.0, `--locked`, shared `--target-dir`)

All logs are in `dev/local/reviews/FT-7b-logs/`.

1. `build`: pass, 19s (`gate-1-build.log`).
2. `clippy --workspace --all-targets -- -D warnings`: pass, 6m43s (`gate-2-clippy.log`).
3. Fast tier, run as the exact `test-ci` expansion (the literal alias breaks
   in a nested worktree): 42 + 1606 + 224 = 1872 passed, 0 failed/ignored. It
   includes the 3 new FFI tests (`gate-3-test-ci-expanded.log`).
4. `test -p ddb-core --lib bundle`: 42 passed, 0 ignored. That is FT-7a's 39
   plus the 3 FFI tests the filter also matches (`gate-4-bundle.log`).
5. `test -p ddb-core --lib ffi::tests::bundle_import`: 3 passed (`gate-5-ffi-bundle-import.log`).
6. `test -p ddb-core --test sync_test`: 6 passed (`gate-6-sync-test.log`).
7. `build -p ddb-cli`: pass (`gate-7-cli-build.log`).
8. `test -p ddb-e2e --test e2e bundle` with `CARGO_TARGET_DIR`: 8 passed. That
   covers the 3 new tests, `bundle_import_recovers_real_conflict_and_reimport_is_noop`,
   `integration_28_delta_bundle_target_export` and 3 other bundle scenarios
   (`gate-8-e2e-bundle.log`).
9. Full `test -p ddb-e2e`: 572 passed, 0 failed, 0 ignored, on the first run
   (`gate-9-full-e2e.log`).

Non-cargo checks (`rustfmt-mdbook-diffcheck.log`):

- rustfmt `--check` on the two new files is clean. For `main.rs`, 0 diffs in
  it or the new module. The 29 drift hunks all sit in untouched modules
  (`rustfmt-main-children.log`).
- mdbook build passes and the new anchor renders.
- `git diff --check` is clean, including the new file (marked intent-to-add).

## Candidate

- Snapshot: `dev/local/tmp/FT-7b-build-candidate.patch`, made with
  `git diff --binary` after `git add -N` of the new e2e file.
- SHA-256: `ac5f935082287e19e05d3d3c2c5aa984ae2446253a1f7d5473e1197b80012ddc`.

## Not done (by design, next sessions)

- The full fresh review roster, Victor, at most one rework, and closure.
- Commit, the batch-end gates, push, and CI on the exact HEAD.
- The hold/00168 CRITICAL note and moving PRD 00203 to done.
- Carried open items (not in this card's allowlist, not done here):
  - The `bundle/tests.rs` split (1,847 lines).
  - Three docs that still cite `bundle.rs`.
- Pre-existing and untouched: loupe flagged the existing `unsafe` block at
  `ddb-core/src/ffi/tests.rs:524` (poisoned-lock test, already has a SAFETY
  comment).
