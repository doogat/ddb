# FT-5 build evidence (complete; independent review pending)

## Final build status — 2026-09-13 23:40 CEST

Build complete on uncommitted master based at `4fd6052`. This status supersedes
the historical in-progress/quota entries below. Every required gate has an
observed passing result on the final seven-file patch. No independent review,
commit, push or exact-HEAD CI verification occurred in this build session.

| Gate | Result | Log in `FT-5-logs/` |
| --- | --- | --- |
| Focused update unit tests | 3 passed; 2.59s execution | `focused-update.log` |
| Focused backend lock/maintenance unit tests | 7 passed; 0.60s execution | `focused-lock.log` |
| `cargo +1.89.0 build` | passed; 4m18s | `build-2.log` |
| `cargo +1.89.0 clippy --workspace --all-targets -- -D warnings` | passed; 3m32s | `clippy.log` |
| `cargo +1.89.0 test-ci` | 1,856 passed: CLI 42, core 1,590, server 224; core execution 1041.25s | `test-ci.log` |
| `cargo +1.89.0 test -p ddb-core --lib service` | 198 passed; 411.68s execution | `service.log` |
| `cargo +1.89.0 build -p ddb-cli` | passed; 3m35s | `cli-build.log` |
| Original concurrent single-row update hazard | passed, all six rounds retained; 2.28s execution | `hazard.log` |
| Existing CLI/GraphQL write-lock race | passed; 6.71s execution | `write-lock-races.log` |
| Full `cargo +1.89.0 test -p ddb-e2e` | repeat: 569 passed, 0 failed/ignored; 54.57s execution | `e2e-repeat.log` |
| Pinned rustfmt check, touched files only | passed (`--config skip_children=true`) | `rustfmt-check.log` |
| `mise exec -- mdbook build docs` | passed | `mdbook.log` |
| `git diff --check` | passed | `diff-check.log` |

All Cargo commands were foreground and sequential. Server e2e used approved
sandbox escalation for localhost listeners. Log footer wall durations include
approval/polling delay; compiler/test timings above come from Cargo itself.
No full-workspace heavy test battery, properties, coverage, or showboat verify.

**Initial full-e2e failure retained:** `e2e.log` has 568 passes and one
`Connection reset by peer` in
`integration_error_codes::integration_49_1_unique_violation_extensions_code`,
at `common.rs:186`, during the setup `graphql_with_vars` request. This was a
transport failure before the UNIQUE assertion, not a mismatched error code.
The exact test passed in isolation (1.35s; `e2e-error-code-repro.log`), then
the unchanged full suite passed all 569 tests. The reset's cause is unproven;
do not label the initial run green or claim a harness fix. No source changes
were made in response to that failure.

Final custody: one worktree, no new branches, no active PRD. HEAD and local
origin/master remain `4fd60524ca3aed2ff633ee9716b14cd8fb19d035`; current remote
and CI state are unverified. Exactly the card's seven files are modified.
Stash `3044281e0c932fe250e657a446ef573debb20dcf` remains at `stash@{0}` as the
OLD incomplete candidate; do not apply it over the corrected working tree.
Retain it until the reviewed FT-5 change is committed, then remove that exact
stash. No disposable worktrees/branches or executed plan files were created;
retain the fast-track plan and all evidence.

Snapshot `dev/local/tmp/FT-5-build-ready.diff` matches the live binary diff
byte-for-byte, SHA-256
`5bf0888b86081b2078342c43c25c2c223812449de7d8fef6967da31056299d82`.
Next session: `dev/local/tmp/handoff-20260913-2340.md`.
State is `build_complete`, `next_phase: review`, review unstarted, rework 0.
Run the full plan §1 roster in that fresh session; Victor verifies every
CRITICAL/HIGH; at most one review-driven rework, then per-finding closure.
Commit/push only after review approval and required gates; verify exact-HEAD
CI, then stop before any other fast-track item.

## Continuation preflight (2026-09-13)

