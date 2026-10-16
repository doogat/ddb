# ddb fast-track plan: 2026-09-07 to 2026-09-12

Maintainer's steer (verbatim): "We need a fast track implementation plan that will avoid autopilot/autoclaude loops because these are terribly slow. On the other hand I like how autoclaude/autopilot loops review the work done, so we don't ship slop code. Find a balance - the key is using subagents with different models and zero context from driving session (so they are not biased)."

This file is the reference for the next days. Each session below has a copy-paste starting prompt. Sessions are fresh Claude Code sessions in `~/git/src/github.com/doogat/ddb` (this keeps reviewers and implementors free of any driving-session context by construction). Read §1 once; then just run the sessions in order.

Companions: `dev/local/audit-results/refactor-assessment-2026-09-06.md` (why), `~/.claude/workflows/fast-track.workflow.js` (the harness), PRDs `00203-00205` (specs for batch 2), the six pinned hazard tests (specs for batch 1; commit `dbc128f`).

## 1. The lane (what every session runs)

One **item** = one spec card + one pass through the harness: fresh implementor, sequential gate, every review lens with zero session context, adversarial verification of every CRITICAL/HIGH, one rework, exit rule. What the loop's phases cost is gone (catchup, design-solution, plan-tasks, per-task Pat/Tess/Devon, self-deslop, per-item full suite, cap-deferred ledger). What the loop's review is worth stays: the same personas from the installed autopilot plugin, four model families, refuters before anything blocks.

| Stage | Who | Sees | Model |
|-------|-----|------|-------|
| Card | you + the session (attended, ~10 min) | the item's row in §2/§3, the tests, the allowlist, the decision | n/a |
| Implement | fresh implementor (Ivan persona) | failing tests + allowlist + card constraints + read-only context; never acceptance prose, never the session | sonnet; opus when the card says so |
| Gate | one sequential runner | the card's gate commands (build, clippy at 1.89.0, the item's tests, touched e2e modules) | n/a |
| Review (parallel) | review-fanout child workflow (rita, cora, grace, toby, mallory, trent, victor); Eve; Blake (card only, never the diff); Bob via codex; Carl via gemini | card + diff (+ repo read access) | Claude sonnet, Fable, Codex, Gemini |
| Verify | Victor per CRITICAL/HIGH from Eve/Blake/Bob/Carl (the fan-out verifies its own) | the finding + repo | sonnet |
| Rework (max 1) | a second fresh implementor | confirmed findings as the spec | as Implement |
| Closure | one checker per confirmed finding, on the rework diff | rework diff | sonnet |
| Exit | the session (you watching) | `ready`: commit on master (conventional message; CHANGELOG under Fixed/Added when the card says so). `blocked`: `git stash` or a `fast-track/<item>` branch; master untouched | n/a |
| Batch end | the session | `cargo test-ci` + `cargo test -p ddb-e2e`, then `git push`; CI Tier 2 does the rest | n/a |

**Harness call** (the session does this; the prompt below spells it out): `Workflow({scriptPath: "/Users/bob/.claude/workflows/fast-track.workflow.js", args: {repo, item, card_path, plugin_root: "/Users/bob/.claude/plugins/cache/buvis-plugins/autopilot/0.5.1", fanout_script: "/Users/bob/.claude/workflows/review-fanout.workflow.js", date: "<today>"}})`. The harness refuses a dirty tree, so commit or stash between items. It returns `outcome` (`ready` / `blocked` / `gate_failed` / `implementor_blocked`), `remaining` findings, `assumptions`, `lane_status` and the review file path `dev/local/reviews/<item>-fast-track-review.md`.

**Manual fallback** if the harness misbehaves: run the same stages by hand with the Agent tool (implementor via `subagent_type: autopilot:ivan`, Eve via `autopilot:eve`, Blake via `autopilot:blake`, Victor via `autopilot:victor`; Bob/Carl via the `use-codex`/`use-gemini` helper scripts as background Bash) and record the same review file. Never skip a lane; never let a CRITICAL/HIGH block without a Victor verdict; never commit a `blocked` item.

**Card format** (`dev/local/tmp/<item>-card.md`; the harness parses these exact headings):

```markdown
# FT-4 raw create with an existing id must not overwrite
## Goal
One paragraph: what must be true after the change, and the decision it implements.
## Tests
- ddb-core/tests/hazard_raw_create_existing_id.rs::raw_create_with_existing_id_rejects_or_remints_and_never_overwrites
## Allowlist
- ddb-core/src/service/create.rs
- ddb-core/tests/hazard_raw_create_existing_id.rs (only: remove the #[ignore] attribute)
- CHANGELOG.md (only: one line under ## [Unreleased] ### Fixed)
## Read-only context
- ddb-core/src/error.rs
- ddb-core/src/id_minting.rs
## Constraints
Decisions and invariants the implementor must respect (copy the row's Decision; cite invariants.md rules; "no new public API" etc.).
## Gate
- cargo build
- cargo +1.89.0 clippy --workspace --all-targets -- -D warnings
- cargo test -p ddb-core --test hazard_raw_create_existing_id
## Docs and CHANGELOG
Which doc paragraph changes (path) and the CHANGELOG line, or "none".
## Transport impact
One line per affected transport, or "core-only; every transport inherits".
## Model
sonnet
```

Budget per item (to be measured on FT-4 and written into §8): implement 10-30 min, gate 5-8 min, review 8-15 min in parallel, rework 10-15 min; expect 45-75 min and 0.6-1.5M subagent tokens. Batch-end `cargo test-ci` is 16 min today.

Rules that never bend: one cargo user at a time (the harness serialises; do not run cargo yourself while it runs); the CI clippy toolchain is 1.89.0; never edit a walkthrough by hand; never run `ddb` inside the checkout; `rm` goes through warden; pushes at batch end only after test-ci + e2e are green.

## 2. Batch 1: the six confirmed hazards (specs exist, all red, committed `#[ignore]`d in `dbc128f`)

All six tests were written by zero-context agents from the assessment's static findings and run once at HEAD `c8b72bd`: every hazard is real. Order is smallest-first so the harness is proven on a 20-line change before the two design-bearing items.

| Item | Test (un-ignore with the fix) | What failed at HEAD | Allowlist (edit) | Decision |
|------|------|------|------|------|
| FT-4 | `ddb-core/tests/hazard_raw_create_existing_id.rs` | `create_doogat_raw` with frontmatter `id:` of an existing doogat overwrote its title and body | `ddb-core/src/service/create.rs` (raw path :419-511) | reject with a Conflict error (`DoogatError::Conflict` or a structured code); never overwrite, never silently re-mint |
| FT-1 | `ddb-core/tests/hazard_update_unique_clobber.rs` (2 tests) | `update` and `batch_update` accepted a UNIQUE collision; INSERT OR REPLACE dropped the typed table from 2 rows to 1 while git kept both | `ddb-core/src/service/update.rs`, `ddb-core/src/service/validation.rs` (reuse `check_unique_constraints` with a self-id exclusion) | reject with `UNIQUE_VIOLATION` before the commit, on both update lanes |
| FT-2 | `ddb-core/tests/hazard_delete_owner_junction_rows.rs` | one owner-side row left in `bookmark_category` after `delete_doogat` (reverse-only cleanup at `indexer/mod.rs:480`) | `ddb-core/src/indexer/mod.rs` (make `cascade_junction_cleanup` two-direction like `sql_engine/dml.rs:1266-1332`) or route `service/delete.rs` through the SQL engine's copy | one two-direction cleanup used by both delete paths |
| FT-3 | `ddb-core/tests/hazard_sql_delete_cascade.rs` (1 failing + 1 passing control) | `DELETE FROM link` returned Affected(1) and the ON DELETE CASCADE child stayed readable; the service path cascades correctly | `ddb-core/src/sql_engine/dml.rs` (delete legs :681-780 call the same cascade planner as `service/delete.rs:31-62`) | cascade on the SQL path exactly like the service path; RESTRICT still refuses |
| FT-6 | `tests/e2e/hazard_server_txn_scope.rs` | with a PgWire `BEGIN` open, a GraphQL `executeSql` INSERT returned success and never reached git (buffered into the foreign transaction) | `ddb-server/src/pgwire.rs`, `ddb-server/src/schema/mutations/operations.rs`, `ddb-core/src/service/sql.rs`, `docs/src/technical/server.md` (:51 is wrong), the test (its BEGIN step accepts a structured error as the safe outcome) | D1: reject `BEGIN`/`COMMIT`/`ROLLBACK` on the shared server actor with a structured error naming `executeBatch` as the atomic unit; in-process CLI/FFI transactions unchanged; per-connection sessions are decided in the contract design (S7) |
| FT-5 | `tests/e2e/hazard_concurrent_single_row_update.rs` | a field vanished in 6 of 6 rounds while both writers exited 0 (read outside the lock, no expected-OID check) | `ddb-core/src/service/update.rs`, `ddb-core/src/git_ops/mod.rs` (a `pub(crate)` way to run the read-modify-write inside the re-entrant write lock; `with_write_lock` is at :310) | D2: single-row update reads inside the write lock; if the lock cannot wrap the read, an expected-OID refusal at commit; loud, never silent. Model: opus |

Batch end: `cargo test-ci` + `cargo test -p ddb-e2e` green, push, watch CI.

## 3. Batch 2: the unowned tail (specs = PRDs 00203-00205; run as items, never through the loop)

| Item | Source | Split into items | Note |
|------|--------|------------------|------|
| FT-7a/b | `00203-bundle-import-ref-namespace-v1.md` (the live 00168 CRITICAL) | Approved 2026-09-27: a = complete fetch/merge/cleanup fix + unit regressions/docs; b = CLI/FFI conformance, walkthrough and closure. Abort API explicitly deferred; see approved cards and preflight note. | opus; sequential Tier 1 plus explicit sync/bundle gates; fresh build and review sessions per item |
| FT-8a/b | `00204-sync-collision-loser-without-frontmatter-v1.md` | a: core fold + SyncReport field; b: renderers (CLI, GraphQL, FFI record) + e2e + docs | opus for a |
| FT-9a/b | `00205-index-eviction-by-indexed-identity-v1.md` | a: capture indexed (id, type) + both eviction legs + tests; b: evict_doogats fail-loud, rustdoc, docs | sonnet |

