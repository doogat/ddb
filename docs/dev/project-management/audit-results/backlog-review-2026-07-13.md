# Backlog review - ddb - 2026-07-13

Verdict: **GO** — all 6 Blocking findings resolved in the apply pass (see Decisions applied). One standing precondition from the user's Finding-6 choice: **install the mobile toolchains before the batch reaches 00190** (iOS targets `aarch64-apple-ios`/`aarch64-apple-ios-sim`/`x86_64-apple-ios`, Android targets `aarch64-linux-android`/`x86_64-linux-android`, full Xcode, a real NDK at `ANDROID_NDK_HOME`, `cargo-ndk`, `kotlinc`) — 00190's Phase 2 acceptance was deliberately kept as real platform builds. If the loop reaches 00190 before the installs land, that PRD will fail its Phase 2 task and pause.

Scope: 29 PRDs in `dev/local/prds/backlog/` (00163-00191 incl. `00169b`). `wip/` and `stalled/` empty; `done/` ends at 00162. Law: create-prd SKILL.md + assets as of today. Grounding: all 29 PRDs verified against the repo by 7 parallel read-only agents; **no PRD is already implemented**; no overlap with post-Jul-7 commits (only 00161/00162 work landed).

## Map

| # | PRD | template | ~lines | subsystems | depends on | verdict |
|---|---|---|---|---|---|---|
| 00163 | git tree-construction correctness | standard | 119 | git_ops, sync_manager | — (pairs 00162 done) | READY (nb: add CHANGELOG task) |
| 00164 | unify ID minting | standard | 145 | service, parser, sql_engine | — (x-ref 00170) | READY |
| 00165 | deterministic CRDT resolution | standard | 128 | crdt_resolver | — | READY |
| 00166 | make HLC load-bearing | standard | 143 | git_ops, sync_manager, crdt_resolver, hlc | 00165 | READY |
| 00167 | sync collision reassignment atomicity | standard | 117 | sync_manager | 00162✓, 00164 | READY |
| 00168 | bundle-import conflict recovery | standard | 124 | bundle, sync_manager | 00162✓ | READY |
| 00169 | indexer rebuild resilience (merged: poison-file skip + cold-start rebuild lock) | standard | ~197 | indexer, git_ops lock, service, app_contract, ffi, cli | — | READY (post-merge) |
| ~~00169b~~ | serialize cold-start index rebuild | minimal | 72 | — | — | MERGED into 00169; archived in hold/ |
| 00170 | ID throughput ceiling decision (spike) | standard | 154 | docs, prds/hold | 00164 | READY (post-fix: emits to hold/) |
| 00171 | collapse actor plumbing | standard | 135 | server actor, resolvers | — (unblocks 174-177) | READY (nb: call-site count) |
| 00172 | unify server error mapping | standard | 156 | server error, app_contract, ffi | — (feeds 176/179) | READY (post-fix: InvalidInput) |
| 00173 | decouple maintenance from write actor | standard | 153 | server actor, git_ops remote, nosql | 00162✓, 00171 | READY (nb: false RemoteCallbacks premise) |
| 00174 | route read/delete through contract | standard | 136 | app_contract, service, ffi, server, cli, conformance | 00171 | READY (post-fix: driver_rest in scope) |
| 00175 | route search through contract | standard | 142 | app_contract, service, ffi, server, cli | 00171 | READY (inherits 00174 driver fix) |
| 00176 | FFI CRUD through contract | standard | 140 | ffi, app_contract | 00172, 00171, 00174 | READY (nb: line-range cite) |
| 00177 | structured fields input | standard | 132 | server schema, rest, ffi | 00171 | READY (nb: `--field`→`--set`) |
| 00178 | SQL identifier escaping | standard | 164 | sql_engine, indexer, service, server filter | — | READY |
| 00179 | server security/resource hardening | standard | 172 | server (schema/lib/pgwire/auth/config) | 00172 | READY |
| 00180 | index-health quarantine surface | standard | 125 | service, cli, server, ffi | 00169 | READY |
| 00181 | where-filter correctness v2 | standard | 160 | server filter/relation_filter, materialize | — | READY (nb: skip-site file) |
| 00182 | split oversized modules round 2 | standard | 144 | server filter/queries, sql_engine helpers | — | READY |
| 00183 | schema apply v2 + FFI zone parity | standard | 141 | schema_diff, service, ffi | — (00161 done) | READY (nb: constant location) |
| 00184 | ddb-client Rust crate | standard | 172 | NEW crate | — (x-ref 00172) | READY (nb: jink metric phrasing) |
| 00185 | offline SDL export + capabilities | standard | 165 | server schema/lib, cli | — | READY |
| 00186 | idempotent create return-existing | standard | 164 | app_contract, service, all transports | 00176 (soft 00177) | READY (nb: UpsertOutcome.created prior art) |
| 00187 | ephemeral serve test mode | standard | 162 | cli, server lib/config | — | READY |
| 00188 | rust typedef codegen | standard | 162 | cli codegen, server naming | 00185 | READY |
| 00189 | close read-freshness hole | standard | ~188 | read_pool, indexer, actor, reload, watch(new), docs | 00169 (rebuild lock) | READY (post-fix: disclaimer + catchup:force) |
| 00190 | FFI git sync mobile replica | standard | ~157 | ffi, git_ops remote, sync_manager, docs | 00163/00165/00167, 00176 | READY — **conditional on toolchain install** (user decision) |
| 00191 | NoSQL HTTP full CRUD parity | standard | 157 | server nosql_api, nosql core, conformance | 00171-00177, 00172 | READY (nb: NoSQL harness driver parenthetical) |

