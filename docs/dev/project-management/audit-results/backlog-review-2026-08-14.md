# Backlog review - ddb - 2026-08-14

Verdict: **GO** — all 6 Blocking classes resolved in the apply pass (2026-08-15); zero open Blocking, no reshapes. The Map's per-PRD verdict column shows the pre-apply state; every FIX row is READY after the Decisions applied below. Lens A re-run on all 23 touched PRDs (heading order, feature/task closure, frontmatter at line 1) and the citation checker re-run clean (0 backlog/wip findings).

Context: batch 202607161128 completed 00163/00200/00164/00165/00166 and stopped 2026-07-19 with 00167 in `wip/` (build not started). This gate covers `backlog/` 00168-00199 (32 PRDs, 4680 lines) before the batch resumes. Grounding verified against HEAD `d77ad58` by 8 read-only subagents (ddb + jink repos); jink evidence is from jink HEAD `7b4cd56` (2026-07-17).

## Map

| # | PRD | template | lines | subsystems | depends on | verdict |
|---|-----|----------|-------|------------|------------|---------|
| 00168 | bundle-import-conflict-recovery | standard | 123 | bundle, sync_manager | 00162 (done) | READY |
| 00169 | poison-file-reindex-resilience | standard | 193 | indexer, write_lock, service, ffi | none | FIX (B1 riders) |
| 00170 | id-throughput-ceiling-decision | standard | 153 | docs, prds/hold (spike) | 00164 (done) | FIX (B3, N5) |
| 00171 | migrate-shell-scenarios-to-e2e | standard | 154 | tests/e2e, workflows, AGENTS.md | none | FIX (N4) |
| 00172 | collapse-actor-plumbing | standard | 136 | ddb-server actor | none; unblocks 00175-78 | FIX (B1 rider) |
| 00173 | unify-server-error-mapping | standard | 155 | app_contract, server error | none; feeds 00177/00180 | READY |
| 00174 | decouple-maintenance-from-write-actor | standard | 154 | actor, remote, git_ops | 00162, 00172 | FIX (B1 rider, N1) |
| 00175 | route-read-delete-through-contract | standard | 135 | service, ffi, server, cli | 00172 | READY |
| 00176 | route-search-through-contract | standard | 143 | service, ffi, server, cli | 00172 | FIX (B1 rider) |
| 00177 | ffi-crud-through-contract | standard | 139 | ffi, app_contract | 00172/173/175 | READY |
| 00178 | structured-fields-input | standard | 131 | server inputs, ffi | 00172 | READY |
| 00179 | sql-identifier-escaping-materializer | standard | 175 | sql_engine, indexer, service | none | READY |
| 00180 | server-security-resource-hardening | standard | 174 | server | 00173 | FIX (B1 riders, B4) |
| 00181 | index-health-quarantine-surface | standard | 124 | service, cli, server, ffi | 00169 | READY |
| 00182 | where-filter-correctness-completeness | standard | 159 | filter, relation_filter, materialize | none | FIX (B1 rider, Q1) |
| 00183 | split-oversized-modules-round-2 | standard | 145 | 14 core/server modules | none | FIX (B1 rider, N2) |
| 00184 | schema-apply-v2-index-evolution-ffi-zone | standard | 140 | schema_diff, ffi | none (00160/00161 done) | FIX (B6) |
| 00185 | ddb-client-rust-crate | standard | 171 | new crate | none | READY (N5 nits) |
| 00186 | offline-sdl-export-capability-handshake | standard | 164 | server schema, cli | none | FIX (B3) |
| 00187 | idempotent-upsert-return-existing | standard | 168 | app_contract, service, transports | 00177 | FIX (B5) |
| 00188 | ephemeral-serve-test-mode | standard | 161 | cli, server run() | none | FIX (N3) |
| 00189 | rust-typedef-codegen | standard | 161 | cli codegen, server naming | 00186 | READY (N5 nit) |
| 00190 | close-read-freshness-hole | standard | 186 | read_pool, watch, reload | 00169 | READY |
| 00191 | ffi-git-sync-mobile-replica | standard | 156 | ffi, sync_manager | 00163/165/167, 00177 | READY (Q2) |
| 00192 | nosql-http-full-crud-parity | standard | 160 | nosql_api, nosql core | 00172-00178 | READY (N6 note) |
| 00193 | pgwire-ffi-conformance-legs | standard | 163 | conformance harness, docs | 00192 | FIX (B1 rider) |
| 00194 | ffi-bindings-ci | minimal | 56 | workflows | 00191 (sequencing) | READY |
| 00195 | guard-batch-rmw-lost-updates | standard | 124 | git_ops, consistency | 00162 (done) | READY |
| 00196 | route-sql-dml-through-write-pipeline | standard | 126 | sql_engine, service | 00175-78, after 00183 | READY |
| 00197 | derive-transport-renders | standard | 157 | server render, sql_engine, ffi, cli | 00172, 00176 | READY (N5 nit) |
| 00198 | verify-zone-default-null-write | standard | 121 | repro + conformance | none | FIX (B2, B3) |
| 00199 | prune-dead-abstractions-hygiene | minimal | 74 | git_ops, traits, ops | none | FIX (N5) |

