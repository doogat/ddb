# PRD 00171 build evidence for the fresh review session

This is review input, not acceptance. The PRD remains in `wip/`. Only the installed build-to-review transition may advance the phase; the fresh session must run every required lens and its normal review/rework loop.

## Identity and scope

- PRD: `dev/local/prds/wip/00171-migrate-shell-scenarios-to-e2e-v1.md`.
- Reviewed design: `dev/local/designs/00171-migrate-shell-scenarios-to-e2e-v1-design.md`.
- Complete coverage map: sibling `00171-migrate-shell-scenarios-to-e2e-v1-mapping.md`.
- Preserve `work_start_sha=b1e785395fb1e5d7043fbc725cc5786f62a6417e`. Do not narrow the whole-PRD review to this continuation's commits.
- Task14 base `5cf4c360556c1b8aa9a3e1bb7dfd83496ba269e4`; retirement revision `c8b72bd9126c83d054264ce6ac3738e8f6d0abda`.
- Unrelated 00202 commits, including `52759bd`, `a7e5179` and `75cde1c`, were preserved. Do not redo their repaired retained-lineage HLC/clippy work because an older CI run was red.
- Only PRD00171 is authorized here. Do not start00172, drain the backlog, or close issues.

## Settled design decisions

Use isolated ports/temp repositories, explicit thread coordination, `tokio_postgres`, and unconditional PgWire coverage. The reviewed design supersedes old serial-marker/psql-skip suggestions. Equivalent names/counts do not prove coverage: preserve original must-fail, race, platform-specific and data assertions. The product's six-transport posture is settled.

## Task12 proof

All391 source rows were reconciled independently:56 Bash smoke,56 PowerShell smoke,140 Bash integration,139 PowerShell integration. All targets resolve in the actual565-test runner registry. No TODO rows or unregistered target modules remain. Source paths and line references now refer to Git history at the task14 parent revision.

- `00171-task12-verification.md`: local gate results, repeat integration runs, failure repairs, review closures.
- `00171-task12-smoke-audit.md`, `00171-task12-parta-audit.md`, `00171-task12-partb-audit.md`: source assertion audits, including independent PowerShell17/18 reconciliation.
- `00171-mapping-discovery.json`:391 source keys and registered target proof;565 discovered tests.
- `00171-task12-devon.md`, `00171-task12-devon-round2.md`: actual adversarial runs, strengthened parsed Git HEAD body/title assertions, exact singleton theme checks and caught replays.
- Five `00171-task12-pat-*.md` partition reports cover every changed file. The one MEDIUM (lost original GraphQL auto-seed coverage beside the CLI port) was repaired in `32aa884` and its resumed review returned NO FINDINGS. Initial report is preserved as `00171-task12-pat-server-r1.md`. Fourteen LOW notes remain for the fresh review to assess.

Task12 commits: `faede1d`, `3965754`, `36c0687`, `32aa884`. The newly registered singleton parity/write-lock modules and section55B/C/D races are covered by actual discovery and execution, not file existence alone. Task12 repeated `integration_` runs passed83 tests; the affected broad subset passed278 tests after repairs. Tier1 passed42CLI+1555core+222server tests.

## CI before shell deletion

Task13 commit `5cf4c36` changed only the two workflows to the exact reviewed replacement wiring. It was pushed BEFORE dispatching Full Validation. Full Validation run33991344845 passed all six jobs, including Linux e2e, macOS full and Windows full, on that exact full SHA. Push Test33991275129 also passed, including the Rust smoke step. No older revision or skipped platform job was accepted.

Evidence: `00171-task13-verification.md`, `00171-task13-ci-green.json`, `00171-task13-ci.json`, `00171-task13-ci-observation.json`. The historical119-unpushed/auth warning was removed through statectl after the actual successful push. Task13 used the then-installed0.5.0 routing and actual Qwen helper; task14 uses installed0.5.1 after a cache update. Routine task13 config-only gate exceptions are recorded explicitly.

## Retirement and validation

Commit `c8b72bd` deleted all four shell suites after the exact-revision green gate; no survivor is needed. AGENTS, CONTRIBUTING, README and technical testing docs now target Rust e2e scenarios. The unrelated jink-feedback repro instructions were preserved byte-for-byte. Every Rust change in task14 is a comment/whitespace edit; replacement assertions and runtime tokens are unchanged. All23 SQL documentation scenario references resolve to actual discovered tests, and the full repository live-reference search finds no obsolete script path.

Post-deletion push Test34016441899 passed both test and coverage jobs on full SHA `c8b72bd9126c83d054264ce6ac3738e8f6d0abda`; completed2026-09-06T06:47:05Z. Evidence: `00171-task14-ci-green.json`.

Task14's independent Devon baseline passed565 tests. Three generic executed mutants were caught: body updates routed to title; discarded SINGLETON declaration; SQL title literals discarded. The title mutant passed the five focused concurrency tests but failed the complete suite on five real behavior assertions. Incidental socket connection failures were explicitly excluded from the mutation verdict. A final real-binary focused control passed5/5. All facades were overwritten as harmless forwarders and independently checked. Full details and exact mutant bodies: `00171-task14-devon.md`.

Task14's complete patch was reviewed in21 bounded Pat partitions, with every deleted source line included exactly once and relevant audited mapping rows supplied per deletion segment. All21 actual reports pass the installed parser: seven LOW findings, no MEDIUM/HIGH/CRITICAL findings. Eight partitions used Sonnet. At the user's explicit request to avoid a quota wait, thirteen used Codex: ten fresh isolated agents and parent reviews for partitions12,16,20. All thirteen Codex reports returned NO FINDINGS. Final provenance and LOW notes are in `00171-task14-pat-index.json` and `00171-task14-pat-assessment.md`. Invalid initial outputs and quota banners are separately preserved, never counted as completed reviews.

All14 build tasks are complete at the retirement revision. Required local gate totals:2384 passed,0 failed,0 skipped (1819fast +565e2e); the supplemental5-test real-binary control is recorded separately. The same-HEAD final result is in `dev/local/autopilot/last-verification.json`. No source edit followed the green gates. `00171-build-phase-report.md` contains task stamps and the unchanged assumption ledger. This continuation's scratch was archived with byte verification before cleanup; manifests and archives are in reviews. The original handoff and all other PRDs' artifacts are preserved.

## Fresh review requirements

Run consensus(Alice), blind/PRD-only(Blake), and doubt plus de-slop(Bob, R1–R5) in every cycle, each with the installed isolation contracts. This build session has not invoked `review-work-completion`. Review the full preserved PRD range and use `last-verification.json` only when its SHA matches the reviewed HEAD. Per-task LOW notes are observations to assess, not automatically accepted defects.

No new source-root database artifacts, worktrees, plans, or branches were created by this continuation. A pre-existing unrelated `worktree-agent-a188c62e61e58f6e5` branch was left alone. Cleanup and exact final verification are recorded in the task14 report before handoff.

## Cycle 1 rework handoff

Read `00171-cycle-2-context.md` before cycle 2: both rework tasks are complete, exact verification and CI heads are recorded, concurrent maintainer commits are outside 00171, and an earlier macOS connection-reset observation remains for fresh review.
