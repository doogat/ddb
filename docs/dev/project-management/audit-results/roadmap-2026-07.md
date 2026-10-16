# ddb roadmap to end state (2026-07)

**Autopilot picks the lowest-numbered backlog PRD first (SKILL.md Phase 0), so this numeric order IS the execution order.**

Derived from the five-audit architecture review (`dev/local/audit-architecture-growth-2026-07.md`). The backlog was renumbered so that PRD number equals execution order: the P0 emergency work sorts first, then convergence/identity (P1), then the app-contract + hardening block (P2), then hygiene/feature, then downstream enablement (P3). Within any phase, each PRD's `Depends on:` header still points only to a lower-numbered PRD.

**State as of the 2026-07-08 backlog review** (`dev/local/audit-results/backlog-review-2026-07-08.md`; **pre-2026-07-16 numbering** — offset +1 for numbers ≥00171): 00162 (write lock) SHIPPED and moved to `done/` (commits ff10bbe, c25251c, f2145ff). 28 PRDs authored; 27 remain in backlog: 00163-00188 plus 00169b. Adjustments applied: collapse-actor and decouple-maintenance swapped (collapse runs BEFORE decouple, so the maintenance worker is built once, on the closure model); the cold-start rebuild lock was renamed into the 00169 line; the escaping PRD gained the where-compiler quoting leg; the client crate switched to dev-dependency-only code parity; the idempotent-create FFI leg retargeted to `create_structured`.

**Update 2026-07-13** (purpose-fit critique + backlog review `dev/local/audit-results/backlog-review-2026-07-13.md`; **pre-2026-07-16 numbering** — offset +1 for numbers ≥00171): 00169b MERGED into 00169 (one PRD owns indexer rebuild resilience; the filename returns to the 5-digit selection contract; the original is archived in `dev/local/prds/hold/`). Three purpose-gap PRDs appended from the 2026-07-12 critique (read-freshness, FFI mobile replica, NoSQL CRUD parity). 00170 is pre-decided (identity correctness: UTC mint + node discriminator); its follow-up implementation PRD is written to `dev/local/prds/hold/` for maintainer review, NOT into this batch. `catchup: force` set on the late-batch PRDs (idempotent-create, read-freshness, mobile replica, NoSQL parity — post-shift: 00187/00190/00191/00192). Precondition before the batch reaches the mobile-replica PRD (now 00191): install the mobile toolchains (iOS/Android rustup targets, full Xcode, NDK, cargo-ndk, kotlinc); setup instructions in README.md → Development → "Platform packaging (UniFFI bindings)".

**Update 2026-07-16** (delta assessment `dev/local/audit-results/evolution-assessment-2026-07-16.md` + maintainer decisions): **37 PRDs in backlog: 00163-00199.** Two changes to the sequence itself:

- **Renumber**: by maintainer decision, a new PRD **00171 migrate-shell-scenarios-to-e2e** (kills the .sh/.ps1 double-authoring tax — assessment F5) was INSERTED immediately before the contract block; former 00171-00198 shifted to **00172-00199**. All living cross-references (backlog PRDs, this roadmap, AGENTS.md, invariants.md, design-decisions.md, memories) were re-synced. Documents dated before 2026-07-16 cite pre-shift numbers: offset +1 for anything ≥00171.
- **Release cadence is standing policy**: jink's adoption PRDs probe the *deployed binary*, so done-but-unreleased ddb work delivers zero downstream value. Cut a release after each roadmap phase completes. Decision 2026-07-16: **cut v0.2.7 as soon as the pending push lands and CI is green** (unblocks jink 00026 — its gates 00158/00159/00160 are done and unreleased since v0.2.6, 2026-06-19).