Hygiene: filenames all `NNNNN-{slug}-v{n}.md`; sequence numbers unique across backlog/wip/done/hold (`00169b` in hold is the absorbed original, invisible to the sequence scan by design); no stray files in `backlog/`; no `dev/local/discovery/` dir (sources are the audit-results docs, all present). Frontmatter: only `catchup: force` on 00187/00190/00191/00192, all valid.

## Findings

### Blocking

- **[B1 — 00169, 00172, 00174, 00176, 00180, 00182, 00183, 00193] A/C: 2026-07-16 "scope addition" blockquotes carry implementable scope that no `#### Feature:` or task decomposes.** The 07-16 gate folded 00163/00179 (Critical/High) and deliberately "left the other 11 as riders" judged to extend existing tasks. This pass disagrees for a subset — the rider IS the only home of genuinely new work: 00169 (index schema-version stamp; frontmatter-size cap), 00172 (schema-reload derivation helper replacing 5 hand-wired sites), 00174 (move `check_write_threshold`/`write_commit_graph` out of the lock hold — core-side, distinct from the worker split), 00176 (`PageRequest` policy), 00180 (NoSQL scan/backlinks cap; PgWire write-semantics docs; DML-triggered redaction test case), 00182 (LIKE `ESCAPE` fix), 00183 (split list 3→14 files + `sql_conn` exception decision), 00193 (verb-disposition registry + test). All rider premises re-verified live at HEAD. -> fails as: wrong-TDD lock-in / rework thrash — the review coverage gate keys on `#### Feature:` headings and plan-tasks derives tasks from Implementation Phases, so rider scope is either silently dropped (surfacing only at blind/doubt review) or planned with invented acceptance criteria. Fix: fold each rider into a Feature + phase task with an Acceptance clause (extensions of existing tasks get an acceptance-clause addition instead); shrink blockquotes to provenance, matching the 00179 pattern.
- **[00198] B (unattended feasibility): the STALE branch's acceptance requires a jink-side commit** ("correct jink's `AGENTS.md:78` gotcha (jink-side commit)"). The autopilot session runs in ddb; writing to another repo triggers permission prompts. Compounding staleness: jink HEAD `7b4cd56` (2026-07-17) already moved zones inline in `TYPE_DEFINITIONS` and removed the SET ZONE migrations, the gotcha now sits near AGENTS.md:101 in jink's dirty working tree, and the PRD's "installed v0.2.6" test pin is one release stale (0.2.7 cut 2026-07-17). -> fails as: unattended hang (cross-repo write prompt mid-loop). Fix: STALE-branch acceptance becomes "record the required jink correction in the findings report"; re-pin the matrix to "latest release + HEAD build"; note jink's inline-zone migration.
- **[B3 — 00170:17, 00186:11, 00198:11] step-1 citation resolution: three dangling citations.** 00170:17 is the illustrative wikilink `[[20260226120000]]` (not a real memory link); 00186:11 cites `jink: dev/local/prds/backlog/00026-...` which moved to jink's `wip/` (verified); 00198:11 names "PRD 00122-era" whose file was GC'd. -> fails as: stall — an implementor premise check trips on the dangling pointer. Fix: `link-ok:` waiver tokens; for 00186 also correct backlog→wip in the path.
- **[00180] D (already implemented): the ws-auth rider's "missing regression test" exists** — `tests/e2e/subscriptions.rs:59` (`ws_auth_invalid_header_returns_401`), `:108` (`ws_payload_auth_invalid_token`), `:142` (`ws_no_auth_no_payload_rejected`). -> fails as: rework thrash — a planner schedules work that is done; reviewers flag the no-op. Fix: rewrite the scope-correction note to state coverage exists; no task.
- **[00187] D (already implemented, half): GraphQL `createDoogat` already exposes `onConflict`** — declared `ddb-server/src/schema/mutations/operations.rs:48-51`, decoded via `schema/input.rs:81-90`, enum registered `type_defs.rs:450`; typed create has it too. The premise "each write-capable interface exposes the policy" was stale at authoring. REST hardcodes `ConflictAction::Error` (`rest.rs:403`), CLI/FFI lack the arg — those remain real. -> fails as: wrong-TDD lock-in — tests written for adding an arg that exists. Fix: problem statement + Phase 1 task scope the GraphQL leg to "extend the enum with `ReturnExisting` + surface `existed`"; REST/CLI/FFI keep the full wiring.
- **[00184] D (grounding): "and any declarable index" is unfounded** — `TableSchema` carries only `unique_together` (`types/schema.rs:103`); `PlanOp` has no index variant; only the `unique_together` half of the cited differ behavior exists (`differ.rs:293-295`). -> fails as: wrong-TDD lock-in — planner invents an index-declaration surface. Fix: scope both features and the Phase 0 task to `unique_together` (the only declarable uniqueness form); the risk section already says general secondary-index DDL is out of scope.