Order sanity: every stated dependency points to a lower number ✓ (the sole exception is 00189→"00169b", unorderable by the 5-digit rule — Blocking 1). Roadmap `dev/local/prds/roadmap-2026-07.md` confirms numeric order = execution order but predates 00189-00191 (stale).

## Findings

### Blocking

1. **[00169b] Hygiene/A — filename breaks the selection contract.** `00169b-…` violates `NNNNN-{slug}-v{n}.md`; run-autopilot Phase 0 picks "lowest sequence number (by `00XXX-` prefix)". "00169b" extracts prefix "00169", which collides with the real 00169 once it reaches `done/` (cross-dir uniqueness) and leaves the pick order undefined. 00189's `Depends on: 00169b` is likewise unorderable under numeric drain. Context: the suffix was a deliberate 2026-07-08 splice (old 00189 renamed to run early). → fails as: **selection break**. Fix options: merge into 00169 (same file `indexer/rebuild.rs`, same availability theme; 00189's dep line becomes "00169"), or renumber pair (00169b→00192, 00189→00193 + reference edits), or waive.
2. **[00170] B — Phase 2 emits an ID-format implementation "PRD stub" into `backlog/` mid-batch.** The autopilot loop drains backlog until empty, so the stub (a public ID-format change: wikilink migration, cross-transport conformance) would execute unattended at the batch tail without ever passing this review gate — and "stub" by definition under-specifies. The roadmap explicitly lists "Any ID-format change" as deliberately deferred. → fails as: **wrong-TDD lock-in / stall on an unreviewed tail PRD**. Fix: emit into `dev/local/prds/hold/` for attended review (recommended), or require a full create-prd-compliant PRD and accept same-batch drain, or hold 00170.
3. **[00172] C/D — success metric names a nonexistent error category.** Metric line says "HTTP status derives from AppErrorCategory (Conflict→409, **Validation**→400, …)". Actual enum (`app_contract/error.rs:12-17`): `NotFound, InvalidInput, Conflict, Internal` — no `Validation`. The PRD itself uses `InvalidInput` correctly in three other places. A test author treating the PRD as spec could mint a fifth variant. → fails as: **wrong-TDD lock-in** (one-value-per-name violation). Fix: word swap Validation→InvalidInput.
4. **[00174] D/B — the 00148 conformance harness has no REST driver, but Phase 2 assumes one.** Harness exists at `tests/e2e/conformance/` with `driver_cli` + `driver_graphql` only; `InterfaceId::Rest` is declared but never exercised. 00174/00175/00177 Phase 2 tasks say "add a step … CLI+GraphQL+REST parity" — hiding the work of building `driver_rest`. → fails as: **rework thrash** (acceptance unachievable as planned; scope surprise mid-loop). Fix: declare `driver_rest` construction in scope in 00174's Phase 2 task (00175/00177 then inherit it); add a parenthetical in 00191 that its NoSQL leg includes building the NoSQL driver.
5. **[00189] E — references `ddb-server/src/actor/handlers.rs`, which 00171 deletes.** Problem statement cites `handlers.rs:155-161` and Structural Decomposition maps `actor/handlers.rs` to two capabilities; 00171 (runs 18 PRDs earlier) removes `handle_command`/handlers.rs entirely, and 00173 moves Sync onto a maintenance worker. 00173 carries a pre-00171 disclaimer; 00189 does not. → fails as: **cross-invalidation** (planning invents Locations against deleted files). Fix: add the same "line references describe the pre-00171/pre-00173 actor" disclaimer, name the target abstractly (the sync execution path on the maintenance worker), and set `catchup: force`.
6. **[00190] B — Phase 2 acceptance requires `dev/bin/build-xcframework` + `build-android` to exit 0; both are confirmed impossible on this machine.** Env check: only `aarch64-apple-darwin` target installed (no iOS/Android targets), Command Line Tools only (no full Xcode for `-create-xcframework`), `ANDROID_NDK_HOME` points to a nonexistent path, `cargo-ndk` and `kotlinc` absent. → fails as: **unattended hang / rework-cap pause** on an environment failure no rework can fix. Fix: scope acceptance to `uniffi-bindgen` generation (pure cargo) + defer platform builds to CI/manual with a note, or install the toolchains before the batch, or hold 00190.

