---
prd: docs/dev/project-management/prds/wip/00183-split-oversized-modules-round-2-v1.md
review: 1
date: 2026-10-20
head_sha: 4bd334d9e8a8c48b0438ef7a6c5e8f79a587c162
codex_thread_id: 01a156c4-f6f3-7791-9867-770c900e4918
reviewers: alice,blake,bob
agents:
  alice: available
  blake: available
  bob: available
  carl: unavailable
dispatch_rows:
  bob: 5907207d
  carl: 94fca06c
---

# Review: 00183-split-oversized-modules-round-2-v1

Diff range: `62659a636dbd6f311fda01ace9898fdd1b58bd1a..4bd334d9e8a8c48b0438ef7a6c5e8f79a587c162`

codex_rung_guard: not fired

## Review Summary

Cycle 1, full review, lane `full`. Three of four rostered reviewers produced usable
output. The 13 module splits verify as faithful pure moves on two independent
line-multiset comparisons (Alice and Blake, run separately). The blocker is the one
file the PRD asked to split "if warranted": `ddb-server/src/schema/queries.rs` is
992 production lines, up from 866 at base, against a PRD success metric of
<= 800 for every production file.

### Agent Status

- **Alice** (consensus, Claude subagent): available.
- **Blake** (blind lens, PRD-only Claude subagent): available.
- **Bob** (doubt lens + de-slop, codex): available **on the retry**. His first
  dispatch failed with `input_too_large` (exit 1): the inlined prompt was 1,597,935
  characters against codex's 1,048,576 limit, because this PRD's diff alone is
  1.47 MB. The retry trimmed the prompt to 1,008,722 characters by (a) generating
  the diff with zero context lines (`-U0`), (b) excluding the nine whole-file
  deletions and substituting their `--stat` line counts, and (c) omitting the
  engram pack. The prompt told Bob exactly what was withheld and why. **Bob's
  review is therefore degraded**: he did not see the deleted monolith bodies,
  so his judgment of move fidelity rests on the added side plus the deletion
  line counts, while Alice's and Blake's rest on the full both-sides comparison.
- **Carl** (UI/design lens, gemini via copilot): **unavailable this cycle —
  runtime failure, exit 1, `You have exceeded your monthly quota`** on backend
  `copilot` model `gemini-3.8-flash`, after ~3.5 AI credits. Not exit 4, so this
  is a one-off runtime failure and Carl is **not** latched for the batch. The
  one-retry budget was deliberately not spent: a monthly quota is not transient
  within this cycle, so a retry would consume credits for a certain second
  failure. Recorded, not retried.
- **Eve**: not activated. `doubt_reviewer` resolves to `codex` and no task
  attempt used a codex implementor, so the codex doubt-roster guard did not fire.

### Degradations and limitations (fail loud)

- `pack: ok` — the engram pack built normally and reached Alice. It was omitted
  from **Bob's** prompt only, to fit the provider input limit.
- The tautological-test-shape check reported `Checked 0 test function(s) in 0
  test file(s)`: it does not parse Rust, so it contributed no `[MECH]` findings
  here. The fail-first replay was not run (no `--replay-cmd`; this is a Rust
  project and the flag takes a pytest invocation). **Neither mechanical test
  check exercised this diff.** Tautology coverage this cycle rests on reviewer
  judgment alone (R2), which all three passed.
- Consolidation ran through `consolidate_findings.py` (not model-side). It
  dropped one finding-shaped line, Blake's `✅` all-clear on the `sql_conn`
  disposition, which is an all-clear and not a finding; it is reproduced in
  Blake's section below.
- No settled-decisions ledger exists yet for this PRD (cycle 1), so the
  `--ledger`/`--ledger-dismiss` flags were correctly omitted and nothing was
  auto-dismissed.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [3/3] | 🟠 | queries.rs is now 992 lines, up from 866 at base. Task 3 says "file stays <= 800 lines", and the PRD success metric requires every production file <= 800. The file has no tests, so all 992 lines are production. Move the per-type helpers (`register_dynamic_type_fields`, `register_typed_object_and_inputs`, `register_per_type_list_field`, `register_per_type_aggregate_field`, `register_per_type_singleton_field_if_applicable`, about lines 543-992) into a sibling file such as `schema/per_type_queries.rs`. Keep the discovery-field registrars in queries.rs. | ddb-server/src/schema/queries.rs:1 | 3 | ALICE, BLAKE, BOB |
