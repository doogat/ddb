---
prd: docs/dev/project-management/prds/wip/00183-split-oversized-modules-round-2-v1.md
review: 2
date: 2026-10-20
head_sha: 5df8f561952aa6b6db052bee00b6559ce8594724
codex_thread_id: 01a156c4-f6f3-7791-9867-770c900e4918
reviewers: alice,blake,bob
agents:
  alice: available
  blake: available
  bob: available
  carl: unavailable
dispatch_rows:
  bob: 486cd62e
  carl: a0718067
---

# Review: 00183-split-oversized-modules-round-2-v1

Diff range: `4bd334d9e8a8c48b0438ef7a6c5e8f79a587c162..5df8f561952aa6b6db052bee00b6559ce8594724`
(incremental: the cycle-1 rework commits 38f681d, bdb8b1e, cf9c8f2 plus the autopilot
state records)

codex_rung_guard: not fired

## Review Summary

Cycle 2, incremental review, lane `full`. Three of four rostered reviewers produced
usable output. **The cycle-1 blocker R1 is resolved**: `ddb-server/src/schema/queries.rs`
is 553 lines (was 992) and the per-type registrars moved to the new
`ddb-server/src/schema/per_type_queries.rs` (632 lines), both under the PRD's 800-line
production cap. R2/R4 (register_* body sizes) are resolved — `register_dynamic_type_fields`
is 47 lines, `register_fields_for_schema` 48 — and the R3/R7 docs and comment refresh
largely landed.

Alice verified the extraction is behavior-preserving by hand (argument parse order, error
conversion, search filter chain, `known_types` construction, `emitted_query_fields` insert
order and the singleton collision fallback all unchanged). Blake, blind, independently
re-ran the pure-move multiset comparison across all 11 split targets and found only
imports, `mod`/`pub use` lines, visibility widenings, `super::` rewrites and rustfmt
re-wraps.

**No CRITICAL or HIGH finding remains.** The seven consolidated findings are two 🟡 and
five ⚪, every one single-voice: a residual-stale-comment tail, one duplication in the new
file, and docs-tree gaps for `per_type_queries.rs`.

### Agent Status

- **Alice** (consensus, Claude subagent): available. Ran `cargo check -p ddb-server
  --all-targets` (pass), `cargo clippy -p ddb-server --all-targets -- -D warnings`
  (clean) and `cargo test -p ddb-server` (320 passed, 1 failed — the settled actor
  load-flake) in a private target dir, because the shared build lock was held.
- **Blake** (blind lens, PRD-only Claude subagent): available. Ran `cargo test -p
  ddb-server --lib` (320 passed, 1 failed, the same flake). His
  `cargo check --workspace --all-targets` was killed stalling on the build-directory
  lock, and he did not run `cargo clippy --workspace --all-targets` or
  `cargo test -p ddb-e2e`.
- **Bob** (doubt lens + de-slop, codex): available, **first dispatch, no retry needed**.
  He resumed his cycle-1 codex thread (`--resume-thread
  01a156c4-f6f3-7791-9867-770c900e4918`). Unlike cycle 1, the inlined prompt fit
  comfortably: 256,712 characters against codex's 1,048,576 limit, because the
  incremental diff is 118 KB rather than cycle 1's 1.47 MB. **Bob's review is not
  degraded this cycle** — he saw the full context, the full diff and the engram pack.
- **Carl** (UI/design lens, gemini via copilot): **unavailable this cycle — runtime
  failure, exit 1, `You have exceeded your monthly quota`** on backend `copilot` model
  `gemini-3.8-flash`, 0 AI credits spent. Same quota wall as cycle 1. Not exit 4, so Carl
  is **not** latched for the batch. The one-retry budget was deliberately not spent: a
  monthly quota is not transient within this cycle.
- **Eve**: not activated. `doubt_reviewer` resolves to `codex` and no task attempt used a
  codex implementor, so the codex doubt-roster guard did not fire.