Scope-addition blockquotes (dated 2026-07-16, inside each PRD) landed on: **00163** (CRITICAL: merge/sync writes bypass the write lock + cached merge index — assessment F1; plus CLI `resurrected` output), **00164** (DoogatId shape validator), **00165** (map_to_yaml quoting + `.crdt/temp` lifecycle), **00166** (HLC depth cap), **00169** (index schema-version stamp + frontmatter size cap), **00172** (centralized schema-reload trigger), **00174** (maintenance subprocesses out of the lock hold), **00176** (PageRequest policy), **00179** (sync-load charset boundary + full sink list + validator consolidation), **00180** (G5 corrected — /ws auth already enforced, regression test only; NoSQL scan caps added), **00182** (LIKE wildcard escaping), **00183** (split list refreshed to 14 files; owns the `sql_conn` exit), **00193** (verb-disposition registry). New PRDs appended: **00195** guard-batch-rmw-lost-updates, **00196** route-sql-dml-through-write-pipeline, **00197** derive-transport-renders-from-core-types, **00198** verify-zone-default-null-write (UNVERIFIED jink defect), **00199** prune-dead-abstractions-and-hygiene. Every PRD depends only on lower numbers (verified). CI note: Test/Nightly red 2026-07-08..16 was toolchain drift (cargo-deny advisory + clippy test lints), fixed in `8cfe90a`; the Tier-1 lint gate is now `cargo clippy --workspace --all-targets`.

**Update 2026-07-17** (delta check): **PRD 00200 make-merge-tree-three-way SPLIT out of 00163** (maintainer decision 2026-07-16, mid-design). The 00163 design review proved the prescribed `Delta::Deleted` patch unsound (gated only by the conflict set it deletes ours-created files) and found a worse pre-existing defect: the existing `Added|Modified` arms silently revert ours' non-conflicting edits on every conflicted merge. 00163 keeps the mechanics — H2 delete freshness (**SHIPPED** 2026-07-16 as T1, commits 055b7fe+8d3868d), H3 merge locking/fresh staging, CLI `resurrected` line; **00200 owns merge-tree content correctness** (true three-way classification against the merge base). **00200 lives in `wip/`, not backlog**: it runs immediately after 00163 via wip-precedence, so its number deliberately does NOT participate in the lowest-number-first backlog order (queueing it as backlog 00200 would bury a shipping P0 behind 36 PRDs). Empirical repros: `dev/local/designs/00200-make-merge-tree-three-way-v1-evidence.md`. Release-gate status: push landed 2026-07-16; Test green 2026-07-16; Nightly green 2026-07-17 at HEAD 8d3868d — **v0.2.7 CUT 2026-07-17** (tag pushed at 58d7d22, preflight nfr01/nfr02 green; includes 00158-00162 + the 00163 T1 delete-freshness fix). jink 00026 is unblocked once CI publishes the release artifacts.

## End state

1. **Safe** — no silent data loss under concurrency, multi-process, sync, or crash. All git writes locked (merge/sync paths included); merges preserve deletions; conflict resolution is deterministic; the HLC actually orders; one bad file never bricks the store.
2. **Coherent** — one app contract every transport adapts; one error policy; one ID-minting path; one SQL-identifier-escaping discipline. Adding a capability is one contract edit plus thin adapters, not twelve files.
3. **Consumable** — a published client crate, offline SDL + codegen, structured typed inputs, upsert, and an ephemeral test mode. jink deletes ~2/3 of its integration code; any new downstream starts from the client, not from scratch. **Delivery transits through releases — cut one per phase.**

## Sequencing rules (all already satisfied by numeric order)

- **P0 before everything.** 00162 DONE; 00163 in progress (T1/H2 shipped 2026-07-16; merge/sync lock legs from assessment F1 remain); **00200 (split 2026-07-16) runs immediately after 00163 via wip-precedence**, outside the backlog numeric order. 00162+00163+00200 together close the write-path data-loss class.
- Within a phase, respect each PRD's `Depends on:` header (every dependency is a lower number).
- **00162 (write lock) + 00164 (ID minting) before 00167** (single-commit collision-reassignment runs inside the write lease and mints the loser ID through the unified oracle).
- **00162 before 00168** (bundle-import recovery runs inside the write lease).
- **00169 (poison-file core) before 00181** (the index-health quarantine surface lists what the lenient skip quarantines).
- **The cold-start rebuild lock rides inside 00169** (merged 2026-07-13): reuses the shipped 00162 lock helper (`git_ops/write_lock.rs`), closes the availability gap the 00162 e2e currently masks with a warm-up workaround.
- **00164 (ID minting) before 00170** (the ID-ceiling decision spike builds on the unified mint path).
- **00171 (harness migration, inserted 2026-07-16) runs after P1, before the contract block** — no dependencies; every later PRD's scenario authoring drops to one Rust test. Must NOT depend on 00188 (ephemeral serve, higher number); it builds its own spawn helper.
- **00172 (collapse actor plumbing) before 00174 (decouple-maintenance) and before 00175/00176/00177/00178** — the twin enums go first, so the maintenance worker is built once on the closure model and routing verbs is one touch point.
- **00162 before 00174** (the maintenance worker needs the write lease).
- **00173 (error policy) before the FFI error-variant rewiring in 00177 and the PgWire redaction in 00180** (they consume its `ffi_variant`/redaction surface).
- **00186 (SDL export) before 00189 (codegen).**
- **00177 before 00187** (the idempotent-create FFI leg rides `create_structured`/`CreateResultRecord`).
- 00183 (module splits) is pure-move hygiene; it sorts after the Phase-2 refactors it once eased. **00196 (SQL-DML pipeline) should sequence WITH or AFTER 00183's dml.rs split** (replan-floor lesson).
- New-PRD deps: 00195 needs 00162 (done); 00196 needs 00175-00178; 00197 needs 00172/00176; 00198/00199 need nothing.