### Non-blocking

- **[00174] C:** Feature "Maintenance worker" Outputs still says "the same `ActorReply` results" — 00172 deletes `ActorReply`. Reword to "the same results, returned over the closure reply channel".
- **[00183] C:** Feature and risk text still instruct fixing the 3 `unnecessary_get_then_check` lints that the scope note says were cleared in `8cfe90a`. Strike the lint sub-scope (metrics already correct).
- **[00188] D:** risk line references "the `.ps1` scenario uses the Windows stop path" — the shell harness is gone by 00188's slot (00171). Also good news: stdout is already clean (both banners are `eprintln!` — lib.rs:49, :87; pgwire.rs:358), so the stdout-hygiene scope reduces to "emit the one JSON block".
- **[00171] C/D:** Repository Structure says "deletion (Phase 3)" but phases end at Phase 2. Mapping-table task should note: section numbers are sparse AND duplicated (two `# 50.` sections in both integration files), the `ddb-fails` helper exists only in PowerShell (bash uses bare `!`), and `tests/smoke.ps1` is invoked by no workflow (its deletion costs zero CI coverage).
- **[N5 grounding refresh, several PRDs]** 00170: post-00164 the mint is `id_minting::existence_oracle` + `write_helpers::unique_id` (clock unchanged at `parser/mod.rs:933`; `SqlEngine::unique_ids` batch-steps +1s, so the ceiling bites per call-site); the persisted clock is `git_ops/hlc_clock.rs`, not `hlc.rs`. 00199: the "CLI `date:`-defaulting duplicate" citation is wrong — `main.rs:683` is a log-filename date; defaulting lives in core (`service/create.rs:230`, `batch.rs:345`) — drop or repoint that nice-to-have. 00189: sanitize/pluralize helpers moved to `base_types.rs:933/951/980` and are `pub(crate)` — codegen needs visibility widening. 00185: `default-members` is a 3-crate subset, not the full member list. 00186: `build_schema` takes a 4th `type_schemas` param. 00197: type builder path is `schema/mutations/types.rs`, not `schema/types.rs`.
- **[00192] G:** the new NoSQL create's `on_conflict`/`existed` parity (00187 capability) is not pinned by any task — one acceptance line in the conformance task would close it.
- **[00171/00194] B (noted, accepted):** both need a mid-PRD `gh` CI dispatch + wait (full-validation; bindings jobs). Headlessly checkable via `gh run watch`-style polling; slow but not a hang.