### Degradations and limitations (fail loud)

- `pack: ok` — the engram pack built normally and reached Alice and Bob.
- The tautological-test-shape check contributed no `[MECH]` findings: it does not parse
  Rust. The fail-first replay was not run (no `--replay-cmd`; it takes a pytest
  invocation and this is a Rust project). **Neither mechanical test check exercised this
  diff**, so tautology coverage rests on reviewer judgment alone again this cycle.
- The test gate ran this cycle but `review-stage` could not parse counts from
  `cargo test-ci`'s exit-101 output, so no `last-verification.json` was recorded. The
  counts in the `Tests:` line below were read directly from the gate log
  (`docs/dev/tmp/gate-20261020c2.log`, lines 56, 1871 and 2209: 47 + 1736 + 320 = 2103
  passed, 1 failed).
- An orphaned `autopilot verify` process from the previous (dead) session held the
  verification lock for roughly an hour before dying without writing a record; the gate
  was re-run from scratch after it cleared. No review input came from that run.
- Consolidation ran through `consolidate_findings.py` (not model-side), with
  `--ledger`/`--ledger-dismiss BLAKE` active. Four Blake re-raises were auto-dismissed
  against the settled ledger; they are reproduced below.
- No verification-check queue was written for this cycle: Bob's VERIFY bucket is
  `(none)`, so no finding named a runnable command. No prior-cycle check queue exists for
  this PRD, so nothing was carried forward.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [1/3] | 🟡 | Prior finding R7/R13 only partly fixed: stale single-file path comments remain. `ddb-core/src/service/validation.rs:137` still says `sql_engine/ddl.rs`, although the prior finding named it and only line 156 was updated. `ddb-core/src/sql_engine/ddl/alter.rs:368` still says `dml.rs`. `ddb-core/src/sql_engine/tests.rs:8237` and `:8412` still say `dml.rs`. Point each at the new path (`ddl/` modules, `dml/update.rs` or `dml/mod.rs`). | ddb-core/src/service/validation.rs:137 | 13 | ALICE |
| R2 | [1/3] | 🟡 | `build_list_where_clause` and `build_aggregate_where_clause` duplicate the same deserialization, empty-filter fallback, validation, and error conversion. Replace them with one shared helper used by both resolvers; preserve the existing fallback behavior. | ddb-server/src/schema/per_type_queries.rs:366 | 11 | BOB |
| R3 | [1/3] | ⚪ | Docs still lag the new layout in a few places. `per_type_queries.rs` is new but absent from the `schema/` trees at `docs/src/technical/server.md:1101` and `docs/src/architecture/overview.md:190`, which list only `queries.rs`. `docs/src/technical/errors.md:14`, `:45` and `:86` still name `crdt_resolver.rs` and `sql_engine/dml.rs::handle_insert`. `docs/src/technical/invariants.md:85` still names `indexer/materialize.rs` and `ddb-server/src/filter.rs`. `docs/src/architecture/modules.md:78` says `repo_core.rs` holds the struct, but `GitRepo` is defined in `git_ops/mod.rs:96`. | docs/src/technical/server.md:1101 | 12 | ALICE |
| R4 | [1/3] | ⚪ | Stale single-file path references remain in comments and docs after the doc-fix commits. `ddb-core/src/service/validation.rs:137` still says `sql_engine/ddl.rs`. `ddb-core/src/sql_engine/ddl/alter.rs:368`, `ddb-core/src/sql_engine/tests.rs:8237` and `:8412` still say `dml.rs`. `ddb-server/src/schema/mod.rs:433` and `:435` still cite `queries.rs:392`, which no longer holds that code. `docs/src/technical/errors.md:14`, `:45` and `:86`, `docs/src/technical/invariants.md:39` and `:85`, and `docs/src/technical/phase-2-exit.md:16`, `:18` and `:24` still name `crdt_resolver.rs`, `dml.rs`, `materialize.rs` or `filter.rs` as single files. The new `schema/per_type_queries.rs` appears in neither `docs/src/technical/server.md:1107` nor `docs/src/architecture/overview.md:196`. The docs acceptance ("docs reflect the new module layout") is largely met. `modules.md`, `overview.md`, `server.md`, `indexer.md` and `walkthrough.md` are updated, but not completely. | docs/src/technical/errors.md:14 | 10 | BLAKE |
| R5 | [1/3] | ⚪ | Several helpers extracted from `build_query_fields` still exceed 50 lines. Examples are `register_doogats_field` (`ddb-server/src/schema/queries.rs:91-153`), `register_search_field` and `register_per_type_list_field` (`ddb-server/src/schema/per_type_queries.rs:221-259`). They are mostly builder-chain and closure bodies, with no inline justification. The PRD requirement is only that the top-level builder be under 50 lines, and that holds: `build_query_fields` is `queries.rs:32-69`. Noted for the record. | ddb-server/src/schema/queries.rs:91 | 3 | BLAKE |
| R6 | [1/3] | ⚪ | The revised module table now places the `GitRepo` struct in `repo_core.rs`, although the split retains its definition in `git_ops/mod.rs`. Assign shared definitions to `mod.rs` and initialization/CRUD implementations to `repo_core.rs`. | docs/src/architecture/modules.md:78 | 12 | BOB |
| R7 | [1/3] | ⚪ | The refreshed schema trees omit the newly added `per_type_queries.rs`. Add it beside `queries.rs` in both architecture/overview.md and technical/server.md, identifying its per-type registration responsibility. | docs/src/architecture/overview.md:196 | 12 | BOB |

