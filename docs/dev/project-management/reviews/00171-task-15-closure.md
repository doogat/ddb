# Task 15 closure — CI coverage

Original task commit: `ec28333069e7b94927abf20c2d61f546286a17fc`.
Rebased commit: `8c3e22a9ea927ddfdd04ac0bf81cf72143294b5b`.

The micro lane changed two files (3 insertions, 30 deletions). Per-push CI now executes the unfiltered e2e suite once, keeps the CLI build before it, and preserves the separate core sync and coverage gates. All 63 previously omitted qualified smoke targets are selected. AGENTS.md describes the restored coverage.

Verification at the original task commit:

- `cargo build`: exit 0 (2m 27s).
- `cargo clippy --workspace --all-targets`: exit 0 (1m 32s).
- `cargo test-ci --manifest-path /Users/bob/git/src/github.com/doogat/ddb/dev/local/worktrees/00171-review/Cargo.toml`, invoked from the main checkout: exit 0; 1,819 passed, 0 failed, 0 ignored. The core test runner reported 952.15 seconds. Full output and counts are in `00171-task-15-test-ci.log` and `.json`.
- CI-selection proof: exit 0; discovery only, no e2e bodies executed.
- The independently executed gates above are the verification evidence; historical hook execution is not relied on.

The first `cargo test-ci` invocation from inside the nested worktree was rejected before running tests because Cargo concatenated the two inherited array aliases. Running the same alias from the main checkout with an explicit worktree manifest avoided that configuration duplication without changing repository config.

Pat reviewed the committed patch through a tool-free Sonnet session. All three CI-finding components were resolved. The sole LOW suggestion was to shorten “including reused smoke scenarios”; the phrase is retained because it calls attention to the exact coverage omitted by the old filter. No correctness finding remains.

The runner initially stopped before dispatch because mise had not trusted the new worktree config (only `dev/bin` added to PATH), then hit sandbox DNS denial. Trust and network permissions were obtained; the actual review completed successfully. These were runner setup failures, not review verdicts.

Attempt: implementor `orchestrator`, pipeline `lean`, red check `n/a:micro-lane`, self-deslop `skipped:trivial`, style gate `clean`, split hygiene `skipped:no-tests`.
