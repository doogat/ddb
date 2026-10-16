# PRD 00171 build phase report

Build complete:14/14 tasks. HEAD and live origin/master: `c8b72bd9126c83d054264ce6ac3738e8f6d0abda`. Full PRD acceptance is pending a fresh review session.

## Verification

- `cargo build`: exit0.
- `cargo build -p ddb-cli`: exit0.
- `cargo clippy --workspace --all-targets`: exit0.
- `cargo test-ci`: exit0.
- `CARGO_BIN_EXE_ddb=/Users/bob/git/src/github.com/doogat/ddb/target/debug/ddb /usr/bin/time -p cargo test -p ddb-e2e`: exit0.

Tests:2384 passed,0 failed,0 skipped (1819fast +565e2e). Counts are runner summaries at the settled HEAD; no source change followed. The supplemental5-test control and deliberately failing mutants are separate. Same-HEAD results were recorded once and reused for final verification; project rules delegate the heavy workspace/property/platform battery to CI.

Replacement CI:FullValidation33991344845 (all6 jobs, Linux/macOS/Windows included) and Test33991275129 green at5cf4c360556c1b8aa9a3e1bb7dfd83496ba269e4 before deletion. Post-deletion Test34016441899 (test+coverage) green atHEAD. Evidence:task13/task14 ci-green JSON reports.

## Per-task gates and provenance

1: style_gate: not recorded in inherited state, split_hygiene: not recorded in inherited state
2: style_gate: not recorded in inherited state, split_hygiene: not recorded in inherited state
3: style_gate: not recorded in inherited state, split_hygiene: not recorded in inherited state
4: style_gate: not recorded in inherited state
5: style_gate: not recorded in inherited state, split_hygiene: not recorded in inherited state
6: style_gate: not recorded in inherited state, split_hygiene: not recorded in inherited state
7: style_gate: not recorded in inherited state, split_hygiene: not recorded in inherited state
8: style_gate: not recorded in inherited state, split_hygiene: not recorded in inherited state
9: style_gate: not recorded in inherited state, split_hygiene: not recorded in inherited state
10: style_gate: not recorded in inherited state, split_hygiene: not recorded in inherited state
11: style_gate: not recorded in inherited state, split_hygiene: not recorded in inherited state
12: style_gate: clean, split_hygiene: skipped:no-tests
13: style_gate: clean, split_hygiene: skipped:no-tests
14: style_gate: clean, split_hygiene: skipped:no-tests

Tasks1–11 were completed before this continuation and were not replanned or reimplemented. Their legacy missing style/split stamps are not inferred as passed. Historical task1–3 review markers say failed:no_diff_gitignored_output; preserve these as fresh-review input. This continuation completed tasks12–14 and their mandated gates. All dispatch ledger rows are closed.

Task14 full Pat diff review:21/21 parsed,7LOW,0MEDIUM/HIGH/CRITICAL. Sonnet reviewed8; the user explicitly authorized Codex replacing the remaining13 to avoid quota waiting. Ten Codex reviews used fresh isolated agents; parent reviewed12,16,20. Canonical index and raw results are durable. Task12's14LOW and task14's7LOW are carried to fresh review. Task12's one MEDIUM was fixed and its resumed review cleared it. Actual adversarial evidence is in task12 and task14 Devon reports.

## Cleanup and boundary

57 task12/13 scratch files and163 remaining continuation scratch files were archived and byte-verified before removal. Archives/manifests live in reviews. Original handoff, unrelated PRD scratch, prior-session task7/design scratch, all durable artifacts and the unrelated pre-existing branch are preserved. Only the main worktree remains, no stash, no plans directory. No session-created branch or worktree remains. Final git status is clean and live origin/master matchesHEAD.

Preserve work_start_sha=b1e785395fb1e5d7043fbc725cc5786f62a6417e. Phase transition uses installed autopilot phase-done --outcome tasks_done; build leaves no PRD acceptance marker. Fresh review must run Alice, Blake and Bob doubt+de-slop R1–R5 every cycle. Do not invoke review-work-completion from this build session, close issues, start00172, or drain backlog.

## Assumption ledger (unchanged, copied verbatim)

# Assumption ledger — PRD 00170-id-throughput-ceiling-decision-v1

## 4: [D1] Correct the ID-format decision's collision and throughput claims, and close the 00201 plan gaps

Ivan (opus, rework cycle 1):

- Kept the 8-char/32-bit discriminator and weakened the claim rather than widening the field, because widening would create a second rendering of node identity that 00201's own "Node discriminator drift" risk forbids.
- Pinned the sub-second width at 6 (microseconds) as the decision, since the dispatch required removing `(guess)` and 6 was the width already drafted with a supporting rationale.
- Chose the deterministic hash fallback plus structured warning over a structured error for malformed `.git/ddb-node`; the dispatch permitted either.
- Assumed the create path's existing structured-warning channel is the right carrier for the node-file warning; could not verify that plumbing reaches the mint from the files in scope, so 00201 states it as a requirement rather than a verified wiring.
- Extended the collision and throughput corrections into the blast-radius note (beyond its listed jink item), because the CRITICAL and HIGH findings reproduced verbatim there and the note is the cited evidence trail for both other documents.
- Corrected `design-decisions.md`'s deciding-factor rationale to match the verified jink evidence; the dispatch scoped that verification to the note, but leaving the two documents contradictory would have failed the "documents agree" criterion.

**Consequence flagged by Ivan, carried forward deliberately:** the decision's recorded deciding
factor was "downstream ID parsing", and the only existing downstream (jink) parses nothing —
it types every doogat id as an opaque `Scalars['ID']`. The chosen shape still stands, but its
rationale now rests on ddb's own surfaces, wikilink legibility, and unknown future downstreams
rather than on a demonstrated jink break. Cycle 2's review should confirm this reframing is
sound rather than a rationalisation.

## PRD 00171 task 12: adversarial field-identity repair

Tess treated leading/trailing Markdown body whitespace as insignificant when
checking that a CLI body update landed in the committed body. The exact payload
and preserved title remain asserted; the check reads git HEAD, not the worktree.