### Auto-dismissed (ledger)

- [BLAKE] 🟡 The Tier-1 gate is red at HEAD, but the cause is a flaky test outside this PRD (`actor::tests::shared_actor_preserves_schema_applys_owned_transaction`, timeout at `ddb-server/src/actor/tests/mod.rs:22`). | File: ddb-server/src/actor/tests/mod.rs:22 — pre-existing, documented, and outside this PRD's diff: the actor module is in no file this PRD split. It is the known parallel-load flake of PRDs 00177/00178, re-verified passing alone. Not this PRD's regression and not its to fix.
- [BLAKE] 🟡 `ddb-server/src/filter/mod.rs:20` does not re-export `ReverseRelation`/`reverse_relations`, and `schema/base_types/mod.rs:14` dropped eight names, against "preserve every `pub`/`pub(crate)` item and its path". | File: ddb-server/src/filter/mod.rs:20 — a documented, justified deviation on crate-internal names nobody consumes (recorded at `docs/dev/project-management/meta/assumptions.md:8`), where the alternative is a hook-blocked lint suppression. Blake himself recorded it as a note, not a defect.
- [BLAKE] ⚪ `ddb-core/src/indexer/mod.rs` has 898 production lines and is not on the PRD's split list. | File: ddb-core/src/indexer/mod.rs:899 — genuine and newly measured, but outside this PRD's enumerated split list, fixed at the 8fe51c9 measurement. **Settled deferral** to batch end so a round-3 sweep PRD picks it up with the rest of the regrown files.
- [BLAKE] ⚪ `ddb-server/src/filter/tests.rs` is 2235 lines and `ddb-core/src/sql_engine/helpers/tests.rs` is 451. | File: ddb-server/src/filter/tests.rs — out of scope by PRD text: the success metric is scoped to production files and the PRD's own tree names `filter/tests.rs`. Blake reached the same reading.

## Alice

Findings: R1 (🟡), R3 (⚪) above.

Incremental verification of the prior findings:

- R1 (queries.rs over 800 lines) is resolved: `queries.rs` is 553 lines and
  `per_type_queries.rs` is 632.
