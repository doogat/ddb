# 00171 cycle 2 CI acceptance evidence

Frozen review SHA: `876a153b4039128f8052526629773fa189058c71`. Refreshed 2026-09-07 from GitHub run metadata and actual job logs. No code changed for this validation.

## Current acceptance results

- [Test 34086467208](https://github.com/doogat/ddb/actions/runs/34086467208): success at the frozen SHA, 2026-09-07 05:20:57–05:43:53 UTC. Both test and coverage jobs passed. Fast test groups: 42 + 1555 + 222 = 1819 passed, zero failed/ignored. Unfiltered e2e: 566 passed, 0 failed, 2 ignored in 226.21s. Sync integration: 6 passed. Coverage: 79.88% lines.
- [Full Validation 34092612666](https://github.com/doogat/ddb/actions/runs/34092612666): one manual dispatch at the frozen SHA, all six jobs succeeded, 2026-09-07 06:50–07:45 UTC. No rerun or retry was needed. linux-clippy 101649134652 passed; linux-core 101649134529 passed 6 sync and 59 property tests; mark-validated 101662737367 posted `full-validation/passed` success at 07:45:00 UTC.

| Platform job | ID | Actual e2e result | E2e duration |
|---|---|---|---|
| linux-e2e | 101649134488 | 566 passed, 0 failed, 2 ignored | 225.76s |
| macos-full | 101649134501 | 566 passed, 0 failed, 2 ignored | 275.98s |
| windows-full | 101649134314 | 566 passed, 0 failed, 2 ignored | 602.72s |

The two ignored e2e tests on every platform are the separately owned FT-5 concurrent single-row update hazard and FT-6 server transaction-scope hazard, introduced by concurrent maintainer work. They are not migrated shell scenarios. Preserve `dev/local/plans/fast-track-2026-09-06.md` and both FT stashes. The macOS/Windows full suites include other pre-existing ignored tests; the complete summaries are retained in `00171-cycle-2-full-validation-results.log`.

## Earlier failures: explicit disposition

[Nightly 34020494801](https://github.com/doogat/ddb/actions/runs/34020494801) failed at `c8b72bd9126c83d054264ce6ac3738e8f6d0abda` on 2026-09-06. These failures remain part of the record:

- Windows job 101452070924: `git_ops::tests::commit_merge_logs_distinct_reassignment_warn_per_loser` failed at `ddb-core/src/git_ops/tests.rs:3640`; captured reassignment logs were empty. Core result: 1552 passed, 1 failed. This is outside the migration. The same named test passed in both Windows and macOS full-suite execution in current Full Validation. Disposition: earlier unreproduced core log-capture observation; current strict green-CI gate satisfied, no claimed repair or inferred cause.
- macOS job 101452070927: `integration_warnings_update::integration_58_a_graphql_update_warnings_empty` failed at `tests/e2e/common.rs:186` with `ConnectionReset` (OS 54) posting to localhost GraphQL; 564 passed, 1 failed. A prior isolated macOS rerun at `4a248cb` passed and did not establish a fix. The named scenario now passed inside the complete normally parallel e2e suite on all three platforms at the frozen SHA (Mac 07:32:07 UTC). Disposition: historical, unreproduced suite-level connection-reset observation retained; current full-suite migration coverage passes. Cause remains unconfirmed; this is execution evidence, not proof of a concurrency fix. No retry, skip, readiness change, or reduced parallelism was used to obtain this result.

The strict current push and current cross-platform CI acceptance criteria are satisfied. Independent cycle-2 review and fresh finalization remain required. Older SHA-specific evidence retains its original scope; local review counts must follow the installed review skill's own same-SHA record rule.
