# Cycle 1 rework — 00171

The required consensus (Alice), blind (Blake), doubt (Bob), and Fable guard (Eve) lenses completed. Bob ran through the documented Claude fallback after nested Codex dispatch was refused. Optional Gemini/Carl was unavailable after retry. Raw reports, the consolidated 18-finding review, parent evidence, and settled decisions are preserved in this directory.

Eight actionable findings were fixed in two rework tasks:

- Task 15: unfiltered per-push e2e coverage restores all 63 previously omitted qualified smoke targets; redundant exact steps and obsolete environment settings are removed; CI documentation is updated.
- Task 16: restores six assertion-fidelity gaps and shares the duplicated stdout and ID helpers. The new multi-junction test distinguishes first-link order from numeric ID order; existing one-reference coverage remains. The mapping still has exactly 391 source rows and now documents all strengthened checks.

Task 15 completed via the micro lane; task 16 used the Sonnet implementor's test-only lane. Both retained Pat's required patch review. Pat resolved every quoted finding. Two LOW wording/comment suggestions are carried as observations: the CI phrase identifies the omitted reused scenarios, and comparing all Markdown stems with an already validated survivor ID makes the removed ID filter redundant while detecting unexpected files. Neither observation describes a remaining correctness defect.

## Verification

Task 15: cargo build, all-target Clippy, and test-ci passed; 1,819 tests passed, none failed or ignored. The rebased CI patch was verified byte-identical to the reviewed patch.

Task 16 and the final work-phase gate at `74644b8825e52794f1274b3dcec9875d1a17d808`:

- `cargo build` → exit 0
- `cargo clippy --workspace --all-targets` → exit 0
- `cargo test-ci --manifest-path /Users/bob/git/src/github.com/doogat/ddb/dev/local/worktrees/00171-review/Cargo.toml` → exit 0

Fast tests: 1819 passed, 0 failed, 0 ignored. This settled-content pass also serves as final phase verification; repeating identical successful checks would violate the host instruction against unnecessary reruns. Detailed logs and runner summaries are preserved. The nested worktree requires invoking test-ci from the main checkout with its manifest explicitly selected, avoiding Cargo's concatenation of inherited array aliases.

All 13 committed test paths match the allowlist. The inventory audit finds 568 registered e2e tests including two independently added ignored hazard cases; the newly authored 00171 test is active. Full e2e execution remains delegated to CI. Source and selection audits are not runtime conformance claims.

An overly broad formatter pass was discarded: 52 unrelated files and three unrelated hunks in rest.rs were restored. The final patch retains only task changes. No repository-wide format pass or historical hook execution is relied on as validation evidence.

15: style_gate: clean, split_hygiene: skipped:no-tests
16: style_gate: clean, split_hygiene: skipped:no-tests

The Python-only style/split tools had no Python candidates. Test-only rework skips self-deslop and red-check by the installed work skill, while retaining Pat. Implementor assumptions are preserved in `00171-task-16-ivan.txt` and `dev/local/meta/assumptions.md`.

The skeptical rework pass examined 13 checks: four fix groups, seven verified concerns, and two outstanding limits (fresh CI and mandatory next-session review). See `00171-cycle-1-rework-doubt.md` for evidence and the second pass.

## Handoff

Do not finalize or drain the backlog. After integration/push, record CI status, preserve the original work_start_sha, and use the canonical rework transition to hand off to fresh cycle 2. The original 14 build tasks plus both review tasks are complete once state task-done is applied. Current review acceptance is still pending.

Final outcome: both commits landed on master and were pushed. Test run 34057702782 succeeded at 4a248cb, including full e2e and coverage. The earlier macOS connection reset passed a single-case local reproduction but remains an unresolved full-suite observation for cycle 2. See 00171-cycle-1-final-ci.json and 00171-cycle-2-context.md. The temporary worktree, branch, and mise trust were removed; owned scratch files are archived with byte verification before handoff.