- Verified clean master/HEAD/origin/master at `4fd60524ca3aed2ff633ee9716b14cd8fb19d035`, one worktree, and the sole stash at `3044281e0c932fe250e657a446ef573debb20dcf` with that base parent.
- Process/cwd inspection found the other active CLI agents in agent-skills, claude-autopilot and ovcaq, with only this driver in ddb. No foreign ddb build/editor session found.
- Restored the exact stash after this inspection; retained the stash. The snapshot SHA-256 still matches `21394ce476901de7ca4eeb1511b229cd328304a3de70a4af51f04ef9aed217df`. Hazard diff still only removes its ignore attribute. Whitespace check passed.
- Quota reset had passed before dispatch. Fresh Opus correction session: `1b9966da-21b8-4144-bdf0-24c8cd419a28`, using the existing correction prompt, no prior session context, no delegated cargo. One launch was blocked before startup by the dev/local shell-redirect rule; relaunched without output redirects, with driver preserving returned output through the file-editing tool.
- Correction and gates remain in progress. No review, commit or push.

## Correction result and gate continuation

Fresh Opus correction `1b9966da-21b8-4144-bdf0-24c8cd419a28` exited 0 after
478.089s. Returned result/usage: `dev/local/tmp/FT-5-opus-correct-2.log`;
full native transcript is in the ddb Claude project directory with that session
ID. Reported Opus usage: 100 input, 159,845 cache-created, 5,048,692 cache-read,
32,248 output tokens (including 10,055 thinking); no delegated cargo or agents.

The correction defers `write_commit_graph` at any lock nesting depth, replaces
the boolean maintenance queue with a commit count, drains after both guards
drop on `Ok` or `Err`, and adds on-disk graph/count/threshold regressions. The
schema-race fixture now synchronizes through a one-shot thread-local test seam
between preview and lock acquisition; its typedef indexing/materialization
runs with no Git lock held. Driver removed remaining absolute WAL/subprocess
claims in comments/docs and ran pinned rustfmt on only the four touched Rust
files (`--edition 2021 --config skip_children=true`); whitespace passed.

- Focused update tests: 3 passed (2.59s test execution), including typed and
  untyped 8-round races plus deterministic schema refusal. Log:
  `FT-5-logs/focused-update.log`.
- Focused lock/maintenance tests: 7 passed. The first successful invocation's
  tool wrapper hit a driver serialization error while storing an absent session
  ID; no Cargo process remained. Repeated the short group to retain an explicit
  exit-0 result: all 7 passed again (0.60s test execution). Log:
  `FT-5-logs/focused-lock.log`.
- Full pinned build/gates underway. Independent review remains unstarted.

Base: `4fd60524ca3aed2ff633ee9716b14cd8fb19d035` on master. Scope is
`dev/local/tmp/FT-5-card.md`, including the maintainer-approved exception in
`dev/local/tmp/FT-5-scope-proposal.md`. No active PRD or autopilot execution.

## Preflight

- Clean tracked working tree, no stashes, one worktree.
- Process/cwd inspection: no foreign CLI session using this checkout.
- No Workflow tool available; manual fast-track stages. Review must happen
  in a fresh session after this build handoff, with the entire plan roster.
- The trait gap was verified in a fresh read-only Opus session before the
  maintainer approved the minimal hook/maintenance/docs scope extension.

## Fail-first evidence

`cargo +1.89.0 build -p ddb-cli`: passed, 3m37s.

`cargo +1.89.0 test -p ddb-e2e --test e2e hazard_concurrent_single_row_update -- --ignored`:
FAILED on the intended Git durability assertion in all six rounds. This ran
against unchanged production/test source; the original ignore was explicitly
overridden on the command line. Compilation 23.70s, test execution 2.66s.

Fresh Opus test author added two colocated regressions in `service/update.rs`.
`cargo +1.89.0 test -p ddb-core --lib service::update::tests`: compiled, then
both tests FAILED on the intended lost-field assertions, all eight rounds
each (typed and untyped). Execution 1.64s; whole command 117.92s. Production
code was unchanged. Logs are under `FT-5-logs/` beside this file.

## Isolated build sessions

| Role | Model/session | Result |
| --- | --- | --- |
| Read-only scope feasibility | Opus 5 / `2b5a0a1e-347b-4c9c-843c-80ca0647b6e7` | Confirmed required trait/scope extension; no edits. Its suggested unlocked default was not accepted. |
| Colocated test author | Opus 5 / `91f6846a-e7f9-46cc-8430-9542a0cdb0a8` | Two test-only additions, no production changes; driver obtained semantic red evidence. |
| Runtime implementor | Opus 5 / `24b41de1-9071-4dba-955e-90286507e5a7` | Returned a seven-file patch. Driver found missed explicit requirements before starting the long gates; correction pending. |
| Fresh build correction | Opus / `61646856-c6b4-480e-8804-8de410317e28` | FAILED at startup on quota; no tool calls or edits. Reset 2026-09-13 21:10 Europe/Prague. |

