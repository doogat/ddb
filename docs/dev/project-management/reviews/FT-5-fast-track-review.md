# FT-5 independent review — incomplete (D6 infrastructure stop)

Started 2026-09-13 23:48 CEST from the fresh review session after the build handoff.
Base: `4fd60524ca3aed2ff633ee9716b14cd8fb19d035`.
Reviewed snapshot: `dev/local/tmp/FT-5-build-ready.diff`, SHA-256
`5bf0888b86081b2078342c43c25c2c223812449de7d8fef6967da31056299d82`.
Live binary diff matched the snapshot byte-for-byte at entry. Exactly the seven
card files were modified; one worktree; old incomplete stash
`3044281e0c932fe250e657a446ef573debb20dcf` retained. No source edits in this review.

## Stop condition

**INCOMPLETE — do not commit.** Bob and Carl both exited 3 before their models
started:

```
refusing nested dispatch (already inside a CLI agent)
```

The helper guards check `CODEX_SESSION_ID`/`COPILOT_CLI`:
`~/.agents/skills/use-codex/scripts/codex-run.sh:17-19` and
`~/.agents/skills/use-gemini/scripts/gemini-run.sh:23-25`.
Their documented host-marker guard rejects this Codex-hosted driver, independently
of model health or quota. No auth or quota failure is claimed. Neither guard was
removed, bypassed, or retried. There was no automatic-approval-review rejection.

Plan §7 D6 says: "two unavailable lanes stop the batch until you decide."
No further dispatch occurred after detecting both failures; already launched
read-only reviews were allowed to finish so their output was preserved.
Resume remaining dispatch from a fresh Claude driver after maintainer direction.
Do not substitute this driver's review for Bob, Carl, or Victor.

## Roster

All launched reviewers received a fresh prompt, without the driver's conversation
or earlier reviewer findings. Native helper starts were independent (no resume).
Prompts: `dev/local/tmp/FT-5-review-<name>-prompt.md`.
Raw output: `dev/local/reviews/FT-5-logs/review-<name>.log`.
Workflow tooling was unavailable, so the plan's manual fallback was used.
Sonnet consensus and Blake used `claude-sonnet-5`; Eve used `claude-fable-5-1`.
Bob/Carl models never started.

Native transcript audit: `FT-5-logs/review-session-audit.json` records session
IDs, model IDs, timestamps, tools and token counts (Eve still in flight at the
initial capture; refreshed at exit). Blake read no diff/history/review artifacts;
its reported unchanged assertions are an inference from the card/source, not a
comparison to the baseline. All captured tool activity is read-only. Some Claude
lanes used Bash searches despite the prompt's Read-only-tool restriction, and
Cora/Trent each delegated one read-only implementor search despite the no-agent
instruction. Those deviations are retained in the audit; the subagents received
only bounded code-search prompts, not this driver's conversation. No cargo,
source edits, git mutations, or peer-review reads were observed.

| Lane | Result |
| --- | --- |
| Rita / requirements | APPROVE; one LOW formatting note |
| Cora / correctness | APPROVE; no findings |
| Grace / quality | APPROVE with one MEDIUM and one LOW |
| Toby / tests | APPROVE with two MEDIUM and one LOW |
| Mallory / security | APPROVE; no findings |
| Trent / rubric | CHANGES_REQUESTED for R12/R13 size rules; three MEDIUM, expressly nonblocking correctness-wise |
| Eve / Fable doubt | APPROVE with 8 residuals (2 FIX / 3 VERIFY / 3 KNOWN); D1-D5 pass; details below |
| Blake / blind | No findings; B1-B19 pass |
| Bob / Codex doubt/de-slop R1-R5 | UNAVAILABLE: guard exit 3 |
| Carl / Gemini | UNAVAILABLE: guard exit 3 |
| Victor | Not dispatched after D6 stop; every eventual CRITICAL/HIGH still requires an independent verdict |

Trent evaluated all 12 consensus rubric IDs but used bullet prose rather than the
requested standalone verdict-line format. Preserve the raw output. Its semantic
verdicts are pass for R1/R2/R3/R4/R6/R7/R8/R9/R10/R11 and fail for R12/R13;
canonical output can be obtained when dispatch resumes. Do not silently describe
the entire consensus roster as approved.