- R2 and R4 (functions over 50 lines) are resolved: `register_dynamic_type_fields` is 47
  lines, `register_fields_for_schema` 48. The resolver bodies moved into `resolve_search`,
  `resolve_tag_entries` and `resolve_per_type_*`, each under 50.
  `register_doogats_field` is about 63 lines; it was not changed and keeps its resolver
  closure inline, which the inline-closure exemption covers.
- Settled items (R6, R8, R9, R10, R11) were not re-raised.
- The R3 docs fix and most source-comment fixes are in place. The new
  `sql_engine/delete.rs`, `materialize/inference.rs`, `dml/validate.rs`,
  `helpers/eval.rs` and `dml/insert.rs` references all point at files that exist and hold
  the cited symbols.

Behavior check on the extraction (not a pure move this cycle, so she compared old and new
code by hand): argument parse order unchanged (tag, limit, offset, distinct, orderBy,
where); error conversion unchanged (`ServerError` → `async_graphql::Error` via `Into`); a
non-string `eq` in the search filter chain still returns `None` without falling through;
`known_types` starts empty so `collect_known_types` yields the same map;
`emitted_query_fields` insert order and the singleton collision fallback unchanged; the
added `.clone()` calls copy the same data the old code moved.

Verification she ran (private target dir — shared build lock held):
`cargo check -p ddb-server --all-targets` passed; `cargo clippy -p ddb-server
--all-targets -- -D warnings` clean; `cargo test -p ddb-server` 320 passed, 1 failed (the
settled actor load-flake). All `schema::tests::*` singleton, collision and description
tests passed. No new tests were added, and none are needed: the rework changes no
behavior and the existing schema tests cover it.

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
R12: pass
R13: pass

## Blake

Blind lens, PRD-only prompt: no diff, no file list, no review history. He located the code
himself.

Findings: R4 (⚪), R5 (⚪) above, plus the four ledger-dismissed re-raises listed above.

All-clear (not a finding, so consolidation did not table it):

> [BLAKE] ✅ Everything else I checked holds. Every production file produced by a split is
> <= 800 lines (largest: `ddb-core/src/indexer/search/mod.rs` at 791). Pure-move check:
> whitespace-trimmed line multisets of base 62659a6 against the union of the new files for
> all 11 split targets plus the `search` rename — the only differences are imports,
> `mod`/`pub use` lines, visibility widenings, `super::` rewrites, `impl` headers and
> rustfmt re-wraps. Visibility widening: `pub(crate)` items inside private `helpers/`
> submodules are re-exported at `pub(super)`, so effective visibility is unchanged.
> `build_query_fields` registration order matches the original, with relation-filter
> inputs in the separate `dynamic_inputs` vec so field order is unchanged; the singleton
> collision logic and error text are identical; resolver argument-parsing order is
> preserved. The `sql_conn` exception is retained with its post-split caller inventory
> (25 files, ~90 sites) at `ddb-core/src/traits.rs:68-89` and
> `docs/src/architecture/design-decisions.md:210-223`. Hygiene: no new dependencies, no
> new flags or parameters, no `#[allow]` or TODO in the files he read.

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

Doubt lens + de-slop, codex, resumed on his cycle-1 thread. Full context and full diff
inlined (256,712 characters).

FIX: R2 (🟡), R6 (⚪), R7 (⚪) above.

VERIFY: (none)

KNOWN: (none)

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
R12: pass
R13: pass
D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Carl

Carl: unavailable this cycle (runtime failure, exit 1, `You have exceeded your monthly
quota`, Request ID C0D8:4A94:8E666F:9735D9:6AD7279B) on backend `copilot` model
`gemini-3.8-flash`. Not latched for the batch; no retry spent.

Verdict: 7 findings
Tests: 2103 passed, 1 failed, 0 skipped (suite run this cycle; `cargo test-ci` exit 101 — the sole failure is `ddb-server actor::tests::shared_actor_preserves_schema_applys_owned_transaction`, the documented parallel-load flake of PRDs 00177/00178, which passes alone and sits in no file this PRD touched)