| R2 | [2/3] | 🟡 | `register_dynamic_type_fields` spans lines 545-643, about 99 lines. It has no closure, so the design's closure-only exemption does not apply. It still inlines the discovery registration, the `known_types` and `seen_gql_names` loop, the `emitted_query_fields` and `schema_lookup` setup, the per-type loop and the relation-input loop. The same function-size rule is stated in the task 3 contract ("each register_* own body <= 50 lines"). Extract the `known_types` and `schema_lookup` build (lines 561-589) and the relation-input loop (637-640) into named helpers to get under 50. | ddb-server/src/schema/queries.rs:545 | 3 | ALICE, BOB |
| R3 | [1/3] | 🟡 | Stale module references remain in docs/src. Task 10 only touched modules.md and walkthrough.md, but AGENTS.md's Definition of Done requires updating the relevant docs. The stale lines are: docs/src/architecture/overview.md:144, 185 and 203 (tree still shows `crdt_resolver.rs`, `base_types.rs`, `filter.rs`); docs/src/technical/server.md:1101 and 1113 (same tree); docs/src/technical/indexer.md:3 (Source list still names `graph.rs`, `materialize.rs`, `search.rs`); docs/src/technical/crdt-resolver.md:3 (still `crdt_resolver.rs (487 lines)`); docs/src/technical/sql-engine.md:552, 587, 611, 615, 616 and 625 (`dml.rs`, `materialize.rs`, `helpers.rs`, `builders.rs` as single files); docs/src/technical/clean-architecture.md:11 and 49; AGENTS.md:20 (`crdt_resolver.rs`). | docs/src/architecture/overview.md:144 | 10 | ALICE |
| R4 | [1/3] | 🟡 | The extracted helpers are still far over 50 lines, with no inline justification. `register_search_field` runs 178-294, `register_per_type_list_field` 673-807, `register_per_type_aggregate_field` 810-912 and `register_tag_entries_field` 468-541. The Success Metric says "no single function exceeds the 50-line guideline (or is justified inline)", and the PRD's Behavior line asks for "per-field-group helpers". | ddb-server/src/schema/queries.rs:178 | 3 | BLAKE |
| R5 | [1/3] | 🟡 | The docs still use the old layout, so the "docs reflect the new module layout" acceptance is not met. Stale references remain in: `docs/src/technical/server.md:1101` and `:1113` (tree shows `base_types.rs` and `filter.rs`); `docs/src/architecture/overview.md:144`, `:185` and `:203`; `docs/src/technical/indexer.md:3` (lists `graph.rs`, `materialize.rs`, `search.rs`); `docs/src/technical/crdt-resolver.md:3` (says `crdt_resolver.rs`, 487 lines); `docs/src/technical/sql-engine.md:552`, `:587`, `:611`, `:615` and `:616` (`dml.rs`, `materialize.rs`, `helpers.rs`, `builders.rs`); `AGENTS.md:20` (`crdt_resolver.rs`). Source comments are stale too, for example `ddb-core/src/indexer/materialize/mod.rs:24`, `ddb-core/src/sql_engine/helpers/identifiers.rs:48` and `ddb-core/src/service/write_helpers.rs:17`. | docs/src/technical/server.md | 10 | BLAKE |
| R6 | [1/3] | 🟡 | Required original symbol paths were deliberately dropped: `filter::{reverse_relations, ReverseRelation}` and eight `base_types` exports, as recorded in assumptions.md. Current caller compilation does not satisfy the explicit unchanged-surface contract. Restore the original paths and visibilities through module re-exports. | docs/dev/project-management/meta/assumptions.md:8 | general | BOB |
| R7 | [1/3] | ⚪ | Code comments still point at the old single-file paths: ddb-core/src/indexer/mod.rs:466 and ddb-core/src/service/write_helpers.rs:17 (`sql_engine/dml.rs`), ddb-core/src/service/validation.rs:137 and 156 (`ddl.rs`, `dml.rs`), ddb-core/src/error.rs:194, 255 and 270 (`materialize.rs`, `dml.rs::...`), ddb-core/src/types/query.rs:6 (`filter.rs`). Update them to the new paths. | ddb-core/src/indexer/mod.rs:466 | 4 | ALICE |
| R8 | [1/3] | ⚪ | `ddb-server/src/filter/tests.rs` is 2235 lines. The filter acceptance says "every `filter/*.rs` <= 800", but the Success Metrics say "production file". The PRD tree names `tests.rs` itself, so this is probably intended; I note it for the record. | ddb-server/src/filter/tests.rs | 1 | BLAKE |
| R9 | [1/3] | ⚪ | `pub(crate) ReverseRelation` and `reverse_relations` are no longer reachable at `crate::filter::*`, because `mod.rs` does not re-export them. This is a deviation from "preserve every pub/pub(crate) item and its path". Nothing in the crate uses them at that path, and the deviation is recorded in `docs/dev/project-management/meta/assumptions.md:8`. | ddb-server/src/filter/mod.rs:20 | 1 | BLAKE |
| R10 | [1/3] | ⚪ | `ddb-core/src/indexer/mod.rs` has 898 production lines (its `#[cfg(test)]` starts at line 899). That is over the cap and is not on the PRD list. The PRD said no unlisted file exceeded 800 at 8fe51c9, so it grew afterwards. This is out of scope, but a future sweep should pick it up. | ddb-core/src/indexer/mod.rs:899 | general | BLAKE |
| R11 | [1/3] | ⚪ | `actor::tests::shared_actor_preserves_schema_applys_owned_transaction` is flaky under full-suite load. It timed out at `ddb-server/src/actor/tests/mod.rs:22` and passes alone. It is unrelated to the splits but turns the Tier-1 gate red intermittently. | ddb-server/src/actor/tests/mod.rs:22 | 11 | BLAKE |
| R12 | [1/3] | ⚪ | The updated module table still assigns initialization and CRUD to `git_ops/mod.rs`, although those implementations moved into `repo_core.rs`. Describe `mod.rs` as holding shared definitions and module declarations; assign initialization and CRUD to `repo_core.rs`. | docs/src/architecture/modules.md:78 | 10 | BOB |