## Advisory ledger (unresolved; no rework consumed)

| ID | Source | Severity | Finding | Driver note |
| --- | --- | --- | --- | --- |
| RITA-1 | Rita | LOW | Two formatting-only batch-update hunks outside single-row logic | Pinned touched-file rustfmt produced these; no runtime scope extension. Grace independently recognizes them as allowed formatting. |
| TOBY-1 | Toby | MEDIUM | No added successful-writer race with an already registered SINGLETON/unique_together type | Coverage question for remaining reviewers/rework. The schema-refusal test deliberately starts with needs_transaction=false, contrary to one sentence in Toby's report. |
| TOBY-2 | Toby | MEDIUM | Colocated races accept any Err as loud refusal | Original card accepts loud refusal, and original e2e assertions must remain unchanged. Judge whether tightening new units is needed; do not weaken the hazard. |
| TOBY-3 | Toby | LOW | No nested binary-commit maintenance regression | Only commit_file is exercised by new nesting tests; binary path shares the counting behavior. |
| GRACE-1 | Grace | MEDIUM | Locked update closure increases nesting and length | Consolidate with TRENT-1. Any simplification must preserve approved lock/transaction boundary and allowlist. |
| GRACE-2 | Grace | LOW | lock_requirement predicate naming | Optional bounded naming concern; no source edit made. |
| TRENT-1 | Trent | MEDIUM | update_doogat_parsed exceeds 50-line rubric | Pre-existing long method grew; consolidate with GRACE-1. |
| TRENT-2 | Trent | MEDIUM | update.rs now exceeds 800-line rubric | Mostly mandated colocated tests; do not move tests outside approved scope without justification. |
| TRENT-3 | Trent | MEDIUM | git_ops/mod.rs exceeds 800-line rubric | File was already over the cap; mandated colocated tests add size. |

Raw reviewer counts/line citations are not driver-verified facts: e.g. Toby names
six backend tests but there are seven; Trent mentions panic coverage although the
new regression covers an Err after commit. Cora's update.rs line citations do not
match live source lines. Their actual claims and snippets remain in raw logs.

## Driver exit notes

- No review-driven Opus rework, closure dispatch, commit, stash removal, push,
  remote query or CI verification occurred before the stop.
- Build gates were NOT rerun in this review session because the source patch was
  unchanged and D6 prevented completion. Prior-session results and exact logs are
  in `FT-5-build.md`; they remain prior-session evidence, not fresh observations.
- `git diff --check` passed during review.
- Retain the initial full-e2e failure: 568/569, one setup connection reset.
  Isolated repro and unchanged full repeat passed (569/569); cause remains unproven.
- Retain `dev/local/plans/fast-track-2026-09-06.md` and all build/review evidence.
- Finish FT-5 only. No 00206, other S3 leftovers, S4, catchup or autopilot loop.
## Eve findings and final review status

Eve completed at 23:56 CEST on 2026-09-13 (session
`5c5f4f77-09c7-4399-9e26-24151c930de1`, Fable 5.1).
It returned APPROVE with 8 residual findings: 2 FIX, 3 VERIFY, 3 KNOWN;
D1-D5 pass. **No CRITICAL/HIGH was raised by any completed reviewer.**
All eight launched lanes completed with exit 0. Bob and Carl remain unavailable,
and the overall roster/review remains incomplete under D6.

| ID | Severity | Bucket | Finding / required next action |
| --- | --- | --- | --- |
| EVE-1 | MEDIUM | KNOWN | Post-commit reindex/store_head calls can interleave so a stale row remains while stored HEAD equals current HEAD. Git retains both fields. Pre-existing index-only class; serializing this window is outside approved FT-5 scope. |
| EVE-2 | LOW | FIX | CHANGELOG.md:35 and git-ops.md:93 overstate next-freshness-check repair. Soften to the actual guarantee: the operation stores its own commit identity. Do not adopt any replacement that promises all stale rows heal, because other-row/index interleavings remain possible. |
| EVE-3 | MEDIUM | KNOWN | H5 remains on batch_update / SQL UPDATE, explicitly excluded by the card. No new guarantee for those routes. |
| EVE-4 | LOW | VERIFY | Match fail-first and final race-test evidence in FT-5-build.md / FT-5-logs. Those logs are preserved; no fresh rerun performed this session. |
| EVE-5 | LOW | VERIFY | Trace GraphQL updateDoogat and REST field update to the changed service path; check typed update<Type> separately to avoid claiming batch-update coverage. |
| EVE-6 | LOW | FIX | git-ops.md:94: remove the restrictive "the single-row update's" qualifier from deferred maintenance wording; deferral applies to all counting commit paths. |
| EVE-7 | LOW | KNOWN | Two schema loads for typed updates add per-typedef read/parse work; accepted cost of the approved transaction-preview/locked-rederive design. |
| EVE-8 | LOW | VERIFY | Commit-graph unit tests require git CLI/on-disk graph; exact pushed-HEAD CI must check platform support. Eve suggested Full Validation, but that workflow is normally nightly: follow the card/plan's exact-HEAD CI requirements and record any unrun platform result accurately. |

