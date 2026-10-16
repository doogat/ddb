# Backlog review - ddb - 2026-07-16

Verdict: **GO** — all Blocking findings resolved in the apply pass (see Decisions applied); zero open Blocking, no pending reshapes. Standing preconditions carried forward from the 07-13 gate: install the mobile toolchains before the batch reaches 00191; the 00170 spike emits its implementation PRD to `hold/`, never backlog.

Scope: full 8-lens review of all 37 backlog PRDs (00163-00199) after today's evolution assessment (13 scope amendments, 6 new PRDs, +1 renumber at 00171). Lens D (grounding) ran as 3 parallel agents over 00163-00194 verifying every load-bearing claim against HEAD `8e4b4aa`; I grounded 00171/00195-00199 inline (authored today from source-verified findings). Comprehension: every PRD read end-to-end this session.

## Map

| # | PRD | template | lines | subsystems | depends on | verdict |
|---|---|---|---|---|---|---|
| 00163 | git-tree-construction-correctness | standard | 121 | git_ops, sync | — | FIX |
| 00164 | unify-id-minting | standard | 146 | service, sql_engine, parser | — | FIX |
| 00165 | deterministic-crdt-resolution | standard | 129 | crdt_resolver | — | FIX (minor) |
| 00166 | make-hlc-load-bearing | standard | 144 | git_ops, sync, crdt | 00165 | READY (line drift noted) |
| 00167 | sync-collision-reassignment | standard | 116 | sync_manager | 00162, 00164 | READY |
| 00168 | bundle-import-conflict-recovery | standard | 123 | bundle, sync | 00162 | READY |
| 00169 | poison-file-reindex-resilience | standard | 193 | indexer, service | — | READY |
| 00170 | id-throughput-ceiling-decision | standard | 153 | docs (spike) | 00164 | READY |
| 00171 | migrate-shell-scenarios-to-e2e | standard* | 88 | tests, CI, AGENTS.md | — | **FIX (B1+B2)** |
| 00172 | collapse-actor-plumbing | standard | 136 | ddb-server actor | — | FIX (B3) |
| 00173 | unify-server-error-mapping | standard | 155 | server error paths | — | READY |
| 00174 | decouple-maintenance | standard | 154 | actor, git_ops, remote | 00162, 00172 | READY |
| 00175 | route-read-delete | standard | 135 | contract, transports | 00172 | READY |
| 00176 | route-search | standard | 143 | contract, transports | 00172 | READY |
| 00177 | ffi-crud-through-contract | standard | 139 | ffi, contract | 00172/73/75 | READY |
| 00178 | structured-fields-input | standard | 131 | server inputs | 00172 | READY |
| 00179 | sql-identifier-escaping | standard | 165 | indexer, sql_engine | — | FIX (B4) |
| 00180 | server-security-hardening | standard | 175 | ddb-server | 00173 | READY |
| 00181 | index-health-quarantine | standard | 124 | service, transports | 00169 | READY |
| 00182 | where-filter-v2 | standard (no Priority hdr) | 161 | server filter | — | READY |
| 00183 | split-oversized-round-2 | standard (no Priority hdr) | 145 | moves | — | FIX (stale lint text) |
| 00184 | schema-apply-v2 + zone parity | standard (no Priority hdr) | 140 | schema_diff, ffi | — | READY |
| 00185 | ddb-client crate | standard | 171 | new crate | — | READY |
| 00186 | offline SDL + handshake | standard | 164 | server, cli | — | READY |
| 00187 | idempotent-upsert (catchup:force) | standard | 168 | contract, transports | 00177 | FIX (minor note) |
| 00188 | ephemeral-serve | standard | 161 | cli, server | — | READY |
| 00189 | rust-typedef-codegen | standard | 161 | cli codegen | 00186 | READY |
| 00190 | read-freshness (catchup:force) | standard | 186 | read_pool, watch | 00169 | READY |
| 00191 | ffi-git-sync (catchup:force) | standard | 156 | ffi, sync | 00163/65/67, 00177 | READY |
| 00192 | nosql-crud-parity (catchup:force) | standard | 160 | nosql_api | 00172-00178 | READY |
| 00193 | pgwire-ffi-conformance | standard (no Priority hdr) | 162 | conformance | 00192 | READY |
| 00194 | ffi-bindings-ci | minimal | 56 | CI | 00191 (seq) | READY |
| 00195 | guard-batch-rmw | (defective) | 74 | git_ops, consistency | 00162 | **FIX (B1)** |
| 00196 | sql-dml-through-pipeline | (defective) | 71 | sql_engine, service | 00175-00178 | **FIX (B1)** |
| 00197 | derive-transport-renders | (defective) | 74 | server render, ffi | 00172, 00176 | **FIX (B1)** |
| 00198 | verify-zone-default-null | (defective) | 55 | repro/conformance | — | **FIX (B1)** |
| 00199 | prune-dead-abstractions | (defective) | 38 | hygiene | — | **FIX (B1)** |