### Non-blocking

- **[00173] false premise**: "remote.rs already installs RemoteCallbacks" — actual `remote.rs` (26 lines) passes `None` for options/callbacks in both `fetch` and `push`, and no `pull` fn exists. The deadline work must install FetchOptions/PushOptions+callbacks first. Edit the premise so design/planning sizes it right.
- **[00177] wrong CLI flag**: `--field` does not exist; the repeatable key=value flag is `--set` (`main.rs:282-284/:303-305`), removal `--unset`.
- **[00176] mis-cited range**: the lossy FFI error mapping spans `records.rs:72-129`; the Conflict→Git/Sync→Git/Index→Sql collapses are at `:98-100`, not inside `:111-126`.
- **[00171] call-site undercount**: read pool has ~21 closure call sites (20× `with_service` + 1× `with_service_mut`), not 10. Thrust unaffected.
- **[00170] stale conditional remnants**: feature heading "(conditional)", exit criteria "either fully closed ('keep')…", and the "if keep" success metric contradict the 2026-07-13 pre-decision; scrub. Also design-decisions.md cites drifted to :36/:40.
- **[00189] cosmetic cites**: reload triggers exist on three applySchema surfaces (GraphQL, REST, PgWire DDL), not two; watcher mention at design-decisions.md:86.
- **[00181] wrong file for the self-ref skip**: the deliberate `table_name == current.table_name` skip lives in `filter.rs:816-821` (fn `reverse_relations`), not `relation_filter.rs`. (Structural Decomposition already maps filter.rs for reverse-relation discovery, so impact is small.)
- **[00183] constant location**: `SCHEMA_UNSUPPORTED_CHANGE` is defined in `app_contract/error.rs` and attached in `service/schema_apply.rs`; `differ.rs:293-295` pushes a plain string into `plan.unsupported`. Only `unique_together` has such a branch (no separate "index" branch).
- **[00182] count drift**: helpers.rs is 1544 lines (claimed ~1450); filter.rs=3397 exact; build_query_fields ≈842 exact (queries.rs:52→EOF). The 3-lint claim is compile-time-only (unverifiable statically) — acceptance should not hard-depend on the count being exactly 3.
- **[00186] prior art**: `UpsertOutcome { id, created }` already exists on the singleton path (`service/create.rs:19-22`). The PRD's "no flag on any create result" is overstated; reconcile the `existed` naming with (or note divergence from) `created`.
- **[00163] + [00181] missing CHANGELOG task**: both are user-visible fixes; per the changelog mandate their integration tasks should name the CHANGELOG entry (every other fix PRD does).
- **[00184/00186/00187/00188] jink-side success metrics**: "jink deletes X LOC" is not verifiable headlessly in this repo and can trip the blind (PRD-only) reviewer. Rephrase to enablement ("covers everything jink's hand-rolled X does, so jink can delete it").
- **[00187] tempfile is a dev-dependency only** in all three crates; runtime `TempDir` needs promotion to a normal dep (implementer-obvious; noted for completeness).
- **Roadmap stale**: `roadmap-2026-07.md` predates 00189-00191 and the 00170 pre-decision; append the Phase-3 purpose-gap rows and update the "Not scoped here" ID-format line.
- **Line-drift note (no action)**: `git_ops/mod.rs` references in 00163/00166 drifted ~+50 lines (delete_file :401, delete_files :427, commit_file :308, fresh_index doc :259-262); symbols and behavior match exactly. 00166 nuance: 7 distinct `.commit(` sites (commit_file/rename_file delegate); the no-trailer/no-chokepoint substance holds.