Prompts/results are `dev/local/tmp/FT-5-opus-{preflight,tests,implement}.{txt,log}`.
Test-author usage (transcript, deduplicated API message IDs): 14 calls,
28 input + 89,039 cache-created + 1,100,591 cache-read + 17,663 output tokens.
The author used scoped read-only `rg` to satisfy the local Fact-Forcing Gate;
no cargo was delegated or run in parallel.

Runtime implementor usage (same transcript calculation): 64 calls,
128 input + 168,290 cache-created + 8,676,656 cache-read + 62,802 output tokens.
Its source/tool edit activity ran approximately 18:00-18:13 UTC. The correction
prompt/result is `dev/local/tmp/FT-5-opus-correct.{txt,log}`; do not retry the
quota failure before the reported reset. This failure is infrastructure, not
evidence that the correction was attempted or that its tests passed.

## Build correction required

This is driver verification of the card, not the independent review phase.
The initial implementor interpreted "single-file maintenance" as only
`check_write_threshold`, despite the approved scope explicitly naming
commit-graph too. `commit_files -> write_commit_graph -> Command::status`
still executes under the outer lock. Its `Cell<bool>` maintenance queue also
collapses multiple successful commits into one counter increment, violating
the approved requirement to preserve counters. Both need correction within
the already approved scope. The new schema-race regression expects a refusal
after a 500ms scheduling pause without a handshake, so it can fail if the
updater's preview starts late; replace that ordering with a deterministic
fixture, keeping the refusal/unchanged-Git assertions. The fixture itself
currently opens SQLite materialization under the Git lock; correct its lock
ordering too. These are build corrections, not authorization to widen FT-5.

The original hazard remains changed only by removal of its ignore attribute.
Docs also need to describe actual lock/maintenance guarantees without leaking
the private card's "no public API" negotiation into developer guidance.

## Post-change gates

`cargo +1.89.0 build` PASSED on the incomplete seven-file patch: 264.51s,
log `FT-5-logs/build-1.log`. `git diff --check` passed before stashing it.
Clippy, post-change unit tests, test-ci, service tests, hazard and lock-race
e2e, full e2e, pinned formatting and docs gates remain NOT RUN. Driver owns
every cargo invocation, sequentially in the foreground, with toolchain 1.89.0.
No independent review, FT-5 commit/push or new-SHA CI result is claimed.

## Quota handoff / custody

At 2026-09-13 20:25 CEST, all seven tracked files were stashed as
`3044281e0c932fe250e657a446ef573debb20dcf` (currently `stash@{0}`), message
`FT-5 build incomplete; Opus quota 2026-09-13`. Master is clean at `4fd6052`.
Only the seven named FT-5 files entered the stash; ignored evidence stays on
disk. No new branch/worktree was created. Snapshot:
`dev/local/tmp/FT-5-build-incomplete.diff`, SHA-256
`21394ce476901de7ca4eeb1511b229cd328304a3de70a4af51f04ef9aed217df`.

Local `origin/master` is now also `4fd6052`; its reflog records an update by
push at 19:32:35 CEST, outside this session's actions. The initial handoff's
one-ahead/not-pushed statement is therefore stale. No remote/CI query was run
to verify this commit's current CI. `target/debug/ddb` was built from the
stashed candidate; rebuild before treating it as a binary for clean master.

Resume build via `dev/local/tmp/handoff-20260913-2025.md` after quota reset,
restoring this exact stash first. The scope exception is already approved.
Correction, all remaining gates and fresh independent review remain required.

## Review custody

All lanes remain pending: consensus fanout (Rita/Cora/Grace/Toby/Mallory/Trent,
with Victor); Eve; blind Blake; Codex Bob doubt/de-slop R1-R5; Gemini Carl.
Every CRITICAL/HIGH receives independent Victor verification. At most one
review-driven rework, then per-finding closure. No skipped lenses. One
unavailable lane is documented per D6; two stop the batch for maintainer input.

No review-work-completion invocation from this build session. End with a
fresh-session handoff after the build gates. Retain the fast-track plan and
preserved evidence. Commit/push only after review approval and required gates,
then verify CI for the exact pushed HEAD. Stop before any other item.