## Phase 0 — stop the bleeding (P0, data-loss)

| PRD | Scope | Status / jink impact |
|---|---|---|
| 00162 | Cross-process advisory write lock across the 8 CRUD git write paths | **DONE** (ff10bbe + c25251c + f2145ff); no silent commit loss when jink runs `ddb serve` while the user runs the CLI |
| 00163 | Delete uses `fresh_index()` (H2); **merge/sync path locked + fresh staging base (F1/H3)**; CLI prints `resurrected` — merge-tree content moved to 00200 | **IN PROGRESS**: T1 (H2) shipped 2026-07-16 (055b7fe+8d3868d), T2-T4 pending. No lock-bypassed or freshness-dropped writes |
| 00200 | Conflicted-merge tree rebuilt from `merge_commits`' index + CRDT-resolved-blob overlay (**Alt B, maintainer-decided 2026-07-17** — design phase implements, does not re-open; W1-W4 + renames = acceptance floor) | **QUEUED in `wip/`** — runs immediately after 00163 (split 2026-07-16). jink multi-device edits, creates, and deletes survive conflicted syncs |

## Phase 1 — convergence, availability, identity (P1)

| PRD | Scope | Depends | jink impact |
|---|---|---|---|
| 00164 | Unify ID minting through one repo-aware path; stop future-dating batch IDs; one `DoogatId` shape validator | — | No silent overwrite of jink-created doogats when it mixes `createMany` + single create |
| 00165 | Deterministic CRDT resolution via stable Automerge actor IDs; fix `map_to_yaml` quoting; decide `.crdt/temp` lifecycle | — | Multi-device jink users converge on one winner, not a coin flip |
| 00166 | Make the HLC load-bearing (trailers on ALL commit paths, `recv_hlc` on merge, unified tie-break, explicit revwalk depth-cap handling) | 00165 | Correct, predictable last-writer-wins across jink's devices |
| 00167 | Atomic sync collision-loser reassignment (single-commit winner+loser, deterministic loser ID) | 00162, 00164 | Collision resolution never strands or duplicates a loser between jink devices |
| 00168 | Bundle-import conflict recovery (state-based detection, `--abort` on error, ref-delete only after success) | 00162 | Offline bundle sync between jink devices doesn't silently lose data or report false success |
| 00169 | Indexer rebuild resilience: lenient incremental reindex + serialized cold-start rebuild + index schema-version stamp + frontmatter size cap | — (00162 lock helper shipped) | One bad synced `.md` doesn't brick jink's backend; concurrent cold starts don't error |
| 00170 | Decide the ID throughput ceiling & format (autonomous design spike → design-decisions.md) | 00164 | Determines whether jink can bulk-create faster than ~1/sec |

## Phase 2 — leverage + finish the app contract + hardening (P2)