Then `cargo test-ci` + e2e, push, dispatch Full Validation, and **release v0.2.8** with `dev/bin/release` (197 commits unreleased since v0.2.7; standing policy is a release per phase; jink's two named blockers shipped in v0.2.7).

## 4. Batch 3: the contract block (after one attended design doc)

The block IS the refactor the assessment recommends; it does not start before its design doc. Sessions S7-S10 below. Items after the design: `base_types.rs` split, `queries.rs` split, 00172 (closure actor + explicit event intent + core-first schema-changed signal; 2-3 items), 00173 (error table in core; 2 items), then 00175, 00176, 00197, 00177, 00178 with the rewritten conformance fixtures as acceptance. The wire-break items ship together in v0.3.0 with a jink migration card (~7 call sites: `link.rs:11-21,:277`, `quote.rs:35,:58`, `category.rs:137,:174`, `types.rs:445`). 00179-00199 stay in the backlog for the loop once the plugin's policy PRDs land.

## 5. Session schedule

Times are estimates for attended wall-clock; a session ends when its exit criteria hold or after ~4 hours, whichever comes first (then the next session's prompt picks up from the handoff block).

| Day | Session | Scope | Exit criteria |
|-----|---------|-------|---------------|
| Mon 09-07 | S1 | harness bring-up on FT-4, then FT-1 | both `ready`, committed; harness fixes committed to buvis; §8 has FT-4 timings |
| Mon 09-07 | S2 | FT-2, FT-3, FT-6 | three commits; batch-end `test-ci` + e2e green; pushed; CI Test green |
| Tue 09-08 | S3 | FT-5 (opus) + pre-flight leftovers (sync_test in gates, mint-sleep timing, registry test item) | FT-5 committed; measurements written into §8 |
| Tue 09-08 | S4 | FT-7a, FT-7b (00203: the live CRITICAL) | 00168 CRITICAL closed with the repro e2e green; hold/00168 annotated; pushed |
| Wed 09-09 | S5 | FT-8a/b (00204), FT-9a/b (00205) | four commits; `test-ci` + e2e green; pushed; Full Validation dispatched |
| Wed 09-09 | S6 | release v0.2.8 + docs corrections item (transport-policy-inventory, app-contract.md, ffi.md UDL, mobile-constraints.md, design-decisions.md:177, sql-engine.md) | tag pushed, CI release artifacts published; docs item `ready` and committed |
| Thu 09-10 | S7 | contract-block design doc (attended, five decisions + sequencing), refutation panel, your sign-off | `dev/local/designs/contract-block-design.md` approved |
| Thu 09-10 | S8 | `base_types.rs` split, `queries.rs` split (move-only diffs) | two commits; test-ci green |
| Fri 09-11 | S9 | 00172 as 2-3 items | closure actor in, enums deleted, e2e green |
| Fri 09-11 | S10 | 00173 as 2 items | one error table in core; GraphQL/REST/FFI/PgWire derive from it |
| Sat 09-12 | S11 | 00175, 00176 (design-doc order) | routed read/delete/search; conformance fixtures rewritten |
| next week | S12+ | 00197, 00177, 00178, v0.3.0, jink migration card; then the loop resumes on 00179-00199 | per design doc |
| any evening | S-meta (optional, ~1 h attended) | `autoclaude` in `~/git/src/github.com/buvis/claude-autopilot` over 00186-00189 and in `agent-skills` over 00075 | the loop's policy fixes land while ddb runs on the fast track (meta spend stays under the 30% rule: check the portfolio brief) |

If a session ends with a `blocked` item: it sits on `fast-track/<item>`; the next session starts by reading its review file and either re-running the item with the remaining findings folded into the card's Constraints, or asking you.

**Driver model per session.** The driving session's model matters more than it looks: every harness agent that does not pin a model (prep, gate, diff, report, the Bob/Carl dispatchers) and every lane inside the review-fanout child workflow (rita, cora, grace, toby, mallory, trent, victor) inherits the session model. A Sonnet driver therefore runs the consensus lanes on Sonnet; the loop runs its review phase on Opus. Only Eve (fable), Blake (sonnet), Victor (sonnet) and the implementor (card) are pinned.

| Sessions | Driver | Why |
|----------|--------|-----|
| S1, S7 | Fable 5.1 | S1 debugs an untested harness (novel failure modes, transcript reading, possible manual fallback); S7 is the five-decision design doc with exact signatures and a refutation panel: the highest-judgment work of the week |
| S2, S3, S4, S5, S8, S9, S10, S11+ | Opus 5 | card authoring from PRD phases (allowlists, constraints) and blocked-outcome triage are judgment; the fan-out lanes then run at the loop's review tier |
| S6, S-meta | Sonnet 5 acceptable | release script plus a docs item; autoclaude routes its own models |

Sonnet as a general driver becomes reasonable once the harness pins a `review_model` for the fan-out lanes; that needs a change in the plugin-shared `review-fanout.workflow.js` (golden-tested) and belongs to claude-autopilot PRD 00186, not to this week.

## 6. Session starting prompts (copy-paste; one per session)

Every prompt assumes: fresh session in the ddb checkout, plan file present, `git status` clean, the six hazard tests committed. Replace `<today>` with the date.

### S1: harness bring-up on FT-4, then FT-1

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1, 2 and 6 (this prompt) fully before acting; it is the whole context, do not run /catchup. Today: <today>.
Job: run fast-track items FT-4 then FT-1 through the harness ~/.claude/workflows/fast-track.workflow.js, fixing the harness where it breaks, and commit each item that comes back `ready`.
Per item:
1. Confirm `git status --porcelain` is empty. Write the spec card at dev/local/tmp/<item>-card.md in the exact format of §1 from the item's row in §2 (Goal = the Decision column in one paragraph; Tests = the file(s) with their #[ignore]d test names; Allowlist = the row's edit files plus the test file "(only: remove the #[ignore] attribute)" plus "CHANGELOG.md (only: one line under ## [Unreleased] ### Fixed)"; Read-only context = AGENTS.md is implicit, add ddb-core/src/error.rs and any file the row cites; Constraints = the Decision text plus "the test asserts the safe behavior; do not weaken it; no new public API; invariants I1-I10 per docs/src/technical/invariants.md hold"; Gate = cargo build, cargo +1.89.0 clippy --workspace --all-targets -- -D warnings, the item's `cargo test -p ddb-core --test <file stem>`; Model = sonnet).
2. Call the Workflow tool: scriptPath /Users/bob/.claude/workflows/fast-track.workflow.js, args {repo: "/Users/bob/git/src/github.com/doogat/ddb", item: "<item>", card_path: "<absolute card path>", plugin_root: "/Users/bob/.claude/plugins/cache/buvis-plugins/autopilot/0.5.1", fanout_script: "/Users/bob/.claude/workflows/review-fanout.workflow.js", date: "<today>"}. Watch it with /workflows. If it throws or a stage misbehaves, read the transcript journal it names, fix the harness script minimally (it lives in the buvis dotfiles repo: commit with `git --git-dir=~/.buvis --work-tree=~ add ':(top).claude/workflows/fast-track.workflow.js'` then commit), and resume with resumeFromRunId. If the harness cannot be repaired within ~30 minutes, run the same stages by hand per §1 "Manual fallback" and note it.
3. On `ready`: read dev/local/reviews/<item>-fast-track-review.md, confirm the test is un-ignored and green in the gate output, then commit with `git add -A && git commit` (no chaining: two calls) using `fix(<scope>): <what the user sees>` and the CHANGELOG line the card asked for. Do NOT push yet. On `blocked`: `git stash push -u -m <item>` and report the remaining findings; continue with the next item.
4. Append to the plan file §8 "Measurements": the item's wall time per stage (from /workflows), token totals, lane statuses (which of fanout/eve/blake/bob/carl ran), findings raised/confirmed/refuted, rework count.
End with the handoff block (Status / Changed / Not done / Next) naming the commits and what S2 must pick up.
```

### S2: FT-2, FT-3, FT-6; batch end

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1, 2, 6 and 8 fully; it is the whole context, do not run /catchup. Today: <today>. Check the S1 handoff in §8 and `git log --oneline -5`.
Job: run FT-2, FT-3 and FT-6 through the harness exactly as S1 did (same card format, same Workflow call, same commit rule), then close batch 1.
FT-6 card specifics: Allowlist = ddb-server/src/pgwire.rs, ddb-server/src/schema/mutations/operations.rs, ddb-core/src/service/sql.rs, ddb-core/src/error.rs (only: one structured error code), docs/src/technical/server.md, tests/e2e/hazard_server_txn_scope.rs (only: remove #[ignore]; make the BEGIN step accept a structured error as the safe outcome), CHANGELOG.md; Constraints = decision D1 from §2 verbatim, plus "in-process CLI and FFI transactions (DoogatDriver begin/commit/rollback) are unchanged; the error is a DoogatError::Structured code, redacted like every other structured error; executeBatch remains the server's atomic unit and its docs say so"; Gate adds `cargo build -p ddb-cli` and `cargo test -p ddb-e2e --test e2e hazard_server_txn_scope`; Transport impact = GraphQL and PgWire reject transaction verbs with the same code; REST/NoSQL/CLI/FFI unchanged; Model = opus.
Batch end (only when all three are committed or stashed): run `cargo test-ci` (foreground, timeout 600000; it takes ~16 min) and `cargo test -p ddb-e2e`; both green -> `git push origin master`, then /git-ferry:watch-ci until the Test workflow finishes. Record the batch-end timings in §8.
End with the handoff block naming commits, pushed sha, CI result, and anything stashed.
```

### S2 (revised after S1; use this one, driver = Opus 5)

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1, 2, 6 (the S2 blocks) and 8 fully; it is the whole context, do not run /catchup. Today: <today>.
Preflight: `git log --oneline -6` must show 4a248cb (FT-1) and bd3e7f0 (FT-4) on top of two 00171 commits; `git status --porcelain` must be empty; `ps -eo pid,command | rg run-autopilot` must show no session whose cwd (`lsof -a -p <pid> -d cwd`) is this checkout, because another session committed to master mid-item in S1; stop and ask me if one is here. `git --git-dir=~/.buvis --work-tree=~ log --oneline -3` must show 63d62bd4, cb585d8c, 0684771c; if that repo is still unpushed and the SSH agent refuses, tell me once and continue.
Job: run FT-2, FT-3 and FT-6 through the harness exactly as S1 did: card from the §2 row in the §1 format (copy dev/local/tmp/FT-4-card.md as the template), the §1 Workflow call, commit each `ready` item with `git add -A` then `git commit` as `fix(<scope>): ...`, stash a `blocked` one, one §8 line per item. FT-6 card specifics are in the original S2 block above; use them verbatim.
Learned in S1, apply per item: arm a Monitor on the run's transcript dir (newest *.jsonl mtime via GNU `stat -c %Y`, emit when older than 300 s) and read a stalled agent's transcript before resuming with resumeFromRunId; expect carl (copilot gemini) to time out at 540 s, record it, continue (D6); gate-agent seconds are cache hits, so rerun `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings` and the item's tests yourself before committing; read the final diff yourself and put residuals under "## Driver exit notes" in the review file and in §8; if a lane left a probe test in the allowlisted test file, keep it only as a named regression test with a real assertion, otherwise drop it.
Batch end (only when all three are committed or stashed): run `cargo test-ci` with run_in_background (it takes ~16 min, over the 600 s foreground cap) and run no other cargo until its notification arrives; then `cargo test -p ddb-e2e` (foreground, timeout 600000); both green -> `git push origin master`, then /git-ferry:watch-ci until the Test workflow finishes. Record batch-end timings in §8.
Also write, do not build, dev/local/tmp/FT-1b-card.md for S3: the raw FFI update lane's Body-zone UNIQUE residual (needs a hazard test first), the unset-column fold, and the FT-1 de-slop advisories, all listed in §8 under S1 FT-1.
End with the handoff block naming commits, pushed sha, CI result, and anything stashed.
```

### S3 (revised after S2; use this one, driver = Opus 5)

**Resume checkpoint, 2026-09-13:** Jobs A/B are implemented, independently
reviewed, and CI-verified at `7cea5e3`; Job C is authored below. The A/B stash
instructions and old preflight in this historical prompt are superseded by
`dev/local/reviews/stash-recovery-independent-review-2026-09-13.md`; do not
try to recreate or pop the removed stashes. The next implementation item is
FT-5 in a fresh session, using D2 and the original S3 FT-5 card scope. Verify
current Git state first. PRD 00206 is backlog work after its contract/pipeline
prerequisites, not the next implementation job.

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1, 2, 6 (the S3 blocks), 7 and 8 fully; it is the whole context, do not run /catchup. Today: <today>.

Preflight: `git log --oneline -4` must show 876a153 (FT-2) on top of 4a248cb and two 00171 commits, and `git status --porcelain` must be empty. `git stash list` must show stash@{0}=FT-6 and stash@{1}=FT-3; if either is missing, stop and ask me. Then check for a foreign session in THIS checkout: run `ps -eo pid,command | rg -i "claude|codex"` and `lsof -a -p <pid> -d cwd` on every hit that is a CLI session rather than an app helper process. A codex loop was working PRD 00171 in this checkout throughout S2 (pid 17017, cwd = this repo) and S2's preflight missed it because it grepped only `run-autopilot`. If any session's cwd is this checkout, stop and ask me before touching the tree: `git add -A` and `git stash -u` both sweep a foreign session's files into your item.

Job A, FT-6 stopgap (decision D1a, taken 2026-09-07; it supersedes D1's placement). `git stash pop stash@{0}` and keep every line of it: the any-position transaction-verb guard, the `detach_transaction` backstop that rolls back a buffer the engine still holds after execute/execute_batch, the two named regression tests, the error code, the docs and the un-ignored e2e test are all correct. The only thing to change is WHERE the guard applies. Give `DoogatService` a transaction-scope marker (`Exclusive` by default, `Shared` set by the server when it builds the actor) and run the rejection only when the service is `Shared`. `Exclusive` is the default deliberately: forgetting it preserves today's in-process behavior instead of silently dropping the guard on the server, and that failure mode is covered by a REQUIRED ddb-server test asserting the actor's service is `Shared`. Card: Model = opus; Tests = `rejects_a_transaction_verb_in_any_position`, `rejects_a_batch_that_leaves_its_transaction_open`, the new ddb-server test pinning `Shared`, and `tests/e2e/hazard_server_txn_scope.rs::open_transaction_on_one_connection_does_not_capture_another_clients_insert`; Allowlist = ddb-core/src/service/sql.rs, wherever `DoogatService` is defined (only: the scope field and its constructor plumbing), ddb-core/src/error.rs, ddb-server/src/actor/mod.rs, ddb-server/src/schema/mutations/operations.rs, docs/src/technical/server.md, the test files named above, CHANGELOG.md; Constraints = "raw BEGIN/COMMIT/ROLLBACK through `DoogatDriver::execute_sql` and `ddb sql` must behave EXACTLY as at 876a153; the FFI begin/commit/rollback_transaction methods stay untouched; the rejection is one `DoogatError::Structured` code produced in one place, never a per-transport switch; the error message, the CHANGELOG line and server.md must all describe a server-shared restriction, not a core-wide one"; Gate = cargo build, `cargo build -p ddb-cli`, clippy 1.89.0, `cargo test -p ddb-server`, `cargo test -p ddb-core --lib service::sql`, `cargo test -p ddb-e2e --test e2e hazard_server_txn_scope`. Commit as `fix(server): ...`.

Job B, FT-3 retry. Pop the FT-3 stash ONLY after Job A is committed: both stashes touch CHANGELOG.md and popping both at once conflicts, and the harness refuses a dirty tree anyway. Keep the cycle-detection fix it carries (the BFS spanning-tree `parent_of` map plus `is_ancestor`), which is correct and was verified empirically. Everything else is redone against a sharper card. S2's lesson: FT-3 failed because "reuse the planner, do not duplicate it" sat in Constraints prose and the implementor argued its way out of it with a true-but-irrelevant visibility fact. Put the hard requirements in Goal as numbered tasks a reviewer can check off: (1) lift the cascade planner into ONE crate-private helper that both `service/delete.rs` and `sql_engine/dml.rs` call, (2) run the junction cleanup BEFORE `remove_doogat` on the SQL path exactly as 876a153 did on the service path, (3) warn on every dropped typedef row, schema load and child-id decode failure, matching `indexer/materialize.rs`, (4) update `docs/src/technical/sql-engine.md` to name both cascade entry points. Model = opus; Gate = cargo build, clippy 1.89.0, `cargo test -p ddb-core --test hazard_sql_delete_cascade`, `cargo test -p ddb-core --lib delete`, `cargo test -p ddb-core --lib cascade`. Never put `--lib sql_engine` or `--lib service` in a subagent gate: they run 596 s against a 600 s cap. Run them yourself before committing. Commit as `fix(core): ...`.

Job C — authored 2026-09-13; planning only. (1) `dev/local/prds/backlog/00206-per-connection-transaction-sessions-v1.md` specifies private per-session SQLite/index state, commit conflict checks, authenticated token/connection ownership, bounded expiry, and the same CRUD transaction workflow across all six interfaces. It preserves request-local batch behavior and the actual DDL rollback boundary, and depends on the contract/write-pipeline/conformance prerequisites it names. Runtime implementation is not started. (2) `dev/local/tmp/FT-10-card.md` marks the old diamond-cascade card as superseded by `c57fa09`, with planner tests, both-path smoke coverage, independent probe evidence and passing CI. Do not schedule FT-10 or restore the rejected BFS ancestry approach.

Job D, only if budget remains: FT-5 per the original S3 block below, then its Job B leftovers. If A, B and C are done, stop and hand off. FT-5 is a full item and deserves a fresh session.

Apply per item, learned in S2. Arm a Monitor on the run's transcript dir at a 900 s threshold, NOT 300 s: a gate agent running cargo writes nothing to its transcript for ten minutes at a stretch, so 300 s fires constantly on false positives while 900 s caught the one real stall and produced a single false positive across three items. A real stall looks like this: an agent with no `result` line in journal.jsonl whose last transcript event is a `tool_use` with no matching `tool_result`, AND no matching live process in `ps`. That is a permission block with no responder, not a slow command, and waiting never clears it; the fix is `TaskStop` on the workflow then `Workflow({scriptPath: ..., resumeFromRunId: ...})`, which replays completed agents from cache and re-runs only the hung one. Treat any gate figure under about one second as a cache hit, and rerun `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings` and the item's own tests yourself before committing. Expect carl (copilot gemini) to fail roughly half the time in two shapes, `timeout 540` exit 124 or exit 0 with an empty review body; record per D6 and continue. Read the final diff yourself and put residuals under "## Driver exit notes" in the review file and in §8.

Batch end, only when every started item is committed or stashed: run `cargo test-ci` with run_in_background (about 16 minutes, over the 600 s foreground cap) and run no other cargo until its notification arrives; then `cargo build -p ddb-cli` and `cargo test -p ddb-e2e` in the foreground; then `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings`. All green -> `git push origin master`, then /git-ferry:watch-ci until the Test workflow finishes. The SSH agent intermittently refuses to sign; if it does, tell me once and stop rather than retrying in a loop.

End with the handoff block naming commits, pushed sha, CI result, and anything stashed.
```

### S3 (original): FT-5 and the pre-flight leftovers

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1, 2, 5, 6 and 8; do not run /catchup. Today: <today>. Check the S2 handoff and `git log --oneline -5`.
Job A: FT-5 through the harness. Card specifics: Model = opus; Allowlist = ddb-core/src/service/update.rs, ddb-core/src/git_ops/mod.rs (only: expose the re-entrant write lock to the service crate-privately, e.g. a pub(crate) fn that runs a closure under with_write_lock, no public API), tests/e2e/hazard_concurrent_single_row_update.rs (only: remove #[ignore]), CHANGELOG.md; Read-only context = ddb-core/src/git_ops/write_lock.rs, docs/src/technical/invariants.md, docs/src/technical/git-ops.md; Constraints = decision D2 from §2 verbatim plus "SQLite-outer/git-inner lock ordering (git_ops/mod.rs:306-309) must hold; never hold the git write lock across a network or subprocess call; batch update and SQL UPDATE are out of scope unless the same helper covers them for free"; Gate = build, clippy 1.89.0, `cargo test -p ddb-core --lib service`, `cargo build -p ddb-cli`, `cargo test -p ddb-e2e --test e2e hazard_concurrent_single_row_update`, `cargo test -p ddb-e2e --test e2e integration_write_lock_races`. Commit on `ready`.
Job B (attended, no harness): (1) one timed run: stub the 100 ms sleep in ddb-core/src/parser/mod.rs generate_unique_id under cfg(test) in a scratch branch, run `cargo test-ci`, record the core-suite seconds next to the 952.8 s baseline in §8, then discard the branch (the real removal is held PRD 00201; 00164's SM1/SM2 tests depend on the reserved window); (2) write a one-line note in §8 on whether `--test sync_test` should join the per-item gate for git_ops items (it is in FT-7's card already); (3) two CI-flake pins as one small item "ci-flakes" (sonnet): the e2e server readiness/port race behind "Connection reset by peer" (tests/e2e/common.rs:41 bind-then-release TOCTOU and the readiness wait around common.rs:56-73 and :180-190: make the request helper retry a refused/reset connection for up to 5 s, and pick ports from a per-process range) and the Windows-only empty `tracing` capture in ddb-core/src/git_ops/tests.rs:3611-3643 (replace the thread-local `with_default` capture with a subscriber that survives callsite-interest caching, e.g. a global `tracing_subscriber` set once per test binary with a shared buffer, or assert on the returned MergeCommitOutcome instead of log text); Gate = `cargo test -p ddb-core --lib git_ops`, `cargo test -p ddb-e2e --test e2e integration_warnings_update` run three times; (4) create the card for a small item "00193 verb-disposition registry test" (a test that fails on an unregistered AppCommand variant, per backlog 00193's registry feature) and run it through the harness if time allows.
End with the handoff block.
```

### S4: FT-7a and FT-7b (the live 00168 CRITICAL)

**2026-09-26 preparation supersedes the execution recipe below.** Read
`dev/local/designs/00203-bundle-import-ref-namespace-preflight.md` and the
prepared `dev/local/tmp/FT-7{a,b}-card.md` files first. They keep the complete
core fix in FT-7a; FT-7b tests both existing public interfaces. Abort scope is
still pending, so these are draft cards, not an instruction to execute the old
CLI-only recovery design. Preserve unrelated actor tests/.kiro edits. End each
build session before the full fresh review roster; the old one-call harness
example does not override that rule. No FT-7 runtime work or gates have run.

Historical recipe, retained for provenance (do not execute unchanged):

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1, 3, 6 and 8, then dev/local/prds/backlog/00203-bundle-import-ref-namespace-v1.md in full; do not run /catchup. Today: <today>. Check the previous handoff and `git log --oneline -5`.
Job: implement 00203 as two fast-track items. FT-7a = the PRD's Phase 0 tasks (per-import ref namespace, delivered-ref merge target, fetch.prune-independent behavior, unit tests in ddb-core/src/bundle/tests.rs). FT-7b = Phase 1 and 2 (abort surface, the CRITICAL reproduction e2e tests/e2e/bundle_import_ref_namespace.rs registered in tests/e2e/main.rs, docs, hold/00168 note, CHANGELOG). For each item write the card from the PRD's tasks: Tests = the test names the PRD's acceptance lines name (write them first through the harness's implementor if they do not exist: put "the tests named in ## Tests do not exist yet; write them first, fail-first, then make them pass" in Constraints); Allowlist = the PRD's Repository Structure files for that phase; Model = opus; Gate = build, clippy 1.89.0, `cargo test -p ddb-core --lib bundle`, `cargo test -p ddb-core --test sync_test`, `cargo build -p ddb-cli`, `cargo test -p ddb-e2e --test e2e bundle`. Commit each `ready` item as `fix(bundle): ...` with the CHANGELOG line.
After FT-7b: `cargo test-ci`, `cargo test -p ddb-e2e`, push, /git-ferry:watch-ci. Move the hold PRD note per the PRD's task; move 00203 to dev/local/prds/done/ with `mv`.
End with the handoff block.
```

### S5: FT-8a/b and FT-9a/b

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1, 3, 6 and 8, then dev/local/prds/backlog/00204-sync-collision-loser-without-frontmatter-v1.md and 00205-index-eviction-by-indexed-identity-v1.md; do not run /catchup. Today: <today>. Check the previous handoff and `git log --oneline -5`.
Job: implement 00204 as FT-8a (core fold + SyncReport field + unit tests; opus) and FT-8b (CLI/GraphQL/FFI renderers, e2e in tests/e2e/collision.rs, docs, CHANGELOG; sonnet), then 00205 as FT-9a (capture indexed identity + both eviction legs + the four regression tests; sonnet) and FT-9b (evict_doogats fail-loud, rustdoc, docs, CHANGELOG; sonnet). Cards come from each PRD's phases exactly as S4 did; gates: 00204 items add `cargo test -p ddb-core --test sync_test` and `cargo test -p ddb-e2e --test e2e collision`; 00205 items add `cargo test -p ddb-core --lib indexer` and `cargo test -p ddb-e2e --test e2e poison_file_reindex`. Commit each `ready` item; move each PRD to done/ when both of its items are in.
Batch end: `cargo test-ci`, `cargo test -p ddb-e2e`, push, then `gh workflow run full-validation.yml` and /git-ferry:watch-ci on it.
End with the handoff block.
```

### S5 (revised 2026-09-29 after FT-7; use this one, driver = Opus 5.5)

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1, 3, 6 (S5 blocks) and 10, then dev/local/prds/backlog/00204-sync-collision-loser-without-frontmatter-v1.md and 00205-index-eviction-by-indexed-identity-v1.md in full, and dev/local/reviews/FT-7b-fast-track-review.md plus dev/local/tmp/FT-7b-card.md as the current card/review template. Do not run /catchup. Today: <today>.
Preflight (read-only; stop and report on any mismatch): master == origin/master == `git ls-remote origin refs/heads/master` == a3e2dbc (FT-7b ab14498 + eec6884 00172 phase-0 closure transport + ac1dcbd kiro spec + a3e2dbc docs/dev placeholders) or a descendant you can explain; `git status --porcelain` empty; `git worktree list` shows only the main checkout; no stashes. No foreign session in this checkout (`ps -eo pid,command | rg -i "claude|codex"` + `lsof -a -p <pid> -d cwd`).
Job 1 (cards): write dev/local/tmp/FT-8a-card.md, FT-8b-card.md, FT-9a-card.md, FT-9b-card.md from the PRD phases per §3 (FT-8a opus: core fold + SyncReport field + unit tests; FT-8b: CLI/GraphQL/FFI renderers, e2e, docs, CHANGELOG; FT-9a: capture indexed (id,type) + both eviction legs + regression tests; FT-9b: evict_doogats fail-loud, rustdoc, docs, CHANGELOG). Each card: FT-7b headings, six-interface Transport impact, explicit allowlist, fail-first replay requirement, gates (00204 adds `--test sync_test` + e2e `collision`; 00205 adds `--lib indexer` + e2e `poison_file_reindex`). Check each against docs/src/architecture/design-decisions.md and invariants.md. If a PRD needs a scope decision (like FT-7's abort deferral), ask me one question and stop.
Job 2 (build FT-8a only): worktree dev/local/tmp/worktrees/00204-ft-8a on branch fast-track/ft-8a from master. Fresh Opus Ivan (Agent, autopilot:ivan) gets only the card's Tests, Allowlist, Constraints, Read-only context; it runs no cargo. Driver runs all Cargo sequentially: Rust 1.89.0, `--locked`, `--manifest-path <worktree>/Cargo.toml`, `--target-dir /Users/bob/git/src/github.com/doogat/ddb/target`; test-ci as the expanded alias with run_in_background and no other cargo meanwhile; e2e via `env CARGO_TARGET_DIR=...` after `build -p ddb-cli`. Fail-first replay in a separate detached baseline worktree (never committed; a setup/shape failure is not bug evidence). Logs to scratchpad then `cp` into dev/local/reviews/FT-8a-logs/. rustfmt only the touched leaf files before the final gates.
Then record dev/local/reviews/FT-8a-build.md, dev/local/tmp/FT-8a-state.json, the candidate patch + SHA-256, a §10 entry, and a fresh-review handoff in docs/dev/tmp/, and END the session. No review, commit or push in the build session; no FT-8b/9, S3 leftovers, PRD 00206 or release.
End with the handoff block.
```

### S5b: FT-8b build (written 2026-10-02 after FT-8a landed as d7867d2; driver = Opus 5.5)

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1 and 10 (latest entries), then dev/local/tmp/FT-8b-card.md (binding), dev/local/prds/backlog/00204-sync-collision-loser-without-frontmatter-v1.md in full (incl. "Deferred work"), dev/local/reviews/FT-8a-fast-track-review.md, and dev/local/reviews/FT-8a-build.md as the build-record template. Do not run /catchup. Today: <today>.
Preflight (read-only; stop and report on any mismatch): master == origin/master == `git ls-remote origin refs/heads/master` == d7867d2 or a descendant you can explain; `git status --porcelain` shows nothing except the untracked docs/dev/project-management/ mirror of dev/local/ (seen 2026-09-30; leave it untouched); `git worktree list` shows only the main checkout; no stashes. No foreign session in this checkout (`ps -eo pid,command | rg -i "claude|codex"` + `lsof -a -p <pid> -d cwd`).
Card refresh: the card was prepared at a3e2dbc; re-base it to the current master, re-check every cited file:line and premise against the code, fix drift in the card and flag it. If a scope decision is needed, ask me one question and stop.
Build FT-8b only: worktree dev/local/tmp/worktrees/00204-ft-8b on branch fast-track/ft-8b from master. A fresh Ivan (Agent, autopilot:ivan, model per the card) gets only the card's Tests, Allowlist, Constraints and Read-only context; it runs no Cargo and no ddb. Walkthroughs (00203 re-record, new 00204) go through the showboat CLI only, never hand edits; verify with dev/bin/safe-showboat-verify, never from the project root.
Driver runs all Cargo sequentially: Rust 1.89.0, `--locked`, `--manifest-path <worktree>/Cargo.toml`, `--target-dir /Users/bob/git/src/github.com/doogat/ddb/target`; test-ci as `test --workspace --exclude ddb-e2e --exclude ddb-uniffi-bindgen --lib --bins` with run_in_background and no other Cargo meanwhile; e2e via `env CARGO_TARGET_DIR=...` after `build -p ddb-cli`. Fail-first replay of the new tests in a separate detached baseline worktree at master (never committed; a shape/compile failure is not bug evidence). rustfmt only touched hunks (never workspace `cargo fmt`), then `git diff --check`. Logs to the scratchpad, then `cp` into dev/local/reviews/FT-8b-logs/. Lane context files for a later review go under dev/local/tmp/, not the scratchpad (Carl cannot read it).
Then record dev/local/reviews/FT-8b-build.md, dev/local/tmp/FT-8b-state.json, the candidate patch + SHA-256, a §10 entry, and a fresh-review handoff in docs/dev/tmp/, and END the session. No review, commit or push in the build session; no FT-9a/9b, S3 leftovers, PRD 00206 or release. PRD 00204 stays in backlog until FT-8b is reviewed and landed.
End with the handoff block.
```

### S6: release v0.2.8 and the docs corrections item

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1, 3, 5, 6 and 8; do not run /catchup. Today: <today>. Check the previous handoff, `git log --oneline -8`, and `gh run list --limit 5` (Test and Full Validation must be green at HEAD; if not, fix first).
Job A: cut v0.2.8 with dev/bin/release only (never gh release create); follow the script's prompts; verify the tag push triggered the release workflow and that artifacts appear; note the version in §8.
Job B: one fast-track item "docs-corrections" (sonnet): Allowlist = docs/src/architecture/design-decisions.md (:177 "nine invariants I1-I9" -> ten, I1-I10), docs/src/technical/transport-policy-inventory.md (:26-29 REST "Specialized" and NoSQL writes "intentionally absent" contradict design-decisions.md:18; say what is true today and point at 00192), docs/src/guide/app-contract.md (:147,:156 says GraphQL warning forwarding is deferred; it is live: ddb-server/src/warning_extension.rs), docs/src/technical/ffi.md (the UDL is 7 methods stale: mark it non-authoritative or regenerate the list from ffi/driver.rs), docs/src/technical/mobile-constraints.md (:197-201 claims ~20-30 ms creates; performance.md:87 measures 972 ms), docs/src/technical/sql-engine.md (CASCADE on DELETE FROM now holds after FT-3: state both entry points). Tests = none (docs item: Constraints say "every claim edited must cite the file:line that proves it; no new claims"); Gate = `cd docs && mdbook build` (one Bash call with cwd docs, timeout 300000) and clippy is not needed. Commit as `docs: ...`; no CHANGELOG line. Push.
End with the handoff block.
```

### S7: the contract-block design doc (attended)

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1, 4, 6 and 8, then dev/local/audit-results/refactor-assessment-2026-09-06.md sections F5, F7, F8, F9 and "Decision", then dev/local/prds/backlog/00172-collapse-actor-plumbing-v1.md, 00173-unify-server-error-mapping-paths-v1.md, 00175-route-read-delete-through-contract-v1.md, 00176-route-search-through-contract-v1.md, 00177-ffi-crud-through-contract-v1.md, 00178-structured-fields-input-v1.md, 00197-derive-transport-renders-from-core-types-v1.md; do not run /catchup. Today: <today>.
Job: write dev/local/designs/contract-block-design.md with me in the loop. It records five decisions with the exact types and signatures: (1) closure actor (ActorHandle::call<R>, explicit EventIntent, and a core-first schema-changed signal on SqlResult / SchemaApplyReport / SyncReport with the reload trigger on the ActorHandle side, because the actor thread cannot await the reload loop); (2) the error spec table in ddb-core app_contract (code -> category, HTTP status, redaction, FFI hint) that server error.rs, http_error.rs, ffi/records.rs and pgwire.rs derive from; (3) AppOutput on read/list/search/delete/batch and the classification of every transport-called service method as public verb or specialized surface, including typed filtered list and where the where-compiler lives; (4) JSON fields + a core-owned flat, UniFFI-representable DoogatView + one core column-extraction chokepoint replacing base_types.rs:419-467, materialize.rs:1190-1243 and consistency/mod.rs:124-139, with uniffi::remote scoped to flat report types; (5) server transaction scope (per-connection session handle vs the FT-6 rejection kept as the contract). It also fixes sequencing: base_types.rs and queries.rs split first; 00197 after 00176; whether 00196's core legs run before 00175/00177/00178; 00190's staleness probe with 00175's read leg; the 00186 capabilities-registry hub; the internal test rewrites (108 service/tests.rs call sites, raw_ffi_semantics.rs, ffi/tests.rs) as acceptance of 00175/00177.
Method: draft each decision as 3 options with benefit/drawback and ask me one AskUserQuestion per decision; then run /review-design-doc on the file; then run a refutation panel (a Workflow with three skeptics: architecture, oracle/testability, downstream-jink; and a judge) and apply upheld amendments; then ask me for sign-off. Write the item list for S8-S12 into the design doc's last section (each item: goal, tests, allowlist, model).
End with the handoff block.
```

### S8: split base_types.rs and queries.rs

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1 and 6, then dev/local/designs/contract-block-design.md (the item list) and dev/local/prds/done/00156-split-oversized-core-modules-v1.md (the move-only method); do not run /catchup. Today: <today>.
Job: two fast-track items, move-only: split ddb-server/src/schema/base_types.rs (1,677 lines; keep the inline test module with the code it tests) and ddb-server/src/schema/queries.rs (893 lines, zero tests) per the design doc's grouping. Tests = none new; Constraints = "move-only: the acceptance is `git diff --stat` showing pure relocation and every existing test green; diff the moved logic, not the outputs (00156 method); no behavior change; keep pub(crate) visibility minimal"; Gate = build, clippy 1.89.0, `cargo test -p ddb-server`, `cargo build -p ddb-cli`, `cargo test -p ddb-e2e --test e2e serve`. Model = sonnet. Commit each as `refactor(server): ...` (no CHANGELOG). Push at the end after `cargo test-ci`.
End with the handoff block.
```

### S9 and S10: 00172, then 00173

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1 and 6, then dev/local/designs/contract-block-design.md and the PRD (00172 for S9, 00173 for S10); do not run /catchup. Today: <today>.
Job: run the PRD as the items the design doc lists (S9: closure call beside the enums with a unit test; verb migration + EventIntent + reload signal; enum deletion + docs. S10: the table in core with contract tests; the four transport derivations + PgWire redaction). Cards from the design doc's item list; Model = opus for the migration items, sonnet for the deletions; Constraints always include the byte-identical obligation ("every existing server/e2e test passes unchanged; diff the moved logic"); Gate = build, clippy 1.89.0, `cargo test -p ddb-server`, `cargo test -p ddb-core --lib app_contract`, `cargo build -p ddb-cli`, `cargo test -p ddb-e2e --test e2e serve`, `cargo test -p ddb-e2e --test e2e integration_error_codes` (S10). Commit each `ready` item; mv the PRD to done/ when its items are in; `cargo test-ci` + e2e + push at the end; move the hold/ and backlog references the PRD names.
End with the handoff block.
```

### S11 and later: 00175, 00176, 00197, 00177, 00178

```text
Read dev/local/plans/fast-track-2026-09-06.md sections 1, 4 and 6, then dev/local/designs/contract-block-design.md and the PRD; do not run /catchup. Today: <today>.
Job: run the PRD as the design doc's items. These change wire shapes on purpose (the 2026-09-05 "beta, replace directly" decision), so for each item the card's Tests are the REWRITTEN conformance fixtures and e2e assertions (tests/e2e/conformance/*, the ~44-55 pinned lines the PRD names) written first by the harness's implementor under "write the tests first, fail-first" in Constraints; Blake's blind lens gets the PRD's Transport Impact section appended to the card. Gate as in S9 plus `cargo test -p ddb-e2e --test e2e conformance`. After 00178: `cargo test-ci`, e2e, push, Full Validation, then v0.3.0 with dev/bin/release and a jink migration card written to ~/git/src/github.com/doogat/jink/dev/local/tmp/ddb-0.3.0-migration.md listing the 7 call sites from §4.
End with the handoff block.
```

### S-meta (optional evening): let the loop improve itself

```text
In ~/git/src/github.com/buvis/claude-autopilot: run /review-prd-backlog on 00186-00189 (written 2026-09-06 from the ddb assessment; check_links must be clean for them), apply its fixes, then start `autoclaude` for that backlog. Same for ~/git/src/github.com/buvis/agent-skills PRD 00075. Do not touch ddb from this session. Watch the meta share of monthly spend (30% rule).
```

## 7. Decisions taken (defaults; tell the session if you want another)

- D1 (FT-6): reject transaction verbs on the shared server actor now; per-connection sessions are decision 5 of the contract design, not a fast-track item.
- D1a (FT-6, taken 2026-09-07 after S2; supersedes D1's placement only): the rejection stays in `DoogatService::execute_sql` but fires only when the service carries a `Shared` transaction-scope marker the server sets when it builds the actor. `Exclusive` is the default, so raw `BEGIN` through `DoogatDriver::execute_sql` and `ddb sql` keeps behaving exactly as at `876a153`, and forgetting the marker preserves in-process behavior instead of silently dropping the guard on the server. A required ddb-server test pins the actor's service as `Shared`. Reason: the hazard exists only where clients share one service, so that is where the guard belongs, and one code in one place still holds. Proper `BEGIN` support is not abandoned, it becomes PRD 00206 (S3 Job C), which lifts this rejection when it lands.
- D2 (FT-5): single-row update reads under the re-entrant write lock; an expected-OID refusal is the fallback; no optimistic-concurrency API yet.
- D3 (FT-4): raw create with an existing id is a Conflict error, never a re-mint.
- D4 (gate): `cargo test-ci` once per batch, not per item; a card may demand it per item.
- D5 (custody): an item whose CRITICAL/HIGH survives rework goes to a stash or `fast-track/<item>` branch, never master.
- D6 (lenses): every lane runs on every item; a lane that is unavailable (codex/gemini down) is recorded as such in the review file and never silently skipped; two unavailable lanes stop the batch until you decide.

## 8. Measurements and handoffs (sessions append here)

- 2026-09-06 baseline: `cargo test-ci` = 952.8 s core (two runs); e2e subsets 16-34 s; clippy 1.89.0 full 2m18s-4m30s; hazard-test authoring 6 agents / 0.67M tokens / 8 min; six hazard tests run once by one runner: all six FAIL on the hazard assertion (see the assessment F6/F9 and the test doc comments).
- 2026-09-06 CI state at plan time: Test green at `dbc128f` (the pinned tests). Nightly at `c8b72bd` (run 34020494801) red on two single-test flakes, Linux green: Windows `git_ops::tests::commit_merge_logs_distinct_reassignment_warn_per_loser` (tests.rs:3640, the `tracing` capture came back empty: a log-capture race, not a merge defect) and macOS `integration_warnings_update::integration_58_a_graphql_update_warnings_empty` (common.rs:186, "Connection reset by peer" from the test server: the known e2e readiness/port flake, ledger item "e2e port flake", no owner). S3 Job B item (3) below owns both; until they are pinned, judge Nightly by the failing test names, not the conclusion (memory: nightly green is not validation).
- Dotfiles: the harness commit `7f27975f` in `~/.buvis` is pushed (first attempt was refused by the SSH agent, the retry went through; expect the same intermittency on unattended pushes).
- S1 (2026-09-06, Fable driver) FT-4 `ready` after 1 rework; three harness runs on one run id (`wf_d29d3769-747`):
  - Run 1 (11:16-11:27, 672 s, 4 agents, 0.31M tokens): prep ~4 min, implement (sonnet) ~6 min, gate ~1 min, diff <1 min; threw at Review on an unfilled `{AGENT_NAME}` in the output-format section (harness fix `0684771c` in buvis).
  - Run 2 (11:28-16:43, 5.2 h wall, 16 agents, 0.72M tokens): fanout (6 agents), eve, blake, bob finished in 2-6 min each; carl's copilot/gemini helper outlived the 600 s Bash cap, was backgrounded, and the lane agent stalled; from 11:58 every newly spawned agent (carl retry x5, Victor x6, rework Ivan x6) produced zero model output for ~16 min each until the runtime gave up. Suspected cause: the host idled while unattended (not confirmed). Harness fix `cb585d8c`: `timeout 540` on the codex/gemini lanes, exit 124 = failed lane.
  - Run 3 (19:12-19:37, 1529 s, 21 agents, 1.12M tokens): child fan-out and bob re-ran (child workflows do not share the parent cache; bob's prompt changed), eve/blake cached; verify 2 candidates; rework 1; gate; 1 closure; report.
  - Lanes: fanout APPROVE (6 advisory, 0 blocking), eve ok (1 MEDIUM 4 LOW), blake ok (1 LOW), bob ok (1 CRITICAL, 1 HIGH, 1 MEDIUM, 2 LOW), carl failed (timed out at 540 s while running clippy and test-ci inside the review; recorded per D6, one lane down, batch continues).
  - Findings: raised 22, verified 2 (1 confirmed: pre-lock existence check TOCTOU; 1 refuted: test does not pin the Conflict variant, allowlist-bound), rework 1 (check moved inside the `BEGIN IMMEDIATE` window that wraps the commit), closed 1, remaining 0, advisory 15.
  - Driver re-ran clippy 1.89.0 (41 s, clean) and the hazard test (1 passed) before committing; the gate agent's 0.35 s clippy figure is not credible, treat gate seconds as approximate.
  - Total S1 subagent spend for FT-4: 2.15M tokens (0.6-1.5M budgeted); attended wall time excluding the stall: ~50 min.
  - Follow-ups worth a card (from Advisory): pin `DoogatError::Conflict` in the hazard test and refresh its stale header; rustdoc line on `create_doogat_raw`; FFI maps Conflict to `DdbError::Git` (00177); the same pre-lock TOCTOU exists on the mint path (`unique_id`, write_helpers.rs:245), fold into FT-5's card.
- S1 FT-1 `ready` after 1 rework, run id `wf_8353ee07-eca`, two harness runs:
  - Run 1 (20:05-21:13, stopped by the driver): prep/implement/gate/diff ~10 min; review fan 10 lanes ~25 min (carl timed out at 540 s: the copilot/gemini reviewer spends its budget running cargo instead of writing findings, twice now); verify 12 candidates in parallel ~8 min (11 confirmed, 1 refuted); rework implementor edited for ~15 min then ran `cargo test-ci` on its own twice (each would outlive the 600 s Bash cap); driver killed the run and the workflow, harness fix `63d62bd4` in buvis (rework instruction forbids suites), resumed.
  - Run 2 (21:13-21:35, 1307 s, 1.46M tokens for the resumed part alone; the stopped run's total was not reported): fresh rework implementor found the predecessor's fixes already in the tree, validated, gate, 11 closure checks, report.
  - Lanes: fanout CHANGES_REQUESTED (3 HIGH confirmed by its own Victor, 8 advisory), eve ok (3 HIGH, probe-proven out of repo), blake ok (1 CRITICAL, 1 HIGH refuted: a probe file another lane left in ddb-core/tests and later removed), bob ok (4 HIGH, 2 MEDIUM), carl unavailable. Total raised 33: 12 blocking candidates, 11 confirmed, 1 refuted, 21 advisory.
  - Rework closed: intra-batch UNIQUE collision (seen-key set in `batch_update`), Body-zone columns missing from the merged set (row snapshot fold), unchecked raw FFI update lane, BEGIN IMMEDIATE now also wraps unique-bearing typedef updates, hyphenated columns no longer skip the check.
  - Driver exit: reran clippy 1.89.0 (clean, 14 s) and the hazard tests (3 passed); kept the first rework's unstaged intra-batch probe as a real regression test (renamed, asserts UNIQUE_VIOLATION and both rows kept) since no allowlisted test covered that fix. The gate agent's 0.55 s build / 0.29 s clippy figures are cache hits, not measurements.
  - Residuals the roster did not raise (driver read, suspected, not probed): (1) `update_doogat_raw` folds the row's OLD materialized value for any group column absent from `parsed_fields(desired)`, so a raw update that changes a Body-zone (TEXT) UNIQUE column can still evict the sibling; needs a hazard test and a body-zone extraction on the raw lane. (2) An update that unsets a group column gets that column's old value folded back in, which can only false-reject (loud, safe). (3) Roster advisories worth one card: merged-set construction duplicated across three lanes, `check_effective_unique_fields` duplicates `check_unique_constraints`, no self-exclusion or composite-group test, SQL `UPDATE` via sql_engine unchecked.
  - Concurrency hazard: two 00171 commits (`103c8ab`, `74644b8`, e2e scenario restoration) landed on master from another session between FT-4 and FT-1's commit while FT-1's tree was dirty. No file overlap this time; the harness's `git add -A` would have swept a colliding session's files into the item. Do not run the fast track while an autopilot batch targets this checkout.
  - Cost: FT-4 2.15M + FT-1 >1.46M subagent tokens; the 0.6-1.5M budget holds only for a run with no stall and no rework.
- S2 (2026-09-06/07, Opus 5 driver): 1 of 3 items landed. FT-2 committed `876a153`; FT-3 and FT-6 both came back `blocked` and are stashed per D5 (`stash@{0}` = FT-6, `stash@{1}` = FT-3; both touch CHANGELOG.md, so pop them one at a time). Batch total: 7.18M subagent tokens across 111 agents, about 4.5 h wall.
  - **FT-2 `ready`** after 1 rework, run `wf_bd6fa552-5ac`, one clean run (22:29-23:56, 5197 s, 22 agents, 2.19M tokens, implementor sonnet). Gate: build 29 s, clippy 118 s, hazard test 9 s, `--lib sql_engine` **596 s**, `--lib indexer` 3 s. Lanes: fanout APPROVE, eve ok, blake ok, bob ok, carl failed. Findings: 2 blocking (both bob HIGH), both confirmed by Victor, 0 refuted, 27 advisory, rework closed both, 0 remaining. Driver re-ran clippy (0.57 s, a cache hit) and the tests (hazard 1 passed, `--lib indexer` 266 passed) before committing.
  - **The 596 s `--lib sql_engine` figure is the single most useful number this session.** It is a real measurement, not a cache hit, and it sits 4 s under the 600 s subagent Bash cap. FT-3's card replaced it with the `delete` (138 s) and `cascade` (56 s) name filters, which covered the same blast radius at a third of the time and never approached the cap. Do that on every future core item; never put `--lib sql_engine` or `--lib service` in a subagent gate.
  - **FT-3 `blocked`**, run `wf_f6791c8d-dbd`, 4 confirmed HIGH remaining after 1 rework (7 before, 3 refuted, 40 advisory; fanout CHANGES_REQUESTED, eve/blake/bob ok, carl unavailable at `timeout 540` exit 124). Gate all green (build 64 s, clippy 90 s, hazard 13 s, delete 138 s, cascade 56 s) and green did not matter: every blocker is a spec deliverable or a data-integrity defect no gate command exercises. What blocked it: the card said "do not write a second cascade walker" and allowlisted `service/delete.rs` so the planner could be lifted to a shared helper, and the implementor hand-copied the walker into `sql_engine/dml.rs` anyway, citing a true visibility fact (`service::delete` is a private mod) as if it were a dead end; the copy had already drifted from its source before review. Plus the mandatory `sql-engine.md` update was never written, cascade children can be dropped silently, and `remove_doogat` still runs before the fallible junction cleanup (the exact ordering hazard `876a153` had fixed on the service path one commit earlier). The rework DID correctly fix the cycle-detection false positive, which is why this is a stash and not a discard.
  - **New live bug found by FT-3's roster, not in its scope: the diamond cascade graph is rejected as `CASCADE_CYCLE` on the SERVICE path too.** A legal delete of a parent whose child is reachable through two CASCADE columns is permanently refused today through `ddb delete`, GraphQL, REST and FFI. Eve reproduced it on both paths. This is on master right now and needs its own card with a hazard test; the stashed FT-3 work carries the fix shape (BFS spanning tree plus an `is_ancestor` check) for the SQL copy only.
  - **FT-6 `blocked`**, run `wf_9a639fa1-d97` (4512 s, 54 agents, 4.80M tokens, implementor opus), 1 HIGH remaining out of 16 before rework (4 refuted, 28 advisory). All five lanes ran; carl came back and finished in 5m29s with a matching CRITICAL. The fifteen closed blockers were all one real defect that every lane found independently: the first guard inspected only `statements.first()`, so `SELECT 1; BEGIN` walked past it, and `execute_batch` had no guard at all. The rework fixed both properly and added two named regression tests. The survivor is a scope violation: the guard sits in `DoogatService::execute_sql`, which `DoogatDriver::execute_sql` also calls, so raw `BEGIN` through FFI and `ddb sql` now fails, while D1 and the card both scoped the change to the shared server actor and the new error message plus the CHANGELOG line still claim it is server-only. **Decided attended 2026-09-07 as D1a**: keep the guard in `DoogatService::execute_sql` but fire it only when the service carries a `Shared` marker the server sets, `Exclusive` by default, with a required ddb-server test pinning `Shared`. Retry is S3 Job A. The maintainer also asked what proper `BEGIN` support would cost; the answer, read off the code, is one SQLite connection per session (`SAVEPOINT ddb_txn` is a fixed name on a stack shared by one `Connection`, `sql_engine/transaction.rs:25`, so two sessions cannot be isolated), commit-time conflict detection (two buffers flush independently and lose updates silently), and a session token with idle/disconnect expiry for GraphQL, which has no connection to key on. That is PRD-sized, so it becomes 00206 (S3 Job C) and lifts the D1a rejection when it lands. Full options with benefit/drawback in `dev/local/reviews/FT-6-fast-track-review.md` under "Driver exit notes".
  - **Harness stall, new failure mode, cost about 25 min.** A Victor verifier on FT-3 issued `rg -n "..." <file> | Read`, piping shell output into a tool name. No process ever spawned (`ps` was empty), so it was not a slow command: the call sat blocked at the permission layer for 17 min with no responder, past the 600 s Bash cap. Fix was `TaskStop` on the workflow then `resumeFromRunId`, which replayed the ~30 completed agents from cache and re-ran only the hung verifier, for 246 s and 0.19M tokens. Watch for a pending agent whose last transcript event is a `tool_use` with no matching `tool_result` and no live process; that signature means a permission block, not a long command, and waiting will not clear it.
  - **Stall-monitor tuning.** The 300 s threshold from S1 fires constantly on false positives, because a gate agent running cargo writes nothing to its transcript for ten minutes at a stretch. 900 s is the right threshold for this harness: it caught the real FT-3 stall at 918 s and produced one false positive across three items (a 596 s cargo run).
  - **Carl remains unreliable, not intermittent**: failed on FT-2 (exited 0 after 45 s having read all three inputs, emitted 158 output tokens, wrote no review body) and on FT-3 (`timeout 540`, exit 124, no output file), then ran fine on FT-6. Three failures in five fast-track items across S1 and S2. D6 keeps the batch running on one lane down, but the roster should stop being described as five lanes.
  - **Card-writing lesson.** FT-2 and FT-6 both landed their real work; FT-3 did not, and the difference is where the hard requirement lived. In FT-3 the "reuse the planner, do not duplicate it" instruction was a Constraints sentence, and the implementor treated it as advice it could argue its way out of. Requirements that a lane must be able to check (a shared helper exists; a named docs paragraph changed) belong in Tests or as numbered tasks in Goal, not in Constraints prose.
- S3 stash recovery, 2026-09-13 (Codex; manual implementation followed by a fresh independent review, as authorized): FT-6 `2f04f3a`; FT-3 `c57fa09`; both pushed to origin/master. Build, all-target Clippy, 1846 fast tests, targeted hazards, Docs CI, and Test CI (34752919620, test + coverage) passed. Evidence: `dev/local/reviews/stash-recovery-2026-09-09.md`; fresh-review handoff: `dev/local/tmp/handoff-20260913-1255.md`. D1a retained. The FT-3 BFS ancestry assumption is superseded by a tested iterative DFS: cross-sibling edges can hide a real cycle in the spanning-tree check. Replaced stashes, implementation drafts, and the merged orphan branch removed; working tree clean. Independent review remains pending in a fresh session. Jobs C/D and later sessions remain future work.
- S3 independent review, 2026-09-13 (fresh Codex session): reviewed `876a153..c57fa09` against Jobs A/B and original blockers; no functional findings. Corrected misleading DDL rollback/atomicity guidance in `7cea5e3`, pushed to origin/master. Fresh build, all-target Clippy `-D warnings`, 1846 fast tests (core 1038.06s), 35 cascade tests, independent CLI diamond/cycle/buffered-reference probes, mdbook, scoped rustfmt, and diff checks passed. New-push CI was not awaited. Jobs A/B review is complete; Jobs C/D and later work remain unexecuted, so this plan is retained. Review: `dev/local/reviews/stash-recovery-independent-review-2026-09-13.md`.
- S3 Job C, 2026-09-13 (Codex, planning only): created backlog PRD `00206-per-connection-transaction-sessions-v1.md` using the create-prd standard template and an opus floor for concurrency/contract work. Two explicit authoring assumptions: conservative HEAD-change rejection and a persistent CLI invocation for incremental transactions. All six interfaces share the golden CRUD workflow; separate SQLite connections alone are insufficient, so each session owns a private derived index. FT-10 is superseded in `dev/local/tmp/FT-10-card.md`. Corrected stale transaction guidance in backlog 00180/00193 so neither lifts FT-6's guard; documented D1a's planned evolution in `docs/src/architecture/design-decisions.md`. Test (34767218504, including test/coverage) and Docs (34767218556, build/deploy) passed for `7cea5e3`; recorded in the independent review. No runtime code, cargo execution, or FT-5 work in this planning task. Retain this plan; later jobs remain unexecuted.
- Job C authoring checks: standard-template heading order and Feature/Module fields passed; all tasks have dependency/acceptance text; 00206 is unique across all PRD states (no discovery directory); 185 lines, two marked assumptions; whitespace checks passed. No builds/tests run. The tracked change to `docs/src/architecture/design-decisions.md` remains uncommitted; all other Job C artifacts are in gitignored `dev/local/`. Account for this known docs edit before FT-5's clean-tree preflight.
- PRD 00206 follow-up, 2026-09-13: maintainer answered "yes" to rejecting COMMIT whenever Git HEAD changed since BEGIN, including unrelated changes. Recorded as settled in the PRD and architecture decision note; read-only/empty transactions validate the same baseline without writing a Git commit. One authoring assumption remains: persistent CLI invocation, to ask separately as requested. No runtime work started.
- PRD 00206 CLI follow-up, 2026-09-13: maintainer answered "yes" to one running `ddb` process owning an interactive/scripted transaction, with rollback of uncommitted work on exit. Recorded in the PRD and architecture decision note; no cross-invocation transaction persistence. Both authoring assumptions are now confirmed and no unresolved questions remain. Planning only; FT-5 still requires its fresh implementation session.
- Planning handoff, 2026-09-13 19:18 CEST: maintainer authorized committing the decision note and preparing FT-5's fresh-session handoff. Committed `4fd6052 docs: record transaction session decisions`; `mdbook build docs` and staged/unstaged whitespace checks passed. Working tree clean, no stashes, one worktree; local master is one commit ahead of origin/master (`7cea5e3`). `4fd6052` is not pushed and has no CI result. The confirmed PRD assumptions and local planning artifacts are retained. Continue with `dev/local/tmp/handoff-20260913-1918.md`; FT-5 is authorized for a fresh build session followed by independent review. No FT-5 implementation or Rust test run occurred in this handoff task.
- FT-5 preflight, 2026-09-13 19:43 CEST (Codex; build not yet started): verified clean master at `4fd60524ca3aed2ff633ee9716b14cd8fb19d035`, no stashes, one worktree. Process/cwd inspection found no foreign CLI session in this checkout. The Workflow tool is unavailable in this runtime; the manual lane is required. A fresh read-only Opus 5 preflight confirmed the original card's visibility-only proposal cannot reach `GitRepo::with_write_lock` through generic `G: GitBackend`; expected-OID fallback also needs a trait extension. Additionally, the existing `commit_file -> commit_files` path invokes commit-graph and threshold maintenance while holding the lock, conflicting with the card's no-subprocess constraint. Proposed minimal exception in `dev/local/tmp/FT-5-scope-proposal.md`: one REQUIRED `GitBackend` lock hook (no unsafe unlocked default), implementation/deferred single-file maintenance in `git_ops/mod.rs`, and compatibility/lock docs. Maintainer approval was requested and is pending; D2 itself is unchanged. No runtime edits, implementation dispatch, review, commit or push occurred. Baseline `cargo +1.89.0 build -p ddb-cli` passed (3m37s), and the unchanged ignored hazard was explicitly run with `-- --ignored`: it FAILED on the intended Git durability assertion in all 6 rounds (2.66s execution; 23.70s compilation), proving the bug remains at this exact baseline. Logs: `dev/local/reviews/FT-5-logs/baseline-{build,hazard}.log`; isolated Opus prompt/result: `dev/local/tmp/FT-5-opus-preflight.{txt,log}`, session `2b5a0a1e-347b-4c9c-843c-80ca0647b6e7`. Required post-change gates and every fresh review lane remain unexecuted. Resume this scope decision before dispatching the fresh Opus implementor; retain the plan and all evidence.
- FT-5 scope approval, 2026-09-13: maintainer answered "proceed" to the minimal backend hook / maintenance / docs exception in `dev/local/tmp/FT-5-scope-proposal.md`. The approved card is `dev/local/tmp/FT-5-card.md`; D2 remains unchanged. Fresh Opus test-authoring dispatch started before runtime implementation, session `91f6846a-e7f9-46cc-8430-9542a0cdb0a8`. Driver owns all sequential cargo execution. Build must hand off and end before the fresh complete review roster; no review-phase invocation in the work session.
- FT-5 build quota handoff, 2026-09-13 20:25 CEST: fresh Opus test author added two colocated typed/untyped races; the driver compiled and ran them against unchanged runtime code, and both failed on lost successful fields in all 8 rounds. Fresh Opus implementor produced a seven-file candidate implementing locked authoritative reads, a required backend hook, type/schema requirement rechecking, own-commit index markers, tests and docs. Its `cargo +1.89.0 build` passed (264.51s), but driver card verification found commit-graph still under the lock, a boolean pending flag collapsing multiple maintenance counts, and a nondeterministic 500ms schema-race fixture; the approved card requires fixing all three. A fresh Opus correction dispatch failed at startup on usage quota (no tools/edits), reset **2026-09-13 21:10 Europe/Prague**. No retry before reset, no remaining gates or independent review run, no FT-5 commit/push. Candidate quarantined in stash `3044281e0c932fe250e657a446ef573debb20dcf` (`FT-5 build incomplete; Opus quota 2026-09-13`); master clean at `4fd6052`, one worktree. Full evidence, usage and next steps: `dev/local/reviews/FT-5-build.md`; approved card `dev/local/tmp/FT-5-card.md`; correction prompt `dev/local/tmp/FT-5-opus-correct.txt`; resume `dev/local/tmp/handoff-20260913-2025.md`. `FT-5-state.json` has phase `blocked_quota`, next_phase `build`, not review. Test-author total transcript tokens 1,207,321 (including cache); initial implementor 8,907,876 (including cache); correction did not run. Local origin/master now equals `4fd6052` (reflog: push 19:32:35 CEST, not by this session), so the initial handoff's unpushed-doc-commit claim is stale; its CI remains unverified here. Retain plan/evidence; finish FT-5 only after quota reset, full gates, fresh complete review roster and closure, then commit/push/verify exact-HEAD CI. Do not begin any other item.
- FT-5 build completion, 2026-09-13 23:40 CEST (Codex): verified and restored exact stash `3044281e0c932fe250e657a446ef573debb20dcf`, retaining it. Fresh Opus correction after reset (`1b9966da-21b8-4144-bdf0-24c8cd419a28`) fixed graph deferral, per-commit maintenance counting, deterministic schema-race synchronization and lock ordering; driver tightened prose and formatted only touched files. All pinned build gates passed: build, strict all-target Clippy, test-ci (1,856), service (198), CLI build, original six-round hazard, existing CLI/GraphQL lock race, full e2e repeat (569), touched rustfmt, mdbook, whitespace. The initial full-e2e run had one setup connection reset (568/569); exact isolated repro and unchanged full repeat passed. Cause unproven, failure retained in evidence. No independent review/commit/push; master and local origin/master remain `4fd6052`, exactly seven approved files modified, one worktree, stash retained as the OLD incomplete candidate (do not reapply). Build snapshot SHA-256 `5bf0888b86081b2078342c43c25c2c223812449de7d8fef6967da31056299d82`; evidence `dev/local/reviews/FT-5-build.md`; fresh-review handoff `dev/local/tmp/handoff-20260913-2340.md`; state next_phase `review`, rework 0. END build session here; complete the full §1 roster in the fresh review session, every CRITICAL/HIGH independently verified, at most one review rework and per-finding closure. After approval/fixes/gates, commit FT-5, drop its exact old stash, push master and verify exact-HEAD CI; stop before other items. Retain plan/evidence.
- S4:
- S5:
- S6: (release)
- S7: (design sign-off)

- FT-5 independent review stop, 2026-09-14 00:28 CEST (fresh Codex driver): eight isolated lanes completed (Rita/Cora/Grace/Toby/Mallory/Trent/Eve/Blake), 0 CRITICAL/HIGH, 17 raw MEDIUM/LOW advisories including duplicates. Eve identified two doc-wording FIX items, three VERIFY items, three KNOWN limitations. Bob/Carl helpers both exited 3 before model startup with `refusing nested dispatch (already inside a CLI agent)`; D6 stopped further dispatch, so Victor/final candidate inventory, remaining external lanes, rework and closure remain pending. No source edits, test reruns, commit/push or remote/CI check; prior gates remain in FT-5-build.md. At blocked-item exit, stashed exact reviewed candidate as `7f53131f57fca39a6f96affd4e338be5a3b4c6b6` (stash@{0}), retaining OLD incomplete `3044281e0c932fe250e657a446ef573debb20dcf` (now stash@{1}; do not apply). Reviewed stash matches build-ready snapshot SHA-256 `5bf0888b86081b2078342c43c25c2c223812449de7d8fef6967da31056299d82`; master clean at `4fd6052`, one worktree, no active PRD. Review model activity 7m50s; 5,436,908 tokens incl. cache across eight lanes and two read-only search children; custody later delayed by sandbox approval. Evidence and audit: `dev/local/reviews/FT-5-fast-track-review.md`, `FT-5-logs/review-session-audit.json`; continuation `dev/local/tmp/handoff-20260914-0028.md`. Maintainer direction/fresh Claude driver required to resume D6-blocked lanes; guards were not bypassed. Rework 0; retain plan/evidence; finish FT-5 only.
- FT-5 review resumed and closed, 2026-09-14 (fresh Claude driver, Opus 5): reviewed stash applied and verified against the snapshot SHA; Bob (codex, exit 0: 1 HIGH, 2 MEDIUM FIX, 1 VERIFY, R1-R5 pass) and Carl (copilot: `gemini-3.1-pro-preview` unavailable, `gemini-3.8-flash` exit 0 in 5m32s, 2 MEDIUM, 12 rubric verdicts) ran from this driver since its env carries no codex/copilot marker (D6 lesson: the nested-dispatch guard trips on the driver's host, not on the model; run the roster from a Claude driver). Victor (Sonnet) REFUTED the only CRITICAL/HIGH candidate FT5-BOB-01 (typedef commit-then-index window in `sql_engine/ddl.rs` is pre-existing, LOW, no allowlist fix) → PRD 00190 Risks. One Opus rework (7 items: EVE-2/EVE-6 docs, GRACE-1 extraction + `schema.is_some()`, GRACE-2 rename, BOB-03, BOB-02 attribution test with driver fail-first replay, TOBY-1 SINGLETON race); TOBY-2 rejected; 7 Sonnet closure checkers all CLOSED. All card gates rerun green (test-ci 42/1592/224, service 200, e2e 569/569 first run). Committed `99bc9bcee59e277e5c14bf3c29f999c26d8ec818` on master; both stashes dropped. **Push pending**: `git push origin master` failed twice with the 1Password SSH agent refusing to sign (sandboxed and unsandboxed); maintainer pushes and verifies `test.yml` on that exact HEAD. Carl note: the copilot default model in `gemini-run.sh` (`gemini-3.1-pro-preview`) is gone; pass `-m gemini-3.8-flash` or update the helper's default.

- FT-5 push continuation, 2026-09-14 21:22 CEST (Codex): resumed from the stale 00:28 handoff, found the later completed review/rework/gates and commit, and verified the committed delta matches the reviewed snapshot plus saved rework byte-for-byte. No restore, source edits or fresh cargo execution. Pushed `99bc9bcee59e277e5c14bf3c29f999c26d8ec818`; GitHub API confirms exact remote master. Both stashes had already been dropped by the prior session. Clean tree, one worktree, only master. Exact-HEAD Test `34886490658` and Docs `34886490594` in progress. Retain plan/evidence; no other FT item started.
- FT-5 final CI result, 2026-09-14 (same Codex continuation): exact-HEAD Docs `34886490594` passed (build/deploy); Test `34886490658` finished failed, with coverage successful and the test job blocked at Dependency policy by `RUSTSEC-2026-0285` in existing rustls 0.23.37 (cargo-deny requires >=0.23.45). Clippy/build/fast tests/e2e/sync were skipped in CI. Dependency files and policy are unchanged by FT-5. Failed-job log saved in `dev/local/reviews/FT-5-logs/ci-test-job-104118323553.log`; report/state updated. No dependency upgrade outside the FT-5 allowlist, policy bypass, rerun or next item. Stopped after FT-5; commit/push/review/local gates complete, CI verified failed and requires separate dependency remediation.

- FT-5 dependency CI follow-up closed, 2026-09-21 (Codex): separate authorized repair `a739c292748bcf5e35b5c6340eec7a360ceceb0c` updates rustls 0.23.37 → 0.23.45 for RUSTSEC-2026-0285 plus required AWS-LC/WebPKI lockfile changes and one changelog bullet. Dependency self-review found no corrective findings; fresh cargo-deny 0.20.2, pinned build, all-target Clippy with warnings denied, 1,858 fast tests, and diff checks passed. Pushed master; GitHub master ref confirmed and exact-HEAD Test `35573084487` passed (dependency policy, lint, build, fast tests, e2e, sync and coverage). Docs correctly not triggered by its path filter. Original FT-5 failure evidence preserved in report/state; clean tree, one worktree, only master, no stashes. Retain this unfinished plan; no other FT item started.

## 9. Logged elsewhere today

claude-autopilot backlog: 00186 item-grained fast-track lane, 00187 cap-out CRITICAL custody + deferred-HIGH homes, 00188 convergence/cap/roster telemetry, 00189 stall-to-split on plan expansion. agent-skills backlog: 00075 ground PRD claims against installed code. ddb backlog: 00203-00205 (specs for batch 2). Memory: `project_refactor_assessment_2026_09_06`.

## 10. FT-7 preparation and unresolved question — 2026-09-26

Prepared two draft cards and the source-grounded preflight note above at
`a739c292748bcf5e35b5c6340eec7a360ceceb0c`. Fixed the unsafe phase split in
the proposed item boundaries; added same-payload retry/concurrency, delivered
head/OID validation, late-failure custody and actual CLI/FFI conformance.
No implementation, Cargo gates, full review, commit, push or new CI run.
Existing actor/tests.rs additions and untracked .kiro/ remain untouched.
FT-5's completed evidence/state and all unfinished plan items are retained.

Approval 2026-09-27: maintainer said "ok to proceed" after the recommended
abort deferral. FT-7a/FT-7b cards and PRD00203 now retain the isolation fix and
existing CLI/FFI conformance; the recovery API is explicitly deferred. The
original PRD is preserved in dev/local/designs/00203-pre-FT-7-scope.md.
A fresh FT-7a build starts on fast-track/ft-7a in dev/local/worktrees/ft-7a from
verified local/remote a739c29. Main-checkout actor/mod.rs, actor/tests.rs and
.kiro/ changes are unrelated and must remain untouched. After build/gates,
END the session and hand off for the complete fresh review; no commit/push
before review. FT-7b follows only after FT-7a readiness. Do not start the
loop, S3 leftovers, other fast-track items or a release.

Location approval 2026-09-27: maintainer explicitly selected the hook-approved
dev/local/tmp/worktrees/00203-ft-7a exception after autopilot 0.5.6 rejected
source edits under dev/local/worktrees/. The clean fast-track/ft-7a worktree
was moved there; hooks, AGENTS.md and main-checkout user edits are preserved.
Build evidence/state: dev/local/reviews/FT-7a-build.md and
dev/local/tmp/FT-7a-state.json. Earlier quota/path failures are retained.

FT-7a build progress, 2026-09-28: seven-file candidate written; two primary
regressions proved red against baseline; eight tests added, all 15 original
bundle tests retained. Intermediate bundle filter (36), corrected cross-process
lease test, pinned build and strict all-target Clippy passed. Remaining gates
are pending: the host now rejects shell commands because its configured
/Users/bob/.codex/hooks/block_devlocal_redirects.py is missing. No hook changes
or bypass; post-pause Git rechecks could not run. State next_phase remains build,
not review. Read dev/local/tmp/FT-7a-build-blocked-20260928.md before resuming.
No review, commit, stash, push or FT-7b work; preserve the candidate worktree.

FT-7a review and landing, 2026-09-28 (fresh Claude driver, Opus 5.5): candidate
re-verified byte-identical (09d7869d...). Full roster via review-only harness run
wf_ac810dbb-fd1: fanout APPROVE (security dimension did not auto-arm: no keyword on
+/- lines), Mallory run explicitly, Eve/Blake/Bob(codex)/Carl(copilot gemini-3.8-flash)
all ok; 0 CRITICAL/HIGH, so no Victor candidates. One rework (fresh Opus Ivan,
dev/local/tmp/FT-7a-review-rework.md): real-import lease test, strict parser test,
function extraction, `--no-tags` (Mallory MEDIUM, confirmed live by mutation),
Bob's fixture, LOW docs/comments. Mutation probes: lease-drop and no-`--no-tags`
both fail their tests. 6/6 Sonnet closure checks CLOSED. Fresh gates: bundle 39,
build, strict clippy, fast tier 1869, sync 6, CLI build, bundle e2e 5, full e2e
569/569 first run, rustfmt/diff-check/mdbook. Committed 3b27002 (equals rework
snapshot), fast-forwarded and pushed master. Report:
dev/local/reviews/FT-7a-fast-track-review.md. Deferred to PRD 00203 follow-ups:
bundle/tests.rs 1,847 lines (split needs new file outside allowlist), three docs
still citing bundle.rs, Windows/macOS nightly confirmation of the cross-process
lease test. FT-7b not started.

FT-7b build, 2026-09-28 (fresh Claude driver, Opus 5.5), base 3b27002:
- Worktree `dev/local/tmp/worktrees/00203-ft-7b` (branch `fast-track/ft-7b`).
  A replay worktree `00203-ft-7b-baseline` sits at a739c29.
- A fresh Opus Ivan got only the card's Tests, Allowlist, Constraints and
  Read-only context. It wrote 3 CLI e2e tests and 3 FFI tests and ran no Cargo.
- The driver wrote the bundle/ffi docs and the showboat walkthrough.
- Fail-first replay on a739c29: 6/6 red at the first post-B assertion (old
  runtime reports masterless B as success). `prune=false` was not reached,
  the same limit FT-7a recorded.
- Runtime unchanged; no FT-7a correction needed.
- Gates: build, strict clippy, fast tier 1872, bundle 42, FFI 3, sync 6, CLI
  build, e2e bundle 8, full e2e 572/572 on the first run, rustfmt, mdbook,
  diff-check. The walkthrough passed safe-showboat-verify.
- Candidate: `dev/local/tmp/FT-7b-build-candidate.patch` (SHA-256 ac5f9350...).
- Evidence: `dev/local/reviews/FT-7b-build.md`.
- No review, commit, push, 00168 note or PRD move yet. Next: fresh review
  session via `docs/dev/tmp/00203-FT-7b-fresh-review-handoff-20260928.md`.

FT-7b review and landing, 2026-09-29 (fresh Claude driver, Opus 5.5):
- Candidate re-verified byte-identical (ac5f9350...). Master, origin and
  ls-remote were all at 3b27002.
- The full roster ran through the §1 manual fallback: Rita, Cora, Grace, Toby,
  Trent, Mallory (explicit), Eve, Blake (card only), Bob (codex) and Carl
  (gemini). All ok, 0 CRITICAL/HIGH, so Victor had no candidates.
- One rework (fresh Opus Ivan, `dev/local/tmp/FT-7b-review-rework.md`),
  8 items. 8/8 Sonnet closure checks came back CLOSED.
- Fresh a739c29 replay with prune=false first: 6/6 red on masterless-B false
  success.
- `--no-prune` mutation probe: still green, so the prune axis cannot be
  observed on the new runtime.
- Fresh gates:
  - Build and strict clippy passed.
  - Fast tier 1872, bundle 42, sync 6, CLI build.
  - Full e2e 572/572, first run.
  - rustfmt, diff-check and mdbook passed.
- Committed ab14498 (equals the rework snapshot), fast-forwarded and pushed
  master. Exact-HEAD CI: Test 36590016013 and Docs 36590016016 both succeeded.
- The 00168 hold note was annotated (CRITICAL closed, write-lease HIGH open).
  PRD 00203 moved to done with its Deferred work.
- Report: `dev/local/reviews/FT-7b-fast-track-review.md`. FT-7 is complete.
  Stopped; no FT-8/9 and no release.

S5 cards and FT-8a build, 2026-09-30 (fresh Claude driver, Opus 5.5), base a3e2dbc:
- Preflight clean: master == origin == ls-remote == a3e2dbc, no stashes,
  one worktree, no foreign session in the checkout.
- Cards written: `dev/local/tmp/FT-{8a,8b,9a,9b}-card.md`. No scope question
  was needed. Card-time drift: FT-7's failing-import fixtures (bundle, FFI and
  CLI e2e) use a frontmatter-less loser, so FT-8a moves them to a YAML-invalid
  loser instead of flipping them to success. FT-8b re-records walkthrough
  00203. Changelog rule: the behavior items (8a, 9a) carry their own
  Fixed/Changed lines.
- FT-8a: a fresh Opus Ivan wrote the code (0 Cargo calls). The fail-first
  replay on a3e2dbc (one-token shim) showed 5/5 new tests red on the wedge,
  with 48 controls and re-pointed fixtures green.
- Gates:
  - Build and strict clippy passed.
  - Fast tier 1884, filtered core 53, sync 6, CLI build.
  - e2e collision 4 and bundle 8.
  - Full e2e 572/572 on the first run.
  - Touched-hunk rustfmt and diff-check passed.
- Candidate: `dev/local/tmp/FT-8a-build-candidate.patch` (SHA-256
  51f59851...). Evidence: `dev/local/reviews/FT-8a-build.md`.
- No review, commit or push. Next: a fresh review session via
  `docs/dev/tmp/00204-FT-8a-fresh-review-handoff-20260930.md`.

FT-8a review and landing, 2026-09-30 to 2026-10-02 (fresh Claude driver, Opus 5.5):
- Preflight clean: master == origin == ls-remote == a3e2dbc, candidate
  byte-identical (51f59851...), three expected worktrees, no foreign session.
- Full roster via the §1 manual fallback, all 10 lanes ok. Carl's sandbox
  denied the scratchpad context file, so it read the handoff instead.
- Bob raised 2 HIGH; Victor CONFIRMED both:
  - H2 (MEDIUM-HIGH, new in FT-8a): verbatim bytes were a lossy UTF-8 re-blob.
  - H1 (LOW-MEDIUM): a later loser's link rewrite edited an earlier verbatim
    loser.
- Maintainer decisions:
  - Horizontal-rule predicate: pin and defer.
  - Clippy `manual_contains` after the rework: the driver applies clippy's
    suggestion and reruns every gate.
- One rework: a fresh Opus Ivan, 9 items, `dev/local/tmp/FT-8a-review-rework.md`.
- Fail-first on the build-candidate runtime (shim removed): the H2 and H1
  tests went red.
- Closure: 8/9 Sonnet checks CLOSED. Item 7 stayed OPEN only on test length
  (51/52 lines); accepted.
- Fresh gates:
  - Build and strict clippy passed.
  - Fast tier 1889, filtered core 58, sync 6, CLI build.
  - e2e collision 4 and bundle 8.
  - Full e2e 572/572 on the first run.
  - rustfmt (touched hunks only) and diff-check passed.
- Committed d7867d2 (equals the final snapshot a5bb396c...) and fast-forwarded
  master locally.
- Push: the first attempt failed (the 1Password SSH agent refused to sign);
  pushed once signing recovered. Exact-HEAD CI: Test 36997037232 succeeded
  (test + coverage). Docs was not triggered (it runs only on docs/** paths).
- An untracked `docs/dev/project-management/` mirror of `dev/local/` appeared
  in the main checkout during the session. It was left untouched.
- Report: `dev/local/reviews/FT-8a-fast-track-review.md`.
- Deferred:
  - The horizontal-rule residual wedge and preamble drop, plus the
    over-800-line test file split, go to PRD 00204 follow-ups.
  - Interface success tests, walkthrough 00203, sync.md/I1, the FFI report
    and the unindexable-file note go to FT-8b.
- FT-8b not started; PRD 00204 stays in backlog.

FT-8b build, 2026-10-02/03 (fresh Claude driver, Opus 5.5), base d7867d2:
- Preflight clean: master == origin == ls-remote == d7867d2, no stashes, one
  worktree, only the untracked mirror. Pid 99779 was the idle FT-8a review
  driver (it handed over this prompt), so it was treated as explained.
- Card refreshed. Seven drift points, listed in the card. The main ones:
  - The new type registers in `schema/mod.rs`, which was outside the
    allowlist.
  - The schema test also lives there.
  - Walkthroughs moved to the driver.
  - The e2e frontmatter-less loser must sort below `---`.
  - No scope question was needed.
- A fresh Sonnet Ivan made the change with 0 Cargo calls.
- Fail-first on d7867d2: 3/3 new tests red on the missing CLI line or
  GraphQL field. Limit: the GraphQL e2e test fails at its CLI leg there.
- Gates:
  - Build and strict clippy passed.
  - Fast tier 1890, server schema 1, sync 6, CLI build.
  - e2e collision 6 and bundle 8. Full e2e 574/574 on the first run.
  - safe-showboat-verify 2/2 (00203 re-recorded, 00204 new).
  - mdbook passed. rustfmt applied 0 hunks. diff-check clean.
- Candidate: `dev/local/tmp/FT-8b-build-candidate.patch` (SHA-256
  e166bc93...). 10 files, +223/-5.
- Evidence: `dev/local/reviews/FT-8b-build.md`.
- No review, commit or push yet. PRD 00204 stays in backlog.
- Next: a fresh review session via
  `docs/dev/tmp/00204-FT-8b-fresh-review-handoff-20261003.md`.

Unresolved questions: none for this scope; future abort design remains deferred.
