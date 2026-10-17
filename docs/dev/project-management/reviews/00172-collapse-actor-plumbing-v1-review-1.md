---
prd: docs/dev/project-management/prds/wip/00172-collapse-actor-plumbing-v1.md
review: 1
date: 2026-10-17
head_sha: 777f2a39e8b6d830a12ad99060326a7c723a8633
codex_thread_id: 01a147bb-51ed-71a2-9fe1-5a7cddd312c9
reviewers: alice,blake,bob,carl
agents:
  alice: available
  blake: available
  bob: available
  carl: available
dispatch_rows:
  bob: e8bee6a4
  carl: 2d829897
---

# Review: 00172-collapse-actor-plumbing-v1

Diff range: `d2cbea61044449b082241fbd4a8d4391b8f6298c..777f2a39e8b6d830a12ad99060326a7c723a8633`

codex_rung_guard: not fired

## Review Summary

Reviewed: 6 completed tasks
PRDs checked: 00172-collapse-actor-plumbing-v1.md
Cycle: 1 (full review, `--since work_start_sha`)
Consensus engine: legacy (single Alice subagent)
Pack: ok

### Agent Status

- Alice: ✅ Available (Claude subagent, consensus lens)
- Blake: ✅ Available (Claude subagent, blind/PRD-only lens)
- Bob: ✅ Available (codex, doubt lens) — after one retry (inlined). The first
  dispatch exited 0 with the documented lack-of-input refusal shape
  (`sandbox-exec: sandbox_apply: Operation not permitted`, no readable files),
  so the inlined retry prompt (context + full diff inlined) was dispatched per
  retry-policy.md § Lack-of-input refusal. The retry produced a full review.
- Carl: ✅ Available (gemini via copilot backend, UI/design + generalist lens)

### Consolidation notes

- `consolidate_findings.py` parses only line-initial `[{AGENT}] …` issue lines.
  Bob emits his findings as `- [BOB] …` bullets inside his FIX bucket, so the
  first consolidation run dropped all five of them. A normalized copy of his
  output (leading `- ` stripped from issue lines only, text otherwise verbatim)
  at `docs/dev/tmp/bob-output-00172-c1-normalized.txt` was used as the BOB
  input; his raw output is unchanged at `docs/dev/tmp/bob-output-00172-c1.txt`.
  This is a loud workaround, not a silent drop — Bob's findings are rows R1,
  R2, R11, R12 and R17 below.
- The consolidator reported four citation merges that matched only after line-
  suffix stripping (R1 mod.rs:319~321, R2 integration_rest_pgwire.rs:224~247,
  R3 tests.rs:941~1, R4 mod.rs:868~1).
- Mechanical test checks: the tautological-shape scan checked 0 test functions
  (all changed test files are Rust; the scanner is Python-only), and the
  fail-first replay was skipped for the same reason. No `[MECH]` lines to
  absorb.
- Carry-forward: none (cycle 1, no prior checks queue).
- Verification-check queue: not written. Bob's VERIFY bucket is `(none)` and
  Eve did not run (the codex doubt-roster guard did not fire — all six tasks
  were implemented by Claude), so no VERIFY item names a command.
- Two findings about the same 800-line concern landed as separate rows (R3/R4
  from Alice+Carl, R12 from Bob) because their citations differ.

## Consolidated Findings