Consolidation note: `consolidate_findings.py` merged row R1's three citations
(`queries.rs:1`, `:54`, `:782`) after suffix stripping — all three reviewers
named the same file, so the 3/3 consensus is real, not a line-number artifact.

## Alice

Verification she ran:

- Compared each old file's non-blank, whitespace-trimmed lines against the union
  of its new files, as multisets, for all 13 split targets. Every residual
  difference is an import, a `mod`/`pub use` line, a visibility widening, a
  `super::` path or an `impl` header.
- No logic line differs in the 12 pure-move splits.
- Compared item visibility old vs new per split: every `pub` item stays `pub`
  and every re-export is present. The only changes are `priv`/`pub(super)`
  widened to `pub(super)`/`pub(crate)`/`pub(in crate::sql_engine)`.
- The `queries.rs` decomposition preserves field registration order. The only
  reordering puts the relation-filter inputs before `tags`/`tagEntries`, but
  they go into the separate `dynamic_inputs` vec, so schema order is unchanged.
- No `#[ignore]`, `#[allow]`, TODO, `dbg!` or `println!` was added.
- Confirmed the `sql_conn` disposition at `ddb-core/src/traits.rs:68` and
  `docs/src/architecture/design-decisions.md:210`: ~91 call sites across 25
  files, matching `rg`.
- The `sync_manager` re-measure is recorded in commit d6de3d4 (796 production
  lines).

Findings: R1 (🟠), R2 (🟡), R3 (🟡), R7 (⚪) above.

R1: pass
R2: pass
R3: pass
R4: pass
R6: pass
R7: pass
R8: pass
R9: pass
R10: pass
R11: pass
R12: fail
R13: fail