Hygiene: filenames all `NNNNN-slug-vN.md`; sequence unique across backlog/wip/done/hold (hold's `00169b` is invisible to the 5-digit scanner — archived, no collision); no stray files in backlog/; `catchup: force` frontmatter confirmed on 00187/00190/00191/00192 (survived the renumber on the right files).

## Findings

### Blocking

- **[00171, 00195-00199] A (compliance): the six 2026-07-16 PRDs skip the create-prd template** → fails as: coverage-gate block (review coverage keys on `#### Feature:` headings — none exist) + wrong-TDD lock-in (no Inputs/Outputs contracts for the test author). Specifics: no `#### Feature:` blocks (00171, 00195-00198); missing `### Module:` blocks (00195: partial; 00196/00197: tree only; 00198: no Structural section); `### Users` instead of `### Target Users` (00195-00197); 00199 matches neither `standard.md` nor `minimal.md` (invented `## Task List`). Root cause: authored directly during the assessment instead of via create-prd. Fix: rewrite all six to template (00171/00195-00198 → standard; 00199 → minimal). **Rule-mandated → applied without walkthrough.**
- **[00171 vs ~15 later PRDs] E (cross-invalidation): 00171 deletes `tests/{smoke,integration}.{sh,ps1}` at its Phase 3, but later PRDs' Phase-2 tasks instruct adding scenarios to those exact files** (00174, 00175, 00176, 00178, 00179, 00180, 00181, 00182, 00184, 00186, 00187, 00188, 00190, 00191, 00192 — verified by the comprehension pass) → fails as: stall/rework thrash — a planner mid-batch either fails to find the files or resurrects them. Introduced by today's decision to insert 00171 early. **Walked with maintainer (Decision 1 below).**
- **[00172] C (coherence, in the 2026-07-16 blockquote): "fire the reload once in the actor bridge, where every mutation already passes" is false for PgWire** — PgWire does not route through the actor (00174's own text: PgWire runs on ReadPool/its own handler; the reload trigger at `pgwire.rs:116` serves PgWire DDL). As written, a planner could delete PgWire's reload → PgWire DDL stops reloading the GraphQL schema → fails as wrong-TDD lock-in. Fix: reword — ONE derivation helper from the core signal; the actor bridge covers actor-routed transports; PgWire's DDL path calls the same helper. **Rule-mandated (factual error) → applied.**

### Non-blocking

- **[00163, 00179] A/C: today's Critical/High scope-addition blockquotes carry implementable scope with no `#### Feature:`/task decomposition.** The house precedent (07-13 blockquotes) accepts riders, but these two carry the assessment's highest-severity NEW scope (merge-lock legs; sync-load charset guard). Fix: fold into the bodies as Features + Phase tasks (00163: new "Merge path locked and fresh" capability + Phase 0 tasks; 00179: new "Charset guard at the sync-load boundary" feature + sweep-extension task). The other 11 blockquotes extend existing features/tasks — left as riders. **Applied.**
- **[00164] D: blockquote path error** — `git_ops/attachments.rs:31` does not exist; the validator is `ddb-core/src/attachments.rs:31` (grounding G1). **Applied.**
- **[00183] D: the "3 clippy lints" sub-scope was completed today** (commit `8cfe90a`) — success metric + Phase 0 sub-task now stale (grounding G2, MEDIUM). Fix: reword to "lint debt cleared 2026-07-16; verify clippy stays clean post-split". **Applied.**
- **[00163] D: H2 line refs drifted +52** (`delete_file` now `mod.rs:401`, `delete_files` `:427`, `fresh_index` doc `:259-263`) — refresh while editing (grounding G1). 00166 carries the same class of drift; left as-is (its text anchors on the fn-set, and plan-tasks re-grounds). **Applied for 00163 only.**
- **[00187] D: implementer note from grounding** — `create.rs:90` documents `on_conflict` NOT honored on the `create_doogat_with_extra` path; the ReturnExisting arm must cover it. One line added to Phase 0 acceptance. **Applied.**
- **[00165] C: blockquote item (2) duplicates the existing Phase 2 ".crdt read-back decision" task** — pointer added so the planner merges them instead of doing it twice. **Applied.**
- **[goal-gap-analysis doc] E: `2026-07-13-goal-gap-analysis.md:5` maps G1/G3 to pre-renumber numbers** and is cited as Source by 00193/00194 (slugs disambiguate). Fix: one-line pre-renumber banner at the top of that doc. **Applied.**
- **[00182, 00183, 00184, 00193] A: missing the house `**Priority:**` header line** — cosmetic (not template-required); accepted as-is, consistent with the 07-13 gate.

### Questions

None open — the two maintainer decisions this backlog needed (release cadence; harness strategy) were made earlier today and are encoded in the roadmap.

## Reshapes

None. All 37 PRDs are ≤193 lines, single-subsystem-coherent, and correctly ordered (every `Depends on:` points to a lower number — re-verified post-renumber). No merges/splits/renumbers proposed beyond today's already-executed insertion.

## Gaps

- Release cadence: handled as standing roadmap policy (not a PRD) — v0.2.7 cut decided, pending push.
- Mobile toolchain precondition before 00191: standing note in the roadmap (unchanged).
- No producer/consumer holes found: the conformance-driver chain (00175 builds `driver_rest` → 00176/00178 reuse → 00192 builds the NoSQL driver → 00193 adds PgWire/FFI) is closed; 00186→00189 SDL/codegen chain closed; 00173's `ffi_variant` surface feeds 00177/00180.

## End state after this batch

The backlog, fully drained, delivers the assessment's verified end state: all I1-I9 invariants HOLDS with regression tests (including today's merge-lock legs), one app contract with six conformance-tested transports plus a disposition registry guarding future verbs, a consumable client story (crate + SDL + codegen + upsert + ephemeral mode) with jink's ~3,650 LOC deletion ledger, mobile as a full replica with bindings CI, and the harness authored once in Rust instead of twice in shell. Remaining post-batch: the parked 00170 follow-up format-change PRD in hold/ (maintainer promotes via this gate), parameterized SQL, and the deliberately deferred maintenance-verb contract routing (assessment L13).

## Frontmatter tuning

| PRD | suggestion | why |
|---|---|---|
| 00170 | `design: skip` considered, NOT applied | it is a decision spike, but its Phase 0 blast-radius research benefits from the design step; leave default |
| 00183, 00199 | none | pure-move/hygiene, but both touch many files; default catchup is right |
| 00187/00190/00191/00192 | `catchup: force` | already set (07-13); verified intact post-renumber |

## Decisions applied

1. **B2 harness cross-invalidation (maintainer chose "Edit all PRDs now")**: every scenario-authoring task in the 16 affected PRDs ≥00172 (00174, 00175, 00176, 00178, 00179, 00180, 00181, 00182, 00184, 00186, 00187, 00188, 00189, 00190, 00191, 00192) now targets the Rust e2e harness with an explicit "(the shell harness was removed by 00171)" note; success-metric/conformance-plan stragglers in 00172/00180/00182/00193 re-synced; 00180's duplicate too-deep-query e2e task absorbed; 00189's smoke leg absorbed into its existing e2e task. Verified: `rg` finds shell-file references only in 00171 itself and 00193's historical parenthetical. PRDs 00163-00170 (run before 00171) keep the shell files, unchanged; 00163's scenario task notes this explicitly.
2. **B1 template compliance (rule-mandated)**: 00171, 00195-00198 rewritten to `standard.md` (Feature blocks with Inputs/Outputs/Behavior, Module blocks, Target Users, layered dependency graph, per-task Acceptance); 00199 rewritten to `minimal.md`. Re-verified: `#### Feature:` present in all standard rewrites.
3. **B3 (rule-mandated)**: 00172's reload blockquote corrected — one shared derivation helper; actor bridge for actor-routed transports; PgWire's DDL path keeps a thin call to the same helper (never deleted).
4. **B4 (rule-mandated)**: 00163's Critical merge-lock scope folded into the body (H3 problem statement, "Merge path locked and fresh" capability with 2 features, Phase 0 task, concurrency scenario, CLI `resurrected` feature) with H2 line refs refreshed to HEAD (`:401`/`:427`/`:259-263`); 00179 gained the "Charset guard at the sync-load boundary" feature + the extended-sink sweep task + validator-consolidation behavior, blockquote reduced to provenance.
5. **Grounding fixes (rule-mandated)**: 00164 blockquote path corrected to `ddb-core/src/attachments.rs:31`; 00183's completed lint sub-scope reworded (cleared in `8cfe90a`); 00187 gained the `create_doogat_with_extra` on_conflict coverage note; 00165's blockquote now folds its `.crdt/temp` evidence into the existing Phase 2 decision task; `2026-07-13-goal-gap-analysis.md` gained a pre-renumber numbering banner.

Post-apply lens-A re-run: all touched PRDs template-clean; every `Depends on:` still points to a lower number; filenames unchanged (no renumber in this pass).

**Gate honesty**: lens D on 00171/00195-00199 was performed inline by the reviewer (their citations originate from today's source-verified assessment, and the rewrites preserved them) rather than by an independent agent — named here as the one scope reduction. Lenses A-C and E-H ran inline over all 37 PRDs; lens D ran as 3 agents over 00163-00194 with every load-bearing claim checked against HEAD `8e4b4aa`.