| Ref | Consensus | Severity | Issue | File | Task | Found By |
|-----|-----------|----------|-------|------|------|----------|
| R1 | [3/4] | 🟡 | `ClosureMsg` is now a one-field wrapper plus a dead `event: EventIntent` field, always set to `EventIntent::None` and carrying an allow(dead_code) attribute. The real intent is moved into the boxed `run` closure. Remove the `event` field and the suppression attribute, then send `ActorRun` directly (`mpsc::Sender<ActorRun>`). This is behavior-preserving. | ddb-server/src/actor/mod.rs:319 | 5 | ALICE, BLAKE, BOB |
| R2 | [2/4] | 🟡 | Test gap against the PRD criterion "a schema-changing mutation on each surface bumps the schema version and a non-schema mutation does not". PgWire has only the positive DDL case (`integration_21_pgwire_ddl_triggers_schema_reload`), with no non-schema statement asserting zero reload. The GraphQL tests assert `version > before`, not "exactly one", so a double reload (adapter trigger plus `call`) would still pass. Fix: add a PgWire DML case with an unchanged `schemaVersion`, and change the GraphQL asserts to `== before + 1`. | tests/e2e/integration_rest_pgwire.rs:224 | 4 | ALICE, BOB |
| R3 | [2/4] | 🟡 | `actor/tests.rs` grew from 178 to 941 lines, over the 800-line limit. Split the reload tests and the event-emission and dropped-reply tests into submodules or files, for example `actor/tests/reload.rs` and `actor/tests/events.rs`. | ddb-server/src/actor/tests.rs:941 | 1 | ALICE, CARL |
| R4 | [2/4] | 🟡 | `actor/mod.rs` is 868 lines, still over the 800 limit, though down from about 1050. The 30 trivial read-verb wrappers are the bulk; a split is optional. | ddb-server/src/actor/mod.rs:868 | 2 | ALICE, CARL |
| R5 | [1/4] | 🟠 | `call<R>` does not replace the per-verb `ActorHandle` methods. All 36 remain as thin wrappers (`ddb-server/src/actor/mod.rs:414-829`), and every adapter still calls them (e.g. `ddb-server/src/rest.rs:445,454`, `ddb-server/src/schema/mutations/operations.rs:258,291`, `ddb-server/src/pgwire.rs:110`, `ddb-server/src/nosql_api.rs:38`). Only the tests call `call` directly. The spec says `call` is "the single generic entrypoint replacing 36 per-verb methods" and that callers pass the service method as a closure. It also says adding a verb touches one call site. | ddb-server/src/actor/mod.rs:414 | Phase 1 / migrate callers | BLAKE |
| R6 | [1/4] | 🟠 | Adding a verb still has several touch points, which defeats the PRD's main metric. `call` requires `R: FromActorError + IntentEmit + ReloadSignal` (`ddb-server/src/actor/mod.rs:379`). A new return type needs entries in both `no_emit_intent!` (`:147`) and `no_reload_intent!` (`:212`). A new verb also needs a wrapper method, so that is up to three edits instead of one. | ddb-server/src/actor/mod.rs:104-255 | general | BLAKE |
| R7 | [1/4] | 🟡 | Docs still describe the adapters as the schema-reload callers, which contradicts the PRD docs criterion (the table row and the sentence are server.md's own, so Task 6 missed them). server.md:73 lists "GraphQL executeSql/executeBatch (operations.rs), PgWire (pgwire.rs)" as classifier users, and server.md:1074-1076 says it is "shared by the GraphQL and PgWire transports". server.md:355 still says "per `mutations/operations.rs::executeSql`", and transport-policy-inventory.md:57 has the same shared-by wording. The only caller is now `ActorHandle::execute_sql`/`execute_batch` in `actor/mod.rs`. Fix: reword to "called once inside ActorHandle::call via ReloadIntent". | docs/src/technical/server.md:73 | 6 | ALICE |
| R8 | [1/4] | 🟡 | The reload decision is not a single derivation helper in `ddb-server/src/reload.rs`, which the spec names as the home. The logic lives in `ddb-server/src/actor/mod.rs` as `ReloadIntent` and `ReloadSignal`. The SQL-text classification is still decided separately in two wrappers (`execute_sql` at `:568`, `execute_batch` at `:574-575`), and `apply_schema` passes its own intent. So the decision is made at three sites that feed one check, and `reload.rs` is unchanged. `call` also gained a third `ReloadIntent` parameter the spec does not list. | ddb-server/src/actor/mod.rs:178-255 | Phase 1 / reload centralization | BLAKE |
| R9 | [1/4] | 🟡 | The per-surface reload acceptance test is only partly met. The spec wants a test that a schema-changing mutation on each surface (GraphQL applySchema/executeSql/executeBatch, REST schema-apply, PgWire DDL) bumps the schema version, and that a non-schema mutation does not. The unit tests prove this at the actor level. The only surface test I found is the PgWire e2e. I found no REST or GraphQL surface test of version-bump versus no-bump. | tests/e2e/integration_rest_pgwire.rs:244 | Phase 1 / reload centralization | BLAKE |
| R10 | [1/4] | 🟡 | The docs still describe the deleted file. `docs/src/technical/server.md:1045` and `docs/src/architecture/overview.md:182` list `actor/handlers.rs` as "Command handler implementations" and "Command dispatch logic". `docs/src/technical/app-contract.md:153` says GraphQL parsing happens in `actor/handlers.rs`. A stale comment at `ddb-server/src/actor/tests.rs:68` says the twin enums "still exist". The Phase 2 acceptance says the docs no longer describe the old dispatch. | docs/src/technical/server.md:1045 | Phase 2 / docs | BLAKE |
| R11 | [1/4] | 🟡 | SQL negative-path tests exercise `Vec<TableSchema>`, whose implementation always returns false regardless of intent or result. They cannot catch broken SQL error/false gating. Return `ActorResult<SqlResult>` or `ActorResult<Vec<SqlResult>>` in these tests. | ddb-server/src/actor/tests.rs:354 | 1 | BOB |
| R12 | [1/4] | 🟡 | Both actor files exceed the rubric's 800-line limit. Extract event/reload implementations into focused sibling modules and split tests by concern. | ddb-server/src/actor/mod.rs:868 | general | BOB |
| R13 | [1/4] | ⚪ | Stale comments refer to the deleted enum path or to process steps. `EventIntent` docs mention "the legacy actor_loop enum path" (mod.rs:42-43), and `IntentEmit` docs say it is "the closure-side twin of the pre-refactor emit_mutation_events" (mod.rs:102-103). The `call` docs also cite `emit_mutation_events` (mod.rs:364-366). The Phase 0 header in `tests.rs` says "Task 1.1 runs one wave BEFORE Task 1.2", "WHILE the twin enums still exist" and "drive the SAME verbs through the CURRENT verb-method path". That is now false, because the tests call `call` directly. Rewrite these comments to describe current behavior. | ddb-server/src/actor/tests.rs:63 | 5 | ALICE |
| R14 | [1/4] | ⚪ | Minor behavior widening, contradicting the "byte-identical" success metric but consistent with the PRD intent of one actor-bridge reload site. The GraphQL `sql` query field (`ddb-server/src/schema/queries.rs:304`) routes non-SELECT SQL to `actor.execute_sql`. DDL sent that way now triggers a schema reload, where it previously did not. Likely a fix, but note it in the PRD or docs. | ddb-server/src/schema/queries.rs:304 | 4 | ALICE |
| R15 | [1/4] | ⚪ | `set_reloader` silently ignores a second set and `call` silently skips reload when no reloader is registered. This follows the design, and `lib.rs` is the only production construction path. A `tracing::warn!` on the `Err` from `OnceLock::set` would make a mis-wired embedder visible. | ddb-server/src/actor/mod.rs:356 | 1 | ALICE |
| R16 | [1/4] | ⚪ | Dead leftover in `ddb-server/src/lib.rs:78-82`. `.layer(Extension(reloader.clone()))` is kept for "the REST apply handler", but `ddb-server/src/rest.rs` no longer extracts `SchemaReloader`, so the layer and its comment are stale. | ddb-server/src/lib.rs:78 | Phase 1 | BLAKE |
| R17 | [1/4] | ⚪ | Maintenance documentation now claims `ActorHandle::run_maintenance` performs compaction and returns `CompactionReport`; the implementation calls `svc.run_maintenance` and returns `MaintenanceReport`. Document the compaction path using `ActorHandle::compact`. | docs/src/technical/server.md:980 | 6 | BOB |

Severity totals: 0 critical, 2 high, 10 medium, 5 low.

## Alice

Consensus lens, implementation-aware. Verdict: behaviorally correct, no Critical
or High issues. She diffed all 36 migrated verb closures against the deleted
`handle_command` arms (including the Compact/Sync `rebuild_if_stale` tails and
the delete type-lookup relocation) and found them identical; no `ActorCommand`,
`ActorReply`, `handle_command` or "unexpected reply" references remain in code;
the five adapter reload triggers are gone and `set_reloader` is wired in
`lib.rs`. She ran `cargo clippy -p ddb-server --all-targets -- -D warnings`
(clean), the 28 `actor::` unit tests, and the 10 tests in
`graphql_apply_schema_t161` / `rest_apply_schema_t161` (pass). She did not run
the e2e suite or `mdbook build`.

Findings: R1, R2, R3, R4, R7, R13, R14, R15.

Rubric verdicts:

R1: fail
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
R13: fail

## Blake

Blind lens (PRD-only prompt; no diff, no file list, no review history). Verdict:
the closure transport works and behavior is preserved, but the migration stops
short of the spec — `call` was added beside the 36 verb methods rather than
replacing them at the call sites, so the PRD's headline metric ("adding or
routing a verb touches ONE call site") is not met. He confirmed the twin enums,
`handle_command` and `handlers.rs` are deleted, the five hand-wired reload
triggers are gone with one `trigger_reload_and_wait` call inside
`ActorHandle::call`, and that actor-gone / dropped-reply paths return structured
errors. He ran `cargo test -p ddb-server --lib actor` (28 tests pass); he did
not run clippy, `test-ci` or e2e. He also reports no security or data-safety
regressions: auth untouched, single-writer ordering and `Ok`-gated emission
preserved, delete resolves the doogat type before deleting.

Findings: R5, R6, R1, R8, R9, R16, R10.

Rubric verdicts:

B1: fail
B2: pass
B3: fail
B4: pass
B5: pass
B6: pass
B7: fail
B8: pass
B9: pass
B10: pass
B11: pass
B12: pass
B13: pass
B14: pass
B15: fail
B16: fail
B17: pass
B18: pass
B19: pass

## Bob

Doubt lens + de-slop (codex), after one retry (inlined). Raw output at
`docs/dev/tmp/bob-output-00172-c1.txt`.

FIX:
- [BOB] 🟡 SQL negative-path tests exercise `Vec<TableSchema>`, whose implementation always returns false regardless of intent or result. They cannot catch broken SQL error/false gating. Return `ActorResult<SqlResult>` or `ActorResult<Vec<SqlResult>>` in these tests. | File: ddb-server/src/actor/tests.rs:354 | Task: 1
- [BOB] 🟡 Transport tests do not prove the required reload counts: GraphQL asserts only a version increase; PgWire checks type visibility and has no non-DDL control. Assert exactly one version increment for DDL and zero for DML through each surface. | File: tests/e2e/integration_rest_pgwire.rs:247 | Task: 4
- [BOB] 🟡 `ClosureMsg.event` is unused, suppressed with an allow(dead_code) attribute, and always initialized to `None`; the real intent is already captured by `run`. Remove the field and its initializer. | File: ddb-server/src/actor/mod.rs:321 | Task: 5
- [BOB] 🟡 Both actor files exceed the rubric's 800-line limit. Extract event/reload implementations into focused sibling modules and split tests by concern. | File: ddb-server/src/actor/mod.rs:868 | Task: general
- [BOB] ⚪ Maintenance documentation now claims `ActorHandle::run_maintenance` performs compaction and returns `CompactionReport`; the implementation calls `svc.run_maintenance` and returns `MaintenanceReport`. Document the compaction path using `ActorHandle::compact`. | File: docs/src/technical/server.md:980 | Task: 6

VERIFY:
- (none)

KNOWN:
- (none)

Rubric verdicts:

R1: fail
R2: fail
R3: pass
R4: pass
R6: pass
R7: pass
R8: pass
R9: fail
R10: pass
R11: pass
R12: pass
R13: fail
D1: pass
D2: pass
D3: pass
D4: pass
D5: pass

## Carl

Gemini via the copilot backend (frontend & design specialist; backend-only diff,
so he reviewed as a generalist). He read the full diff and context, ran
`cargo test-ci` himself (250 `ddb-server` unit tests pass), and checked function
and file lengths mechanically. He judged plan compliance, event/mutation parity,
error paths, and security boundaries all satisfied, and found no frontend
surface to evaluate.

Findings: R3, R4 (both the 800-line file-size concern).

Rubric verdicts:

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
R13: fail

## Follow-up Tasks Created

None by this skill (PRD 00249 — task creation belongs to the decision gate).

Verdict: 17 findings
Tests: 1913 passed, 0 failed, 0 skipped (suite run this cycle)