There are 17 raw advisory findings across completed lanes (8 MEDIUM, 9 LOW),
including duplicates (GRACE-1/TRENT-1). They are not all independently verified.
Two documentation FIX items are outstanding. Zero Victor verdicts or closure
verdicts were obtained; there are currently zero CRITICAL/HIGH candidates, but
Bob/Carl can still add candidates. Rework count remains 0.

## Session measurements

Review model activity: 23:48:13–23:56:03 CEST, approximately 7m50s.
Final custody was delayed by sandbox approval; do not attribute that delay to models.
Transcript usage, deduplicated by API message ID within each session and including
Cora/Trent's two read-only search children: input 268, cache-created 846,960,
cache-read 4,500,925, output 88,755; total 5,436,908 tokens including cache.
Full breakdown and tool audit: `FT-5-logs/review-session-audit.json`.
The native Claude sessions can be resumed by explicit session ID if needed;
`claude --continue` would select only the most recent session, not the roster.

## Final custody — 2026-09-14 00:28 CEST

The plan §1 blocked-item exit requires stash or an isolated branch. After every
in-flight reviewer finished, the exact seven-file reviewed patch was quarantined
in a **new** retained stash:

- Reviewed candidate: `7f53131f57fca39a6f96affd4e338be5a3b4c6b6`, currently `stash@{0}`,
  message `FT-5 reviewed candidate; D6 review incomplete 2026-09-13`.
- OLD incomplete candidate: `3044281e0c932fe250e657a446ef573debb20dcf`,
  now `stash@{1}`; do NOT reapply it.
- `git diff --binary stash@{0}^1 stash@{0}` matches the saved build-ready
  snapshot byte-for-byte, same SHA-256 above.
- Master is clean at `4fd6052`; local origin/master is also `4fd6052`.
  One worktree, no new branch, no active PRD; fast-track plan retained.
- Initial stash command failed on sandbox `.git/index.lock` write permission;
  the required escalated retry was approved and exited 0.
- No commit/push, remote/CI query, source edit, test rerun, rework or stash deletion.
- Restore the **reviewed** stash by exact SHA with apply (retain both until the
  reviewed FT-5 patch is committed). Verify the snapshot before remaining reviews.
- Continuation: `dev/local/tmp/handoff-20260914-0028.md`.

## Resumed review — 2026-09-14 (fresh Claude driver, Opus 5)

Maintainer direction: the handoff was launched with "act accordingly", which the
handoff itself defines as the D6 resume direction. Entry checks: clean master at
`4fd6052`, both stashes present; `git stash apply 7f53131f…` restored exactly the
seven card files; live `git diff --binary` SHA-256 =
`5bf0888b86081b2078342c43c25c2c223812449de7d8fef6967da31056299d82` (matches the
build-ready snapshot). This driver's env carries only `CLAUDECODE=1`, so the
codex/gemini nested-dispatch guards did not fire. Both lanes received the saved
fresh prompts (`FT-5-review-{bob,carl}-prompt.md`) unchanged.