| PRD | Scope | Depends | jink impact |
|---|---|---|---|
| 00171 | **Migrate smoke/integration shell scenarios to the Rust e2e crate** (inserted 2026-07-16; kills the 4-file authoring tax) | — | Indirect: every later capability ships with one test authoring pass instead of four |
| 00172 | Collapse actor command/reply twin enums into a closure message; centralize the schema-reload trigger in the actor bridge | — | Indirect: every capability jink needs ships faster |
| 00173 | Unify error rendering into ONE policy (4-path fan-out + missing SqlEngine arm) | — | Consistent error codes/redaction across every transport jink uses |
| 00174 | Decouple sync/compact from the write actor onto a closure-model maintenance worker; bound remote ops (30s default); move `git maintenance`/commit-graph out of the lock hold | 00162, 00172 | A long sync/compact doesn't stall jink's user-facing requests |
| 00175 | Route read/delete through the app contract (wire the dead DTOs) | 00172 | Consistent read/delete semantics + surfaced warnings |
| 00176 | Route search through the contract; fix cross-transport field/filter drift; one `PageRequest` policy | 00172 | REST/FFI search gains filters; consistent result shape deletes jink's search glue |
| 00177 | Route FFI CRUD through the contract (structured create/update + warnings + policy-derived error variant) | 00172, 00173, 00175 | Future mobile jink gets create/update parity + warnings + consistent error categories |
| 00178 | Structured `fields` input (JSON object + native scalars, not JSON-string) | 00172 | Deletes jink's `build_fields_json` + boolean-string workarounds — day-one win |
| 00179 | Escape SQL identifiers in materializer/delete/where-compiler + `dml.rs`/`relation_filter.rs`; charset-validate at BOTH write and sync-load boundaries (`schema_from_parsed`); consolidate the validator pair | — | An odd/malicious synced type name can't corrupt jink's index |
| 00180 | Server security & resource hardening (depth/complexity, body limit, const-time auth, bind warning, PgWire redaction+row-cap, WS-auth regression test, NoSQL scan caps) | 00173 | Safer if jink exposes ddb beyond localhost; a runaway query can't pin the server |
| 00181 | Index-health quarantine listing surface (`ddb doctor` / `indexHealth` / REST / FFI) | 00169 | jink operators can see which synced file was quarantined and why |

## Existing hygiene / feature work

| PRD | Scope | Note |
|---|---|---|
| 00182 | GraphQL where-filter correctness & completeness v2 + LIKE wildcard escaping | Real; the date-`orderBy` gap that forced jink's client-side pagination shipped in 00158/00159 (done) — delivery now waits on the release, not this PRD |
| 00183 | Split oversized modules round 2 (list refreshed 2026-07-16: 14 files >800 prod lines) + `sql_conn` exception exit | Pure moves; sequenced after the Phase-2 refactors per the chosen numbering |
| 00184 | Declarative schema apply v2 + FFI zone parity | Feature; the FFI zone leg rides the 00177 FFI-contract work |

## Phase 3 — downstream enablement (P3, the investor demo)

| PRD | Scope | Depends | jink impact |
|---|---|---|---|
| 00185 | Publish a `ddb-client` Rust crate (MVP: transport, typed errors, envelope parsing, `wait_ready`; mock-tested; dev-dependency-only parity) | (00173 improves it) | Deletes `jink/src/ddb_client.rs` (~227 + ~541 test LOC) + call-site parse glue |
| 00186 | Offline SDL export (`ddb schema sdl`) + capability/version handshake on `/health` | — | Unblocks offline frontend codegen; replaces jink's live feature-probing |
| 00187 | Idempotent create: `ConflictAction::ReturnExisting` + `existed` flag (FFI leg on `create_structured`) | 00177 (soft: 00178) | Deletes jink's UNIQUE_VIOLATION-catch-then-refetch + create-then-read dances |
| 00188 | `ddb serve --ephemeral` test mode (tempdir + free port + printed connection block) | — | Deletes ~150-200 LOC of jink's `tests/common/mod.rs` harness scaffolding |
| 00189 | Rust type codegen from typedefs (`ddb schema codegen --lang rust`) | 00186 | Deletes jink's hand-mirrored typedef structs + selection consts |

## Phase 3b — purpose gaps (from the 2026-07-12 critique)