## Blake

Blind lens, PRD-only prompt: no diff, no file list, no review history. He located
the code himself.

Evidence for his pure-move claim:

- Compared each pre-split file at 62659a6 against its new files as multisets of
  whitespace-normalised lines, covering filter, helpers, dml, ddl, materialize,
  base_types, graph, builders, crdt_resolver, parser, git_ops and search.
- Every difference is an import, a `mod`/re-export line, a visibility change
  (`pub(super)` → `pub(crate)` or `pub(in crate::sql_engine)`) or a `super::`
  path rewrite. No logic lines changed.
- For `build_query_fields`, the diff and the original registration order show the
  same order of fields, types and inputs.
- No new `#[allow]` was added in the split code.

Verification he ran:

- `cargo test -p ddb-core --lib` passed (exit 0).
- `cargo test -p ddb-server --lib`: 320 passed, 1 failed —
  `actor::tests::shared_actor_preserves_schema_applys_owned_transaction` timed
  out under load and passes alone in 0.89s; the actor module is untouched by this
  PRD. This matches the test gate's exit 101.
- `cargo clippy --workspace --all-targets` clean.
- He did **not** run `cargo test -p ddb-e2e`.

Findings: R1 (🟠), R4 (🟡), R5 (🟡), R8 (⚪), R9 (⚪), R10 (⚪), R11 (⚪) above.

All-clear (not a finding, so consolidation did not table it):

> [BLAKE] ✅ The `sql_conn` exception is dispositioned as the PRD asks.
> `ddb-core/src/traits.rs` and `docs/src/architecture/design-decisions.md`
> (retained-exception section) give the post-split caller inventory, and the 25
> files match my own `rg` count. They state no successor PRD and name the
> evolution assessment as owner. | File: ddb-core/src/traits.rs:68 | Task: 9

B1: fail
B2: pass
B3: pass
B4: pass
B5: pass
B6: pass
B7: pass
B8: pass
B9: pass
B10: pass
B11: pass
B12: pass
B13: pass
B14: pass
B15: pass
B16: fail
B17: pass
B18: pass
B19: pass

## Bob

Doubt lens + de-slop, codex, static-only sandbox. Ran on the retry with the
trimmed prompt described under Agent Status.

FIX: R1 (🟡 in his own grading), R2 (🟡), R6 (🟡), R12 (⚪) above.

VERIFY:
- (none)

KNOWN:
- (none)

Because his VERIFY bucket is empty, no verification-check queue file was written
for this cycle.

R1: pass
R2: pass
R3: pass
R4: pass
R6: pass
R7: pass
R8: pass
R9: fail
R10: pass
R11: pass
R12: fail
R13: fail
D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Carl

Carl: unavailable (runtime failure, exit 1 — `You have exceeded your monthly
quota`; backend `copilot`, model `gemini-3.8-flash`). Not latched for the batch:
the exit code was 1, not 4, so this is a one-off runtime failure under the Carl
exit-code contract. The one-retry budget was not spent, because a monthly quota
ceiling cannot clear within this cycle.

Verdict: 12 findings
Tests: 2103 passed, 1 failed, 0 skipped (suite run this cycle; `cargo test-ci` exit 101 — the sole failure is `ddb-server actor::tests::shared_actor_preserves_schema_applys_owned_transaction`, the documented parallel-load flake of PRDs 00177/00178, which passes alone and sits in no file this PRD touched)

Provenance note on the `Tests:` line: `review-stage` ran the gate this cycle at
HEAD and its own parser emitted `Tests: counts unavailable (suite run this
cycle, exit 101, no PASS/FAIL/SKIP/EXIT summary line)` — `cargo test-ci` prints
one `test result:` line per crate target, not the single summary line the parser
expects. The counts above are read directly from that same run's log,
`docs/dev/tmp/gate-00183-c1.log` lines 56, 1871 and 2209 (47 + 1736 + 320
passed; 1 failed). `autopilot verify` was deliberately not re-run: it wraps the
same `run_gate()` parser, so a second ~26-minute suite run would reproduce the
same unparseable shape and add no information.