| Lane | Result |
| --- | --- |
| Bob / Codex doubt/de-slop | exit 0; REQUEST CHANGES; 1 HIGH (FT5-BOB-01), 2 MEDIUM FIX (BOB-02, BOB-03), 1 MEDIUM VERIFY (BOB-04); R1-R5 all pass; 9 doubts, 5 dismissed. Raw: `FT-5-logs/review-bob-resumed.log` |
| Carl / Gemini | first launch failed before the model started (`gemini-3.1-pro-preview` not available on copilot); relaunched with `-m gemini-3.8-flash` as in FT-4/FT-6: exit 0 in 5m32s; CHANGES_REQUESTED on 2 MEDIUM (CARL-01 = GRACE-1/TRENT-1, CARL-02 = TRENT-2/3); all 12 rubric verdicts emitted (R12/R13 fail, rest pass). Raw: `FT-5-logs/review-carl-resumed.log` |
| Victor (Sonnet) on FT5-BOB-01 | **REFUTED** as a FT-5 defect; mechanism real, SCOPE pre-existing-class, SEVERITY LOW. Prompt: `dev/local/tmp/FT-5-review-victor-bob01-prompt.md` |

Trent's canonical verdict-line output was not re-obtained: Carl's lane emitted the
same 12 verdicts in the requested one-per-line form with the same R12/R13 fails,
and Trent's semantic verdicts are already recorded above.

### Candidate inventory

Exactly one CRITICAL/HIGH candidate was raised across all ten lanes: FT5-BOB-01.
Victor's trace: `sql_engine/ddl.rs` `handle_create_table` commits the typedef
(`commit_file`) and then indexes it (`index_doogat`) as a separate unlocked step;
`list_type_schemas` (`service/utility.rs:41`) discovers typedefs via SQLite, so a
writer whose `ensure_fresh` ran before that commit can miss a just-committed
typedef under the lock. Baseline `4fd6052` loaded the same catalog once, unlocked,
with no mid-flight re-derive; FT-5 narrowed the window and did not touch the
primitive. Closing it needs either freshness work under the Git lock (forbidden
by the card) or an atomic typedef registration in `sql_engine/ddl.rs` (outside
the allowlist). Deferred to PRD 00190 (Risks bullet added 2026-09-14).

### Driver-resolved VERIFY items

- EVE-4 / BOB-04: `FT-5-logs/unit-red.log` shows both colocated races FAILED on
  the lost-field assertion in all 8 rounds against unchanged production code;
  `baseline-hazard.log` shows the e2e hazard FAILED in all 6 rounds at baseline;
  `focused-update.log` (3 passed) and `hazard.log` (1 passed) show them green on
  the patch. Gates are rerun below after the rework.
- EVE-5: GraphQL `updateDoogat` (`ddb-server/src/schema/mutations/operations.rs:71`),
  REST `PUT` (`ddb-server/src/rest.rs:426`) and the singleton `update_<type>`
  mutation (`schema/mutations/singleton.rs:63`) all call
  `actor.update_doogat` → `handlers.rs:81` `svc.update` → `update_doogat_parsed`.
  `batchUpdate` → `svc.batch_update` (unchanged, out of scope). No other typed
  update route exists. CHANGELOG wording is accurate.
- EVE-8: `sync_manager/tests.rs:313-324` already asserts the on-disk
  commit-graph after `commit_file`, and it runs in `test-ci` (Linux, per push)
  and `test-full` (macOS/Windows nightly); the new tests add no new platform
  dependency.

### Rework spec (rework 1 of 1)

`dev/local/tmp/FT-5-rework-prompt.md`, fresh Opus implementor (Ivan persona),
allowlist `update.rs`, `git-ops.md`, `CHANGELOG.md`. Items: EVE-2, EVE-6,
GRACE-1/TRENT-1/CARL-01 (extract locked body into a named method, `schema.is_some()`),
GRACE-2 (rename `lock_requirement` → `needs_transaction_lock`, verb-first rule),
BOB-03 (drop unused round in `Report`), BOB-02 (commit-attribution regression via
an `AT_COMMIT_TO_STORE_HEAD` test seam; fail-first replay by driver), TOBY-1
(SINGLETON-typed two-writer race). Rejected: TOBY-2 (contract accepts any loud
refusal). Accepted as-is: RITA-1, TOBY-3, TRENT-2/3, CARL-02, EVE-1/3/7 (KNOWN).

### Rework result and closure