### Questions

- **[00182] every+NULL rule** is delegated to the implementer ("Decide + implement"). Both outcomes are acceptance-testable, but pre-deciding now (keep documented 3VL vs Codex's COALESCE-fail) removes a review-disagreement surface.
- **[00191] credential-passing shape** carries an explicit `(guess)` resolved in the design phase (per-call token vs host-app callback). Autopilot's design step can decide it, but a maintainer pre-decision is one sentence now.

## Reshapes

None. No merges, splits, or renumbers: all 32 PRDs ≤ 200 lines (00169 at 193 will grow slightly when riders fold in — keep the folds terse), every cross-PRD dependency points at a lower number (00191→00167 resolves because wip precedes backlog), no duplicate scope, no order breaks, no hold-dependencies.

## Gaps

- The `PageRequest` producer/consumer pair (00176 defines, 00180 consumes) existed only in two orphaned riders — invisible to the task graph until B1 folds them in.
- 00177 deprecates raw `create_doogat`/`update_doogat`; no PRD removes them. Acceptable: the deprecation window + removal timeline is documented in `compatibility-checklist.md` per 00178's pattern; note for a future hygiene PRD.
- jink's inline-zone refactor (7b4cd56) means 00198's "jink carries a workaround migration per table" framing is already historical; the defect question (literal NULL) remains live and worth the matrix.

## End state after this batch

With 00167 + 00168-00199 done, ddb closes its last unlocked write path (bundle import), makes the index resilient to poison files and concurrent cold starts, completes the app contract (every verb `AppCommand → DoogatService → AppOutput`, one error policy, one result shape, structured fields), hardens the server (DoS guards, redaction parity, constant-time auth), reaches real 6-transport CRUD parity with a conformance harness that makes drift a CI failure (PgWire+FFI legs, NoSQL writes, bindings CI), honors git truth on reads (freshness probe + watch mode + a written consistency contract), turns mobile into a full git replica, and ships the downstream toolkit (client crate, offline SDL, codegen, ephemeral serve, idempotent create) that lets jink delete its glue. The shell-scenario tax ends early (00171), and the roadmap's "half-built app contract" root cause is retired. Still lacking after the batch: the ID-format change itself (00170 deliberately parks it in `hold/`), extended PgWire protocol, a shared read-only redb handle, and removal of the deprecated raw FFI methods — all consciously deferred with documented owners.

## Frontmatter tuning

| PRD | suggestion | why |
|-----|-----------|-----|
| 00170 | `design: skip` | decision spike; Phase 0 research IS the design — a design doc would duplicate it |
| 00194 | `design: skip` | CI wiring on a minimal template; no implementation design to review |
| 00199 | `design: skip` | independent micro-fixes; design phase adds ceremony only |
| 00174 | `catchup: force` | first consumer of the post-00172 actor restructure; batch-cached catchup will be stale |
| 00196 | `catchup: force` | runs after 00183's dml split; line refs and module layout will have moved |

## Decisions applied

Walkthrough decisions (maintainer, 2026-08-15):

1. **B1 riders → fold all 8** (applied): 00169 (+Rebuild-adjacent guards capability: schema-version stamp, frontmatter cap — 2 features + 2 Phase-1 tasks), 00172 (+Centralized schema-reload derivation feature + Phase-1 task; header dep note updated), 00174 (+Maintenance-subprocesses-after-guard-drop feature + Phase-0 task), 00176 (+One PageRequest policy feature; Phase-0 task extended), 00180 (+NoSQL scan/backlinks cap feature + Phase-0 task consuming 00176's policy — dep header now lists 00176; PgWire write-semantics folded into the Phase-2 docs acceptance; DML-error case folded into the Phase-1 redaction acceptance), 00182 (+Escape-LIKE-wildcards feature + Phase-0 task), 00183 (+Round-2 sweep feature listing the 11 additional modules with HEAD counts + sweep task; +sql_conn-exception decision feature + Phase-2 task; problem statement reframed 3→14), 00193 (+Verb-disposition registry capability + Phase-1 task + registry file in the tree). All blockquotes shrunk to provenance.
2. **B2 00198 → report, don't commit** (applied): STALE-branch acceptance now records the jink AGENTS.md correction in the findings report (no cross-repo write); binary pin updated v0.2.6 → "latest release (0.2.7+) + HEAD, record the exact version"; jink's 7b4cd56 inline-zone migration noted; `expr_to_string`/`extract_default` cites corrected to `helpers.rs`.
3. **Q1 00182 → NULL fails `every`** (applied): decision written into the feature, task, metrics, problem statement, and risk; pinned-00159-e2e + docs + CHANGELOG update is the task's acceptance.
4. **Q2 00191 → per-call token** (applied): `(guess)` replaced with the decided contract (host-owned storage/refresh, mid-sync expiry fails that sync, callback interface deferred to v2); the Phase-0 resolution task became a document-the-contract obligation.
5. **Low batch → both** (applied): 00192 conformance acceptance pins NoSQL create `on_conflict`/`existed` parity with REST; frontmatter tuning — `design: skip` on 00170/00194/00199, `catchup: force` on 00174/00196 (00187/00190/00191/00192 already had `catchup: force`).

Rule-mandated fixes (applied without walkthrough, reported here):

6. **B3 citations**: 00170:17 wikilink example waived (`link-ok`); 00186:11 jink path corrected backlog→wip + waived; 00198:11 "PRD 00122-era" waived. Checker re-run: 0 findings in backlog/wip.
7. **B4 00180**: ws-auth rider rewritten as Resolved — the "missing" regression tests already exist (`tests/e2e/subscriptions.rs:59/:108/:142`); no ws task remains.
8. **B5 00187**: GraphQL-already-wired premise fixed (arg at `operations.rs:48-51`, decode `input.rs:81-90`, enum `type_defs.rs:450`); Phase-1 task scopes the GraphQL leg to enum extension + `existed`; `ConflictAction` cite refreshed to `types/doogat.rs:494-502`.
9. **B6 00184**: "and any declarable index" scoped down to `unique_together` everywhere (feature, task, metric, risk, tree) — no index concept exists in `TableSchema`/`PlanOp`.
10. **Grounding/coherence nits**: 00170 (post-00164 mint entry points: `id_minting::existence_oracle` + `write_helpers::unique_id`, SQL batch +1s stepping; `HlcClock` at `git_ops/hlc_clock.rs`); 00171 ("Phase 3" tree labels → Phase 2; mapping-table task now encodes duplicate `# 50.` sections keyed by file:line, PowerShell-only `ddb-fails`, orphaned `smoke.ps1`); 00174 ("same ActorReply results" → closure reply channel); 00183 (lint sub-scope struck — cleared in `8cfe90a`; capability renamed; sweep-size risk added); 00185 (`default-members` 3-crate subset); 00186 (`build_schema` 4-arg signature); 00188 (stdout already clean — banners on stderr; stale `.ps1` risk line replaced); 00189 (naming helpers at `base_types.rs:933/:951/:980`, `pub(crate)` → visibility widening named); 00197 (type-builder path `schema/mutations/types.rs`); 00199 (nonexistent CLI date-default item dropped with the corrected core locations; `capitalize_first` cite fixed).

Status: all findings applied; none deferred, none rejected. Deferred-to-execution items (line-ref re-verification) are carried inside the PRDs' own re-check notes, not this review.