### Questions

- **[.crdt write-state]** Critique §8.3 recommends "stop writing never-read `.crdt` state either way"; the maintainer-decisions section closed 5 items and says "No items remain undecided", but no backlog PRD covers this and it wasn't explicitly ruled. Deliberately dropped, or future PRD?
- **[00190] credential shape** carries one `(guess)` (per-call token vs callback interface) with an explicit Phase 0 design-step resolution task — sanctioned pattern, no action needed; flagged for visibility only.

## Reshapes

- **00169b → merge into 00169** (recommended): same module (`indexer/rebuild.rs`), same availability theme; restores filename compliance and makes 00189's dependency orderable ("00169"). Alternative: renumber 00169b→00192 and 00189→00193 (requires editing 00189's dep lines, 00191's consistency-page cross-ref, roadmap, and 3 memory files). The suffix was a deliberate 2026-07-08 splice, so "waive" is a legitimate third option.

## Gaps

- **Deprecation debt without removal PRDs** (accepted, note only): FFI raw `create_doogat`/`update_doogat` stay `#[deprecated]` (00176); string-form `fields` keeps a deprecation window (00177). No removal PRD exists for either — fine for now, revisit next assessment.
- **00173's "shared read-only redb handle" follow-up** may be obsoleted by 00191's Phase 0 read-store decision (which can retire redb's serving role). No action; the 00191 decision should mention it.
- **Batch/sync/sql/discovery verbs not contract-routed** — deliberately deferred per roadmap ("decided when invariant I7 flips to HOLDS"). Consistent.
- **.crdt never-read state** — see Questions.

## End state after this batch

Data-safety invariants all enforced with regression pins (write lock ✓ done, merge deletions, fresh-index deletes, unified minting, deterministic CRDT, load-bearing HLC, atomic collisions, honest bundles). Indexer survives poison files and concurrent cold starts, with an operator-visible quarantine surface. One app contract with one error policy spans all six transports, FFI included; NoSQL HTTP reaches full CRUD as the contract's acceptance test; search/read/delete/fields drift is gone. Server hardened (DoS guards, constant-time auth, PgWire redaction+cap), SQL identifiers escaped everywhere. Downstream story: published client crate, offline SDL + Rust codegen, idempotent create, ephemeral test mode — jink deletes ~2-3K LOC of glue. Purpose gaps closed: reads honor git truth (probe + opt-in watch + written consistency contract), mobile becomes a full git replica over FFI. Still open after the batch: the ID-format implementation PRD (emitted by 00170 — held for review under the accepted fix), the two deprecation removals, redb read-concurrency (possibly mooted by 00191), and non-CRUD verb routing (deferred by decision).

## Frontmatter tuning

| PRD | suggestion | why |
|---|---|---|
| 00186, 00189, 00190, 00191 | `catchup: force` | They execute late in the batch against a codebase reshaped by 00171-00177; the batch catchup cache will be stale (00189 also per Blocking 5) |
| 00170 | `design: skip` | The spike IS the design activity; a design doc about producing a decision doc is ceremony |
| 00182 | `design: skip` | Pure-move splits with a byte-equivalence oracle; nothing to design |
| 00189 | `rework_cap: 5` | Hardest PRD in the batch (hot-path probe + new watcher + schema reload + contract page) |

## Decisions applied

All choices made interactively 2026-07-13; every edit below is applied.

1. **00169b → merged into 00169** (user choice: recommended option). 00169 rewritten as "Indexer rebuild resilience: poison-file skip and serialized cold-start rebuild" — new capability `Serialized cold-start rebuild` (features: Cross-process rebuild lock; Double-checked locked rebuild), write_lock.rs + concurrent_writes.rs added to the structure, lock tasks folded into Phases 0-2, lock-ordering risk added. Original archived at `dev/local/prds/hold/00169b-serialize-cold-start-index-rebuild-v1.md`. 00189's four `00169b` dependency references retargeted to 00169. Roadmap + 3 memory files updated. Template heading walk re-run on the merged file: compliant.
2. **00170 → follow-up PRD goes to `hold/`** (user choice: recommended). Phase 2 task, feature, structural decomposition, dependency graph, success metrics, exit criteria, and test scenarios all rewritten: complete create-prd-compliant PRD into `dev/local/prds/hold/`, promoted only via /review-prd-backlog. Stale "conditional"/"keep" remnants scrubbed; design-decisions.md cites corrected to :36/:40.
3. **00172 → InvalidInput swap + collision note** (user choice: swap + note). Metric now reads InvalidInput→400 with an explicit "no Validation category variant" warning. Also: FFI allowlist cite widened to records.rs:72-129 (here and in Transport Impact).
4. **00174 → driver_rest declared in scope** (user choice: recommended). Phase 2 harness task now names building `driver_rest` explicitly (acceptance includes its existence); 00175/00177 harness tasks note "REST driver lands in 00174"; 00191's conformance task notes building the NoSQL HTTP driver.
5. **00189 → disclaimer + catchup: force** (user choice: recommended). Pre-00171/00173 note added after the problem statement; structural decomposition and Phase 1 task retargeted to the post-refactor sync execution path; reload-trigger and watcher cites corrected; frontmatter `catchup: force` added.
6. **00190 → PRD unchanged; user installs toolchains before the batch** (user choice: install toolchains). The GO above is conditional on that install. Missing today: all 5 mobile rustup targets, full Xcode (CLT only), NDK (env var points at a nonexistent path), cargo-ndk, kotlinc.
7. **.crdt leftover → 00165 docs task** (user choice: note in 00165). The resolver session verifies whether persisted CRDT state is ever read back and records keep-or-stop in design-decisions.md.
8. **Non-blocking corrections — all four groups applied** (user choice: all): factual fixes in 00171 (~21 call sites), 00173 (no RemoteCallbacks exist; fetch/push only — premise, feature, module, metrics, exit criteria), 00176 (:72-129 ×3), 00177 (--set/--unset), 00181 (skip lives in filter.rs:816-821), 00182 (1544 lines; lint count marked re-verify), 00183 (SCHEMA_UNSUPPORTED_CHANGE location); CHANGELOG task lines added to 00163 + 00181; jink metrics rephrased to enablement in 00184/00186/00187/00188; 00186 gains the UpsertOutcome.created prior-art risk; roadmap-2026-07.md refreshed (2026-07-13 update paragraph, merged-00169 row, Phase 3b table for 00189-00191, ID-format deferral rewritten).
9. **Frontmatter** (user choice: catchup group only): `catchup: force` added to 00186, 00190, 00191 (00189 via decision 5). `design: skip` (00170/00182) and `rework_cap: 5` (00189) were offered and declined — defaults stand.

Post-apply verification: 29 backlog filenames all match `NNNNN-{slug}-v{n}.md`; hold/ contains 00169b; merged 00169 passes the standard-template heading walk; the only remaining "00169b" string in backlog/ is 00169's provenance note; frontmatter blocks sit at line 1 of all four files.