Fresh Opus implementor returned all 7 items; rework diff `dev/local/tmp/FT-5-rework.diff`
(update.rs, git-ops.md, CHANGELOG.md only). One documented deviation: the
extracted method takes an `apply: impl FnOnce(&mut ParsedDoogat, Option<&TableSchema>)`
closure instead of the eight literal field arguments, because the literal form
trips `clippy::too_many_arguments` under `-D warnings`; same lock boundary and
sequence. BOB-02 fail-first replay by the driver: with `store_head(&commit.0)`
temporarily replaced by `store_head(&self.repo.head_oid()?.0)` the new test
FAILED (`left: 3adc3816…`, `right: ac05da28…`, `rework-bob02-red.log`); reverted,
5/5 update tests pass (`rework-focused-update.log`).

Seven read-only Sonnet closure checkers, one per finding, each on the rework
diff plus live source: EVE-2, EVE-6, GRACE-1/TRENT-1/CARL-01, GRACE-2, BOB-03,
BOB-02, TOBY-1 — all `CLOSURE: CLOSED`.

### Gates after rework (driver, sequential, pinned 1.89.0; logs `FT-5-logs/rework-*.log`)

| Gate | Result |
| --- | --- |
| `cargo build` | passed, 2m02s |
| `cargo clippy --workspace --all-targets -- -D warnings` | passed, 6m22s |
| `cargo test-ci` | 1,858 passed: CLI 42, core 1,592, server 224; core execution 1044.76s |
| `cargo test -p ddb-core --lib service` | 200 passed; 414.78s |
| `cargo build -p ddb-cli` | passed, 2m43s |
| e2e `hazard_concurrent_single_row_update` | 1 passed, 2.68s |
| e2e `integration_write_lock_races` | 1 passed, 7.29s |
| full `cargo test -p ddb-e2e` | 569 passed, 0 failed, first run; 53.63s |
| pinned rustfmt, touched files | passed |
| `mise exec -- mdbook build docs` | passed |
| `git diff --check` | passed |

### Exit — 2026-09-14

Roster complete (10 lanes + Victor + 7 closure checks), 0 surviving CRITICAL/HIGH,
rework 1 of 1 consumed. Committed on master as `99bc9bc` (`fix(core): keep both
writers' fields on concurrent single-row updates`), seven card files. Both
candidate stashes dropped by exact SHA (`7f53131f…`, `3044281e…`); stash list
empty. `git push origin master` failed twice: the 1Password SSH agent refused
the signing operation (`agent refused operation`), so the push and the exact-HEAD
CI check are pending the maintainer running the push from a terminal where the
1Password prompt can be approved. Deferred: FT5-BOB-01 → PRD 00190 Risks.

## Push and CI continuation — 2026-09-14 21:22 CEST

Fresh Codex continuation found the resumed review, rework, gates, commit and
stash drops already complete. No candidate was restored or source changed in
this session. Verified the old reviewed stash object against the build-ready
snapshot and `git diff --binary 7f53131f… HEAD` against `FT-5-rework.diff`;
both comparisons matched byte-for-byte. Rechecked preserved post-rework test
logs: 1,858 fast tests, 200 service tests, both targeted e2e gates and all 569
e2e tests passed. These are prior-session results, not new test execution.

`git push origin master` succeeded after the network-sandbox retry. GitHub API,
local master and origin/master agree on
`99bc9bcee59e277e5c14bf3c29f999c26d8ec818`. Working tree clean; stash list empty;
one worktree and only master. Retain the unfinished multi-item plan and all
FT-5 evidence. Exact-commit CI started: Test `34886490658`, Docs `34886490594`.
CI verification is in progress; stop after FT-5.

### Final exact-commit CI result

Both workflows finished for `99bc9bcee59e277e5c14bf3c29f999c26d8ec818`:

- [Docs 34886490594](https://github.com/doogat/ddb/actions/runs/34886490594):
  success, build and deploy.
- [Test 34886490658](https://github.com/doogat/ddb/actions/runs/34886490658):
  failure. Coverage succeeded; the test job failed at Dependency policy, so
  Clippy, CLI build, fast tests, e2e and sync integration were skipped in CI.
- Blocking advisory: `RUSTSEC-2026-0285`, existing `rustls 0.23.37`.
  Cargo-deny reports the fix as `>=0.23.45`. Licenses and sources passed.
  Yanked chacha20/spin and an unused license allowance were warnings only.
- Verified no baseline-to-HEAD changes to Cargo.lock, root/core/CLI/server
  manifests, test.yml or deny.toml. A dependency upgrade is outside FT-5's
  approved allowlist; no dependency edit, policy bypass or futile rerun made.
- Full failed-job log retained (ANSI colors removed):
  `FT-5-logs/ci-test-job-104118323553.log`.

FT-5 reviews/rework and prior local gates are complete; commit and push are
complete. CI verification is complete with a **failure**, not approval. Stop
here per the maintainer's FT-5-only instruction. Future dependency remediation
must rerun CI; no later fast-track item was started.

## Separate rustls dependency repair — started 2026-09-14

Maintainer explicitly authorized this bounded CI follow-up in a new continuation.
FT-5 implementation/review decisions remain settled. Entry checks found clean
master and origin/master at `99bc9bcee59e277e5c14bf3c29f999c26d8ec818`, no stashes,
one worktree; `git ls-remote` independently confirmed remote master, including a
repeat on 2026-09-18 after approval delays. No autopilot loop or other FT item
was started. Original failed CI and all earlier gate evidence are retained above.

### Advisory and dependency delta

Upstream [GHSA-2mjx-qc3c-rqvc](https://github.com/rustls/rustls/security/advisories/GHSA-2mjx-qc3c-rqvc)
and the [0.23.45 release](https://github.com/rustls/rustls/releases/tag/v/0.23.45)
confirm affected rustls versions 0.23.13–0.23.44 and the fix in 0.23.45.
RustSec's rendered page was unavailable to the browsing tool; the upstream
advisory, preserved CI diagnostic and fresh cargo-deny check agree.

`mise exec -- cargo +1.89.0 update -p rustls --precise 0.23.45` changes exactly
four existing registry packages; one changelog bullet describes the repair.
No manifest, dependency-policy, workflow, feature flag or application code changed.

| Package | Before → after | Reason |
| --- | --- | --- |
| rustls | 0.23.37 → 0.23.45 | First patched release for RUSTSEC-2026-0285. |
| aws-lc-rs | 1.16.2 → 1.18.1 | rustls 0.23.45 requires ^1.18; Cargo selected its patch release. |
| aws-lc-sys | 0.39.0 → 0.45.0 | aws-lc-rs 1.18.1 requires ^0.45.0. |
| rustls-webpki | 0.103.13 → 0.103.15 | rustls requires ^0.103.14; 0.103.15 fixes docs.rs packaging over 0.103.14. |

Inspected every lockfile hunk and the downloaded crate manifests. All four
declare MSRV 1.71/1.71.0. The only new dependency edge is aws-lc-sys → pkg-config,
a Unix build dependency; pkg-config was already locked, so no package was added.
Rustls's feature declarations are identical. WebPKI's legacy unstable alias no
longer enables AWS-LC's unstable feature; this project does not request that
alias. Direct consumers remain reqwest 0.12.28 (hyper-rustls 0.27.7) and pgwire
0.25.0 (tokio-rustls 0.26.4). Cargo's locked build resolved these unchanged.
Evidence: `FT-5-logs/dependency-update.log`, `dependency-manifest-{0,1,2}.log`.

Reviewed artifact SHA-256 (unchanged at 2026-09-21 verification):
Cargo.lock `ac2d107d5526a092db7d16d52541e52b6a76e5865e94622851ff02cd6db97eca`;
CHANGELOG.md `b0f5785fbc7307b3b229b9c33eff0e48f927ca17563c565cd5191d9b73614767`.

### Dependency review (review-with-doubt)

1. **Least confidence — transitive churn (VERIFY):** checked the old and new
   manifests and every lockfile hunk. The three transitive packages must change
   to meet the new requirements; no unrelated package changed. Dismissed.
2. **Assumptions — fix version and compiler compatibility (VERIFY):** upstream
   advisory identifies 0.23.45; all changed packages declare MSRV 1.71, and the
   pinned 1.89.0 build passes. Dismissed.
3. **Completeness — fresh validation and remote CI (VERIFY):** tracked separately
   from prior FT-5 results. Review closure requires current lint/test gates,
   separate commit, push and CI for that exact SHA; pending below.
4. **Practical failures — hidden TLS copy or policy bypass (VERIFY):** inspected
   the rustls reverse dependency graph and full lockfile diff; a single rustls
   package serves reqwest and PgWire. deny.toml, all manifests and CI configuration
   are untouched. Fresh cargo-deny reports all three policy checks OK. Dismissed.
5. **Retrospective:** exact rustls patch selection survived scrutiny. Narrowing
   to a rustls-only hunk would violate its dependency requirements; broad update,
   advisory suppression and direct dependency additions have no justification.
6. **Missing checks (VERIFY):** local build/lint/test-ci are required; native
   cross-platform checks and the heavy e2e/sync/coverage battery remain CI-owned.
   No application module changed, so no new colocated unit test, public workflow
   scenario or executable walkthrough is warranted for this lockfile repair.

Slop rubric R1–R5: all pass. Two tracked files, net two lines; no new guards,
abstractions, paraphrasing comments, or tests of third-party internals. The
changelog names the advisory and the actual version change.

Known pre-existing warnings: yanked chacha20 0.10.0, yanked spin 0.9.8 and an unused
OpenSSL license allowance. They are nonblocking, unchanged, and outside this
single-advisory repair. Policy remains unchanged.

### Fresh repair gates (sequential, Rust 1.89.0)

| Gate | Result |
| --- | --- |
| cargo-deny 0.20.2, `--locked check licenses sources advisories` | passed; exact checker version from prior CI; `dependency-policy.log` |
| `cargo +1.89.0 build --locked` | passed, 4m37s; `dependency-build.log` |
| `cargo +1.89.0 clippy --locked --workspace --all-targets -- -D warnings` | passed, 13m49s; `dependency-clippy.log` |
| `cargo +1.89.0 test-ci --locked -- --format terse` | passed, 1,858 tests (42 CLI, 1,592 core, 224 server), none ignored/filtered; build 11m20s, core execution 1044.83s; `dependency-test-ci.log` |
| Commit, push and exact-HEAD CI | passed: `a739c29`, Test `35573084487` (test and coverage jobs) |

All fresh local gates completed on the reviewed artifact. The final workspace
feature graph (`dependency-graph.log`) confirms rustls 0.23.45 with WebPKI 0.103.15
across reqwest and PgWire, with both ring and AWS-LC as before and no unstable
WebPKI alias enabled. File checksums still match; `git diff --check` passes.
Dependency review approves the two-file delta; no corrective code change was
needed. Operational completion (commit, push, exact-SHA CI) remains below.

### Commit and push — 2026-09-21

Committed separately as `a739c292748bcf5e35b5c6340eec7a360ceceb0c`
(`fix(deps): update rustls for RUSTSEC-2026-0285`), exactly Cargo.lock and
CHANGELOG.md. Working tree clean. `git push origin master` succeeded (recorded
07:28:05 UTC); `dependency-push.log` retains the result. Exact-HEAD CI pending.

### Final repair closure — 2026-09-21

GitHub's master-ref API confirmed `a739c292748bcf5e35b5c6340eec7a360ceceb0c`
(captured 07:41:32 UTC). At 09:49:26 UTC, verified
[Test 35573084487](https://github.com/doogat/ddb/actions/runs/35573084487)
completed **successfully for that exact SHA**, with both test and coverage jobs
successful. Dependency policy, Clippy, CLI build, fast tests, e2e and sync
integration all ran and passed; none of these gates was skipped. Test job
completed 07:54:06 UTC, coverage 07:52:25 UTC. Full job/step status evidence:
`FT-5-logs/dependency-ci-35573084487.json`.

Docs was not triggered: its push filter covers `docs/**` and its own workflow
file. Cargo.lock and CHANGELOG.md do not match. The earlier FT-5 Docs success
remains historical evidence, not a claimed new Docs run.

Review closure: six doubts verified and dismissed (minimum dependency churn,
advisory fix version, compiler compatibility, single TLS graph/policy integrity,
fresh required validation, and commit/push/exact-HEAD completion). Zero corrective
findings; R1–R5 pass. Three existing nonblocking policy warnings remain as noted
above; no advisory suppression or policy relaxation was introduced.

Final repository checks: clean worktree, local master and origin/master agree,
one worktree, only master, no stashes, and `git diff --check` passes. No new
temporary branches/worktrees or executed plans to remove. Retain the unfinished
fast-track plan and all requested prior failure/review evidence. The authoritative
state archives the old CI failure separately and records this successful repair.
Stop: no PRD 00206, S3 leftovers, S4, other FT item, release, or broad refresh.