| PRD | Scope | Depends | jink impact |
|---|---|---|---|
| 00190 | Close the read-freshness hole: staleness probe, opt-in watch mode, sync-triggered schema reload, per-transport consistency contract page | 00169, post-00172/00174 actor shape | jink clients stop rendering stale data after out-of-band edits |
| 00191 | FFI git sync: mobile devices as full replicas (`set_remote`/`sync`/`fetch`/`push` + structured `SyncReport`) | 00163/00165/00167, 00177 | jink mobile becomes a first-class offline replica |
| 00192 | NoSQL HTTP full CRUD parity (thin app-contract adapters; scan filter/pagination parity; doubles as the contract's acceptance test) | 00172-00178, 00173 | Platform stops carrying a public interface that silently cannot write |

## Proof + appended work (2026-07-13 / 2026-07-16)

| PRD | Scope | Depends | Origin |
|---|---|---|---|
| 00193 | PgWire + FFI conformance legs + verb-disposition registry (parity forcing function) | 00192 | goal-gap G1 + assessment F7 |
| 00194 | FFI bindings CI (nightly Swift/Kotlin generation + platform artifact builds) | 00191 | goal-gap analysis |
| 00195 | Guard batch read-modify-write ops against lost updates (OID re-check at commit: fix_all/migrate/zone_migrate/rename) | 00162 | assessment F8 |
| 00196 | Route SQL DML through the unified core write pipeline (ends the fixed-N-times write-semantics bug class) | 00175-00178 (sequence with/after 00183's dml split) | assessment F4 |
| 00197 | Derive transport renders from core types (serde→GqlValue bridge; one `SqlResult` wire shape) | 00172, 00176 | assessment F9 |
| 00198 | Verify (fix or retract) the zone-default literal-NULL write defect; conformance pin; correct jink's gotcha | — | assessment F17 (UNVERIFIED) |
| 00199 | Prune dead abstractions + hygiene (dead traits, `GitRepo::open` migration gating + stray `.ddb-version`, fail-loud config, micro-dedupe, docs) | — | assessment F13/L4-L9 |

## jink deletion ledger (what upstream work removes downstream)

| jink code | ~LOC | Removed by |
|---|---|---|
| `src/ddb_client.rs` (hand-rolled HTTP client + parse glue) | 227 + 541 test | 00185 |
| `src/graphql/types.rs` mirror structs + selection consts | ~350 (consts moved to mutations/link.rs — sweep both) | 00189 (+ 00186) |
| fields-as-JSON-string double-encoding (`build_fields_json` remnant + inline `json!().to_string()` sites) | ~150 scattered | 00178 (adoption pass must sweep ALL sites) |
| Client-side sort/pagination fallback (frontend, narrowed to `created_at`/`updated_at`) | ~75 | **00158/00159 (done)** — delivery gated on the RELEASE, then jink adoption |
| Category / uncategorized id pre-resolution + two-step junction resolvers | ~230 | **00159 (done)** enabler — release-gated; 00176/00182 polish the rest |
| UNIQUE_VIOLATION catch-refetch + create-then-read dances | ~120 | 00187 |
| `tests/common/mod.rs` process harness (deletable scaffolding subset) | ~150-200 | 00188 |
| `server.rs` hand-written GraphQL router/parser (if client-blessed) | ~275 | 00185 (gateway helper) |
| `SafeSql`/`escape_sql` client-side escaping (1 prod call site left) | ~110 | (parameterized SQL — future PRD; even lower priority now) |

Rough total: ~2,400 backend + ~1,250 frontend LOC of generic glue/workarounds retired, against ~600 LOC of real jink domain logic. This is the measurable "we can deliver" signal.

**Cross-repo unblock** (corrected 2026-07-16): jink's own PRD 00026 (typed date sorts + relation filters + junction traversal) is gated on ddb PRDs **00158/00159/00160 — all DONE and unreleased**. Its probes run against the *deployed binary* (installed ddb self-updates from releases only), so the actual blocker is **cutting v0.2.7**, not any backlog PRD. 00182/00176 are hardening/routing follow-ups, not jink-00026 gates.

## Not scoped here (deliberately deferred)

- Parameterized SQL over transports (bind values for `executeSql`) — real, but jink already moved user values to GraphQL variables; author when a second downstream needs raw parameterized SQL.
- Incremental-indexing scale beyond ~5K doogats — see `design-decisions.md` "SQLite Index as Derived Cache"; revisit when a downstream crosses the threshold.
- Any ID-format change — direction pre-decided 2026-07-13 (identity correctness: UTC mint + node discriminator); 00170 records the exact shape and writes the complete implementation PRD to `dev/local/prds/hold/` for maintainer review. Still NOT in this batch — it enters a future batch only after `/review-prd-backlog` clears it.
- Full contract routing for sql passthrough / sync / discovery / batch verbs — after 00175-00178 the contract covers CRUD + search + schema-apply; whether the remaining verbs get AppCommand DTOs or are declared specialized surfaces is decided when invariant I7 flips to HOLDS. The maintenance-verb result drift found 2026-07-16 (FFI compact/import_bundle discard reports) rides this deferral — recorded in evolution-assessment-2026-07-16.md L13 so it isn't silent.
