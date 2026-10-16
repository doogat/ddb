# Independent review of recovered FT-6 and FT-3

Date: 2026-09-13. Fresh Codex review session after the implementation handoff.
Scope: `876a153..c57fa09`, S3 Jobs A/B only. No active PRD or autopilot run.
The historical FT-6 and FT-3 blocked reviews remain unchanged.

Initial state verified: clean `master` at `c57fa09`; previous commit `2f04f3a`;
base `876a153`; no stashes; one worktree; local `origin/master` at HEAD.
Read the recovery notes, preserved gate logs, S3 Jobs A/B, original blocker
evidence, applicable architecture guidance, and the complete two-commit diff.

## Findings

One documentation/API-description finding; no confirmed functional blocker.

**R1 (medium): FT-6's DDL guidance is too broad.** The new server documentation
and changelog say DDL commits immediately; the modified GraphQL descriptions
retain that claim and describe every batch as atomic. `handle_create_table`
and supported column/typedef ALTER handlers buffer writes during a transaction.
`DROP TABLE` and table renames still commit immediately. An isolated CLI probe
ran `BEGIN; CREATE TABLE review_ddl (name TEXT); ROLLBACK`: Git HEAD stayed at
the repository's init commit and the typedef count was zero. The immediate-DDL
claim is therefore false. Source review confirmed the DROP/table-rename
exceptions in `sql_engine/ddl.rs`.

Corrected the FT-6 changelog entry, server and SQL transaction documentation,
and GraphQL descriptions to state the actual rollback boundary. D1a and runtime
behavior are unchanged. No test added for prose-only changes.

## Closure of historical blockers

| Requirement or blocker | Independent assessment |
| --- | --- |
| FT-6 must preserve embedded/raw-FFI transactions | Exclusive is the constructor default. Shared alone guards SQL verbs and newly retained buffers. Existing raw-FFI commit/rollback regression checks Git and rows. |
| FT-6 non-first verbs and unbalanced batches | All parsed statements are inspected. The detach backstop leaves newly opened Shared buffers on the engine for Drop rollback on both success and error paths. |
| FT-6 actor must opt into Shared | Production actor uses `open_shared`; actor regression exercises BEGIN/COMMIT/ROLLBACK, a trailing BEGIN, and an unbalanced batch. |
| FT-6 internal schema application | Explicitly owned buffers survive helper calls; schema apply closes them through commit or rollback. Commit failure drops its engine/buffer. Actor regression covers the successful workflow. |
| FT-3 one cascade planner | SQL and service call the same crate-private `cascade_delete::plan`; shared scanner functions implement RESTRICT and child discovery. No duplicate walker remains. |
| FT-3 junction cleanup before row deletion | Both execution paths clean junctions before removing metadata/typed rows, within one savepoint covering all nodes and surviving reference edits. SQL appends pending operations only after success. |
| FT-3 dropped schema/row diagnostics | Typedef-path decode, schema load, and child-ID decode failures have structured tracing warnings and skip only the bad item. Query failures and unresolved discovered paths propagate. Settled warn-and-skip resilience retained. |
| FT-3 required documentation | `technical/sql-engine.md` names both entry points, shared traversal, affected-row semantics, reference composition, and rollback limits. Module and walkthrough docs cover the new boundaries. |
| FT-3 diamond/cycle correctness | Iterative DFS separates active ancestry from completed nodes. Diamond deduplication and cross-sibling cycle refusal verified in pure planner tests and independently through both CLI entry points. |

## Independent verification

The preserved implementation logs confirm build, all-target Clippy with
`-D warnings`, and 1846 fast tests (42 CLI, 1580 core, 224 server) at `c57fa09`.
The preserved CI result records successful Test and coverage jobs for run
34752919620. These are implementation evidence, not claimed as fresh runs.

Fresh checks:

- CLI probe: buffered survivor UPDATE plus parent/child cascade preserves the
  update, removes both reference targets/junctions, and creates one Git commit.
- CLI probes on both service and SQL paths: a four-node diamond deletes each
  node once in one commit; a cross-sibling cycle refuses without changing Git
  or row counts. Reproducer preserved in `stash-recovery-logs/review-probes.sh`.
- CLI probe: CREATE TABLE is rolled back with its enclosing transaction.
- `mdbook build docs`, changed-file pinned rustfmt check, and `git diff --check`
  passed after the documentation correction.
- `cargo +1.89.0 test -p ddb-core --lib cascade`: 35 passed, no failures,
  125.37 seconds execution (compilation 2m20s).
- `cargo +1.89.0 build`: passed, 2m45s.
- `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings`: passed,
  5m17s. Logs preserved as `stash-recovery-logs/review-{build,clippy}.log`.
- `cargo +1.89.0 test-ci`: 1846 passed (42 CLI, 1580 core, 224 server),
  no failures. Core execution 1038.06s; compilation 16.47s. This includes the
  raw-FFI transaction, Shared guard, actor-construction, schema-apply, and
  cascade regressions. Full log: `stash-recovery-logs/review-test-ci.log`.

Limits: Git is authoritative; SQLite rollback does not undo a Git commit if a
later index operation fails. Full e2e/property/coverage execution stays in CI.
The optional machine-local jink repro directory is absent. Jobs C/D and later
plan work remain untouched; the partially unexecuted plan must be retained.

Outcome: approved after the R1 documentation correction, committed as
`7cea5e3 docs(server): clarify SQL batch rollback guarantees`, pushed to
`origin/master`. All required local gates passed. The follow-up push's CI
was not awaited; no new CI result is claimed. Temporary probe repositories,
the temporary script, and temporary log copies were removed after preserving
the review evidence here. No new worktree or branch was created.

## Post-review CI verified, 2026-09-13

Continuation session verified clean `master`, no stashes, one worktree, and
HEAD/local `origin/master` at `7cea5e3f2388a8f5a4fd7b6beba0886ee2921663`
before changing this record. GitHub Actions was queried using that full SHA;
both workflows below ran against this exact review follow-up commit.

| Workflow | Result | Completed (CEST) |
| --- | --- | --- |
| [Test — 34767218504](https://github.com/doogat/ddb/actions/runs/34767218504) | Success; `test` and `coverage` jobs passed | 18:23:41 |
| [Docs — 34767218556](https://github.com/doogat/ddb/actions/runs/34767218556) | Success; `build` and `deploy` jobs passed | 17:59:47 |

Final Test job details confirm successful dependency policy, Clippy, CLI build,
fast tests, E2E tests, sync integration tests, and coverage steps. No failed or
skipped steps were reported. Final results checked at 18:30 CEST. This closes
the follow-up CI verification left pending above; older runs are not the
evidence for this commit.

No code changes or additional local test execution were needed. Jobs A/B and
their review are complete. Jobs C/D and later work remain unexecuted and need
a new maintainer instruction; the partially unexecuted plan is retained.
