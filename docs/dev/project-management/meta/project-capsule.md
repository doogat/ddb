# Project Capsule: Doogat DB

Generated: 2026-09-07 (00171 finalized with explicit maintainer exceptions at `876a153`; 16/16 tasks completed, two review cycles, batch intentionally stopped). Older architecture and other-PRD entries retain their original verification scope.

**Capsule pruned this session.** Prior version had grown to ~56K tokens via a chained "Prior header follows" history reaching back to 2026-05 plus 12 duplicate `## GitHub State` blocks and 3 duplicate `## Project Health` blocks. That history is not lost — every completed PRD's story lives in `dev/local/prds/done/`, its review trail in `dev/local/reviews/<prd>-audit.md`, and the cross-PRD narrative in the `project_*` memory files (see § Project Memories). This capsule now carries only current state; update it going forward instead of appending another header chain.

## Key Invariants

1. Git is the sole source of truth - SQLite index is always rebuildable from Git
2. Doogats stored at `ddb/{id}.md`, typedefs at `ddb/_typedef/{id}.md`
3. DoogatId is a 14-digit timestamp string (`YYYYMMDDHHmmss`)
4. Three-zone Markdown: frontmatter (YAML) / body (Markdown) / references
5. Conflict resolution: Git merges first, falls back to Automerge CRDT per-zone
6. `head_oid()` returns `CommitHash` (String newtype, access inner via `.0`), not `git2::Oid`
7. All modules return `error::Result<T>`
8. Data dir: `.ddb/`, node file: `.git/ddb-node`, git signature: `ddb`
9. Folder-typed doogats live in `ddb/{type_name}/{id}.md`
10. `merge_frontmatter` called from both `resolve_conflicts` and `resolve_append_log`
11. `Value` enum holds frontmatter extra fields - supports nested `Map`, `List` with dot/bracket path navigation
12. DoogatService is the sole entry point for all core operations - CLI, FFI, server actor, and ReadPool all delegate to it. No public repo()/index() getters.
13. Tags are stored in frontmatter `tags:` field, indexed in `_ddb_tags` internal table, separate from type columns
14. REFERENCES columns in typedefs create both singular (first match) and plural (all matches) fields in GraphQL typed objects
15. `typed_doogat_to_value()` extracts typed fields from ParsedDoogat using column zone info; currently returns String IDs for REFERENCES
16. BOOLEAN columns stored as INTEGER (1/0) in SQLite, coerced to true/false in GraphQL typed queries via `extract_typed_field`, but raw SQL queries return "1"/"0"
17. GraphQL type names use `capitalize()` on `table_name` (first char uppercased). Query fields use `pluralize()` on `table_name`. Both assume valid GraphQL identifier input - hyphens rejected by `is_valid_graphql_name()`.
18. `createDoogat` actor must pass typed fields via `BatchCreateInput.fields` for upsert pre-check to work - empty fields map means the pre-check is a no-op
19. SQL INSERT into typed tables is atomic: `build_and_index_row` wraps `index.index_doogat()` + `insert_materialized_row()` in a SQLite `SAVEPOINT insert_row`. On failure, the savepoint is rolled back so no ghost `doogats` index row is left behind. Fix for #4, see dml.rs:138-194.
20. `executeSql` write path enforces NOT NULL, VARCHAR(N)/CHAR(N) length, INTEGER/REAL/BOOLEAN type, and unknown-column checks via `validate_row_against_schema` (ddb-core/src/sql_engine/dml.rs) on both INSERT and UPDATE. Validation runs before any side effects (no ghost rows possible). Multi-row INSERT pre-validates all rows in pass 1 before any writes in pass 2; `InsertNextCounters` (bare + partitioned) tracks `DEFAULT NEXT` counters in-memory across pass 1. Expression-synthesized NULL detected via `eval_values_nullable` and rejected on NOT NULL columns. The implicit `title` column is retained in `schema.columns` (only `id` and `type` stripped); downstream code skips core columns via `is_core_column` or `BASE_DOOGAT_FIELDS`. Issue #7 / PRD 00122.
21. `UPDATE`/`DELETE WHERE id = X` fast path guarded by `SqlEngine::row_exists_in_table(table_name, id)` against the target materialized type table (not global `doogats` index). Missing/cross-table id returns `Affected(0)`. Fix for #5.
22. `search()` accepts in-query field filter syntax (`tag=X`, `category=Y.Z`, `field:value`) via `compile_search_plan` in `ddb-core/src/search_query.rs`. Tag negation works; non-tag negation returns `BadRequest`. Empty query with no filters returns `BadRequest`. `normalizeSearchQuery` validates via the same rules. PRD 00121.
23. SQL `JOIN`, CTE (`WITH`), subquery-in-FROM, `UNION`, and window functions (`OVER`) all work via sqlparser + rusqlite pass-through. PRD 00123 archived to `deferred/`.
24. PRD 00128 ALTER COLUMN TYPE: string-literal-aware preprocessor for `SET DATA TYPE`/`TYPE` shorthand; CHAR vs VARCHAR family enforced via `TypeKind`; widening paths VARCHAR→TEXT, CHAR→TEXT, INTEGER↔REAL. REFERENCES widening covered.
25. PRD 00129 typed writes: structured error code vocabulary (`UNIQUE_VIOLATION`, `REFERENCES_VIOLATION`, `NOT_NULL_VIOLATION`, `UNKNOWN_FIELD`, `TYPE_NOT_REGISTERED`, `CASCADE_CYCLE`); typed `createDoogat` populates type-specific tables; ON DELETE CASCADE walks with cycle detection; CREATE INDEX IF NOT EXISTS as no-op; typed nested accessor on `Doogat`; `tags` filter on per-type Where inputs.
26. PRD 00133 unified typed-write path: every typed-create entry point (CLI/FFI `create_doogat_with_extra`, GraphQL `createDoogat`/`createMany`, SQL `INSERT`) routes through `sql_engine::typed_insert::prepare_typed_insert_validate`. REFERENCES values land in the *reference zone* (not frontmatter); FK validation queries the *referenced typedef table* (not the generic `doogats` index); `allowed_values` (ENUM) enforced uniformly. CLI/FFI strict pipeline only fires for *registered* typedefs — unregistered types still keep the silent-base-only behavior preserved by PRD 00129 §T3. Counter seeding now lazy-only: `precompute_next_counters` removed (commit 0786765); helper's `seed_bare_next` covers all cases.
27. **Search field-filter LIKE asymmetry (PRD 00133, #15 follow-up)**: in-query syntax `search(query: "title=X")` / `search(query: "<col>=X")` does **substring (LIKE)** match against the title (or referenced typedef's title for REFERENCES columns). `tag=` keeps **exact** match. The explicit `where: { field, eq: X }` API filter is **always exact**. Asymmetry is the contract: in-query is the user-facing convenience form. Search now traverses *user-defined membership typedefs* (e.g. `category-membership` shape with `link_id` + `category_id` REFERENCES columns) and direct `<field>_id` columns, in addition to auto-junction `{type}_{field}` and direct `<field>` columns. Detection is column-shape based, not name-based.
28. **`ALTER TABLE foo SET SEARCH KEY <col>` (commit f1788b2, #15 follow-up)**: tells the search filter resolver to substring-match `field=val` queries against `<col>` on the referenced typedef instead of the default `title`. Round-trips via typedef YAML (`search_key: <col>` frontmatter) and mirrors into `_ddb_meta` so the change takes effect immediately, no rebuild. `DROP SEARCH KEY` resets to default.
29. **`incremental_reindex` materializes typed data (commit f1788b2, #15 follow-up)**: the path that runs after `git pull`, direct file edits, or any change that lands without going through SQL `INSERT`/typed-create entry points now also writes typed data doogats into their materialized type tables and evicts deleted ones. Failures on a single type's schema load are logged and skipped — incremental indexing stays best-effort.
30. **`ALTER TABLE foo RENAME TO bar` (PRD 00132)**: one git commit covers typedef name change, data folder rename (folder-typed) or `type:` frontmatter rewrite (flat-layout), auto-rewrite of every typedef whose `columns:` lists `references: foo`, and path-based wikilinks pointing into `ddb/foo/`. Materialized SQLite table renamed via native `ALTER TABLE` after the commit lands. `materialize_all_types` drops orphan tables on rebuild so a kill between commit and SQLite rename auto-recovers.

## Architecture Decisions

- Consistency module: `fix_all` (detect+apply auto-fixes) vs `migrate_all` (versioned data migrations with `.ddb/migration-version` tracking)
- Fix severity ordering: Error > Warning > Info - applied in that order
- Migration system extensible via `built_in_migrations()` - currently v1-v3
- Search chain: indexer::search_hits → service::search_paginated → read_pool::search → schema search field resolver
- FTS5 uses JOIN on doogats table for metadata; tags in `_ddb_tags`, fields in `_ddb_fields`
- Dynamic GQL schema: `build_schema()` takes `type_schemas: Vec<TableSchema>`, builds per-type Object/Where/OrderBy/Connection/Aggregate types; rebuilds via `SchemaReloader` on typedef changes
- ReadPool uses `spawn_blocking` with per-request DoogatService instances for concurrent reads
- SQL responses: SqlEngine → SqlResult → sql_result_to_value() (GQL) or DataRowEncoder (pgwire); both pass raw SQLite strings without type coercion
- Type name to GraphQL name: `is_valid_graphql_name` gate at 3 points in mod.rs; `capitalize()` only uppercases first char
- Upsert: `unique_together` in typedef creates SQLite unique indexes at materialization; `onConflict: IGNORE` does pre-check query (not SQL ON CONFLICT)
- Single typed-insert helper (`prepare_typed_insert_validate` in `ddb-core/src/sql_engine/typed_insert.rs`) is the chokepoint for default resolution + zone routing + per-column validation across CLI, FFI, GraphQL, and SQL surfaces. Adding new entry points should route through it; new validation rules should land there once.

## Component Boundaries

- `consistency::fix_all` takes `&impl DoogatStore` + `&Index`, loads typedefs via `index.load_all_typedefs(repo)`
- `consistency::migrate_all` takes `&impl DoogatStore` only (no index dependency)
- Service layer is thin wrapper over `fix_all()` / `migrate_all()`
- ReadPool wraps service methods for async access via `with_service`/`with_service_mut`
- Schema module (`ddb-server/src/schema/`) split into `mod.rs` and `base_types.rs`
- `table_name` from `TableSchema` is the raw type name (e.g. `"category-membership"`); `type_name` (PascalCase) and `plural` are derived in-place
- `sql_engine::typed_insert` owns typed-write zone routing; `service::crud::batch_create` and `service::crud::create_doogat_with_extra` both call it. `service::validation` no longer pre-seeds NEXT counters (helper does it lazily).

## Active Work

### Batch 202607161128 (autopilot, STOPPED by maintainer after 00171; backlog remains)

- [x] **00163 git-tree-construction-correctness-v1** (DONE 2026-07-17, in `done/`) — delete-freshness + merge-lock legs of the git write-path data-loss class.
- [x] **00200 make-merge-tree-three-way-v1** (DONE 2026-07-18, in `done/`) — true three-way conflicted-merge tree (Alternative B), closing the last data-loss leg.
- [x] **00164 unify-id-minting-v1** (DONE 2026-07-18, in `done/`) — one repo-aware existence oracle for all mint paths.
- [x] **00165 deterministic-crdt-resolution-v1** (DONE 2026-07-18, in `done/`) — content-derived Automerge actor IDs, deterministic list-field ordering.
- [x] **00166 make-hlc-load-bearing-v1** (DONE 2026-07-19, in `done/`) — machine-local `HlcClock`, HLC trailer on every write, unified `lww_pick` tie-break.
- [x] **00167 sync-collision-reassignment-atomicity-v1** (DONE 2026-08-15, in `done/`) — folded winner + loser + backlink rewrites into one atomic `commit_merge`, loser ID derived deterministically via `(old_id, losing_blob_oid)` through the 00164 oracle. 2 review cycles (cap 2), converged with deferrals: cycle 1 closed 4 CRITICAL + rework; cycle 2 found 0 CRITICAL / 5 HIGH — 2 excluded (settled-deferral re-raise, pre-existing non-divergence), 3 appended as cap-overflow deferrals (malformed-loser sync wedge — deliberate cost of the atomic-abort ruling; `partition_conflicts` missing `ddb/` prefix guard, pre-existing; weak determinism pin in the independent-resolution test). 18-item medium/low tail deferred without a sweep (cap-out branch). All doubt rubric verdicts (D1-D5) pass from both codex and fable. Tier 1 green (1710 tests); `cargo test -p ddb-e2e` 1 unrelated transient flake, isolated rerun passed.

- [!] **00168 bundle-import-conflict-recovery-v1** (STALLED cycle 2, moved to `hold/`) — closes the last unlocked write path in the git write-path data-loss class (memory `project_data_loss_write_path`). Tasks 1-8 all landed (impl + tests + e2e + docs/CHANGELOG/walkthrough), but review cycle 2 found 1 CRITICAL + 3 HIGH (all new, all sharing one root cause: the cycle-1 fix that kept `refs/remotes/bundle/*` across failures turned a transient ref namespace into durable state without redesigning the fetch refspec/merge target/cleanup for it) and hit `rework_cap` 2 with the CRITICAL unresolved, so `autopilot stall` parked it rather than reworking further. Full root-cause detail and repro paths are in the batch deferred JSON (op_id `c389f89aed18`) and `dev/local/reviews/00168-bundle-import-conflict-recovery-v1-review-02.md`. To resume: `mv dev/local/prds/hold/00168-bundle-import-conflict-recovery-v1.md dev/local/prds/backlog/` and re-run autopilot (reuses its existing design doc). Verified at HEAD `6bb1617`: clippy clean, `cargo test -p ddb-core --lib --skip sql_engine` 1102/1102, `cargo test -p ddb-e2e` 449/449; master NOT pushed (00168 never reached its PRD-end push).
- [x] **00169 poison-file-reindex-resilience-v1** (DONE 2026-08-16, in `done/`) — lenient incremental indexing (collect-and-skip per-file failures instead of aborting the whole batch), `--strict` opt-in, skip warnings surfaced on create/update/apply_schema + FFI `RebuildReport.warnings`, plus an independent availability fix: a cross-process rebuild lock (`locked_rebuild`/`locked_explicit_rebuild`) serializing the destructive full-rebuild path so concurrent cold-repo `ddb create`s and `ddb reindex` stop racing on FTS bootstrap, and a `PRAGMA user_version` schema-version stamp. 2 review cycles (cap 2), converged with deferrals: cycle 1 closed with 3 settled-deferral HIGHs (skip warnings never reach read verbs incl. search — owned by backlog 00176; frontmatter cap bounds parse not read cost — needs a streaming reader; ReadPool bypasses the rebuild lock — accepted residual, invariants.md I10); cycle 2 found 0 CRITICAL, 1 HIGH (poison eviction keys off file stem not indexed identity, pre-existing class, wants its own follow-up PRD) plus a 10-item medium/low tail deferred without a sweep (cap-out branch). Tier 1 green throughout.

- [x] **00170 id-throughput-ceiling-decision-v1** (DONE 2026-08-17, in `done/`) — decision/spike PRD (`design: skip`), no runtime code. Recorded the ID-format decision in `design-decisions.md` (UTC mint + 32-bit node-discriminator suffix, framed as collision-resistant not collision-free; ID-mint throughput split from the ~1/sec/process end-to-end doogat-create bound) and emitted the follow-up implementation PRD `00201-id-format-utc-node-suffix-migration-v1.md` into `dev/local/prds/hold/` for maintainer review via `/review-prd-backlog`. 2 review cycles (cap 2), converged with deferrals: cycle 1 closed 1 CRITICAL + 4 HIGH + 3 MEDIUM (absolute collision-freedom overclaim, throughput conflated with doogat-create latency, `(guess)` sub-second width, unvalidated node-file, layering contradiction, unverified jink claim, e2e-locally-vs-CI mismatch, missing CHANGELOG-skip disclosure); cycle 2 found 0 CRITICAL, 2 HIGH + 1 MEDIUM regression findings against the cycle-1 rework itself (malformed-node hash fallback diverges from `HlcClock`'s raw `truncate_node` rendering; promised structured warning has no propagation channel; hash fallback pins no algorithm) — all deferred at rework cap since they are gaps in the held 00201 plan doc that autopilot never drains. Alice/Carl clean, Blake's 3 process findings dismissed against the ledger, Bob's D1-D5 all pass.

- [x] **00171 migrate-shell-scenarios-to-e2e-v1** (DONE 2026-09-07, in `done/`; 2 cycles) — 16/16 tasks and all 16 attempts preserved; 391 mapping rows DONE; four shell scripts retired. The maintainer accepted all 11 remaining cycle-2 findings and waived the remaining tail sweep. These findings remain deferred, not repaired. All required review lenses and the roster/constraint gate passed, including D1-D5 in both lanes. Acceptance: `dev/local/reviews/00171-acceptance-2026-09-07.md`.
  - Test 34086467208 and Full Validation 34092612666 passed at `876a153`; each platform ran 566 passing e2e tests, 0 failures, 2 unrelated hazard skips. Local cycle-2 fast tests: 1819/0/0. Older build/Clippy evidence retains its original SHA. The historical macOS reset remains unreproduced and deferred under 00188.
  - Batch: 9 completed PRDs; 16 deferrals for 00171 (5 earlier + 11 accepted), 47 total. Audit/report reconciled. State resets to `build/build` for a future manual resume only; no next PRD selected or wrapper launched.

- [x] **00172 collapse-actor-plumbing-v1** (DONE 2026-10-17, in `done/`; cycle 1, tail-swept) — collapsed the ActorCommand/ActorReply twin-enum dispatch into a single generic `ActorHandle::call<R>` entrypoint, with `EventIntent`/`IntentEmit` for event emission and `ReloadIntent`/`ReloadSignal` collapsing the 5 hand-wired per-adapter schema-reload triggers into one derivation site. `handle_command` and `actor/handlers.rs` deleted entirely. Cycle 1 converged (0 CRITICAL, 2 HIGH both settled deferrals with design-doc evidence; doubt D1-D5 all pass); 14 medium/low findings swept via 4 tail-sweep tasks (ClosureMsg dead field removed, actor/tests.rs split under 800 lines, stale docs/comments fixed, PgWire/GraphQL reload-parity test gap closed, dead REST reloader Extension layer dropped). Tier 1 green (1913 tests) at `1cd221c`+2 chore commits.
- [x] **00173 unify-server-error-mapping-paths-v1** (DONE 2026-10-17, in `done/`; 2 cycles, 14/14 tasks) — single error-rendering policy in `app_contract::error`, retiring the 4-way mapper fan-out (GraphQL/REST/NoSQL/PgWire) and the FFI allowlist's lossy collapse. Cycle 2 tail-swept (tasks 12-14). Tier 1 green at `b4e2359` (build, clippy@1.89.0, test-ci 1945 passed, e2e 574 passed, mdbook build); no CRITICAL/HIGH outstanding, no deferrals.

### In progress

None.

### Remaining backlog (29 PRDs; not selected)
- [ ] 00174-decouple-maintenance-from-write-actor-v1
- [ ] 00175-route-read-delete-through-contract-v1
- [ ] 00176-route-search-through-contract-v1
- [ ] 00177-ffi-crud-through-contract-v1
- [ ] 00178-structured-fields-input-v1
- [ ] 00179-sql-identifier-escaping-materializer-v1
- [ ] 00180-server-security-resource-hardening-v1
- [ ] 00181-index-health-quarantine-surface-v1
- [ ] 00182-where-filter-correctness-completeness-v2
- [ ] 00183-split-oversized-modules-round-2-v1
- [ ] 00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1
- [ ] 00185-ddb-client-rust-crate-v1
- [ ] 00186-offline-sdl-export-and-capability-handshake-v1
- [ ] 00187-idempotent-upsert-return-existing-v1
- [ ] 00188-ephemeral-serve-test-mode-v1
- [ ] 00189-rust-typedef-codegen-v1
- [ ] 00190-close-read-freshness-hole-v1
- [ ] 00191-ffi-git-sync-mobile-replica-v1
- [ ] 00192-nosql-http-full-crud-parity-v1
- [ ] 00193-pgwire-ffi-conformance-legs-v1
- [ ] 00194-ffi-bindings-ci-v1
- [ ] 00195-guard-batch-rmw-lost-updates-v1
- [ ] 00196-route-sql-dml-through-write-pipeline-v1
- [ ] 00197-derive-transport-renders-from-core-types-v1
- [ ] 00198-verify-zone-default-null-write-v1
- [ ] 00199-prune-dead-abstractions-and-hygiene-v1
- [ ] 00203-bundle-import-ref-namespace-v1
- [ ] 00204-sync-collision-loser-without-frontmatter-v1
- [ ] 00205-index-eviction-by-indexed-identity-v1

**Important for every PRD ≥ 00172 (memory `project_ddb_growth_roadmap`): the shell harness dies at 00171 — never author `.sh`/`.ps1` scenarios in a PRD numbered above it; scenario tasks target the Rust e2e harness directly.**

`hold/`: 00168 (stalled, see historical detail above), 00169b (superseded by 00169), 00201 (ID-format plan held for maintainer review). Autopilot never reads `hold/`.

Numeric order = autopilot execution order (memory `project_ddb_growth_roadmap`); 00200 already shipped as the one deliberate out-of-order exception.

## Related context

2026-09-07: direct review/state artifacts establish the current 00171 context above. Catchup's task-restore helper returned `no_index`; no tasks were restored or rewritten. The optional engram query failed because the sandbox denied access to its embedding server at localhost:8083; harvest/index/portfolio retrieval were not continued. The older retrieval results below are historical and are not current evidence.

Engram re-queried 2026-08-16 (fifth run, topic `master Migrate smoke/integration shell scenarios to the Rust e2e crate`): only self-hits, 00171's own PRD file (repo and portfolio scope return the identical five rows, all within the PRD itself or adjacent Phase-2-titled sections of other backlog PRDs); no external prior art. Staleness stamp (`engram status --scope portfolio`, 2026-08-16, nonzero exit): memory stale=14, prd stale=1, transcripts stale=2 dead=5640, code stale=4 (skipped=3598), discovery/findings clean; many sibling repos report absent prd/decision/discovery layers. Review harvest: `dev/local/reviews/*-review-NN.md` files upserted (30 total, incl. 00170's two cycles); the `*-audit.md` renders still fail to parse (`missing frontmatter`) and are NOT harvested — the audit trail survives only as files, so it dies with a `dev/local` GC.

## GitHub State (refreshed 2026-10-17)

- 0 open issues, 0 open PRs, no stale remote branches. Latest release v0.2.7 (2026-07-17); 215 unreleased commits on master now.
- **Nightly RED 5 straight days (2026-10-12 through 2026-10-16), all pinned to commit `d7867d23`** (= `d7867d2 fix(sync): fold frontmatter-less collision losers byte-unchanged`, the most recent master commit before this session). `full-validation / windows-full` fails: `sync_manager::tests::sync_report_lists_every_verbatim_folded_collision_loser` panics with a byte-array payload `[65, 32, 98, 111, 100, 121, ...]` = `"A body, no frontmatter\r\n"` — a Windows CRLF vs. Unix LF mismatch in that commit's new byte-unchanged-fold assertion. Linux results not shown in the excerpt; likely Windows-only (CRLF). Not yet triaged into a PRD. This commit predates PRD 00172 and is unrelated to actor-plumbing collapse work — flag as a separate follow-up, do not fix inside 00172's scope.

### Prior snapshot (2026-09-07)

- Local master and `origin/master` are both `876a153`; GitHub has a push run on that SHA. Earlier statements about 119 unpushed commits and a blocked push are obsolete. Only the main worktree exists; an older `worktree-agent-a188c62e61e58f6e5` branch was not removed because ownership was not established.
- Test 34086467208 at `876a153` succeeded. Manual Full Validation 34092612666 at the same SHA succeeded on all six jobs (2026-09-07 06:50–07:45 UTC). Actual e2e summaries: 566 passed, 0 failed, 2 unrelated hazard tests ignored on Linux, macOS and Windows. Earlier SHA-specific results retain their original scope.
- Nightly 34020494801 failed at `c8b72bd`; both named failures passed in the later complete suites. The causes remain unconfirmed; current green execution is not a claimed concurrency fix. The maintainer accepted deferring the historical Mac reset under 00188; it remains unreproduced and unfixed.
- Issues, PRs, and releases were not refreshed in this focused pass. The catchup GitHub helper reported authentication failure inside the sandbox; a direct escalated GitHub read succeeded, so no credential failure is established.
- Finalize check 2026-09-07: clean tracked worktree; local HEAD and `origin/master` both `876a153b4039128f8052526629773fa189058c71`. Successful CI on this exact SHA establishes publication. The redundant final push hit sandbox DNS failure and its escalation was interrupted; a fresh live remote recheck is unavailable. No new commits need publication. Preserve `FT-6`, `FT-3`, the old unrelated branch, and hazard commits.

### Historical snapshot (2026-08-17; superseded for current status)

- **Open issues**: 0. **Open PRs**: 0. **Branches**: master only. **origin/master still stuck at `d77ad58`, now 119 commits behind local HEAD** (up from 118 — task 7's self-deslop commit `34138fe` landed on top). Push retried this session and failed again (see below). Working tree clean.
- **Push blocked (reconfirmed 2026-08-17 this session)**: `git push origin master` → `sign_and_send_pubkey` fails for both RSA `buvis` and ED25519 `1Password` identities → `Permission denied (publickey)`, exit 128 — locked/unreachable SSH signing agent on this unattended host. Same recurring condition as after 00167, 00170, and last catchup. Logged to `state.deferred_decisions` (`site: catchup_push_attempt`). **User action needed: unlock the signing agent and `git push origin master` manually** — overdue past this repo's push-per-PRD convention (8 completed PRDs since the last push).
- **Latest release**: v0.2.7 (2026-07-17). Local HEAD now 119 ahead of origin (up from 118). Per standing release-cadence policy (jink adoption is release-gated), a v0.2.8 cut is due once pushed.
- **CI — Nightly RED, still pinned to the same commit `d77ad58`, real failures (not the guard no-op; runs take minutes and `full-validation` reports `failure`).** Failing test: `two_node_singleton_post_sync_resolution` (`ddb-core/tests/sync_test.rs:310`) — resolver picks node A (`"20260601000000"`) over the HLC-later node B write (`"20260601000010"`). Cross-platform, regression from the 00164–00166 resolver range, slipped through the Tier-1 gap (`cargo test-ci` doesn't run `--test sync_test`). Not yet triaged into its own PRD; recorded in the batch deferred JSON (`topic: nightly-red-singleton-hlc-winner`). **5 straight red days now 6 (2026-08-12 through 2026-08-17)** — none of the 119 local commits has ever been Tier-2 validated.
- **Batch 202607161128**: 8 PRDs done (00163, 00200, 00164, 00165, 00166, 00167, 00169, 00170). PRD 00168 STALLED cycle 2, moved to `hold/` (unresolved CRITICAL). PRD 00171 in `wip/`, build gate Phase 3 work resuming at task 8/14.

## Project Health

2026-09-07: 00171 is finalized by explicit maintainer acceptance with 11 findings deferred and the tail sweep waived. Both final-SHA CI workflows passed; the historical macOS reset remains a follow-up under 00188. Batch 202607161128 is intentionally stopped with 31 backlog PRDs remaining. Historical concerns below need their own current assessment and must not expand this closure scope.

### Historical assessment (2026-08-17)

Code is in good shape — 6 P0/P1 PRDs landed cleanly closing the git write-path data-loss class for every locked write path (invariants I1/I2/I4 HOLD) plus poison-file reindex resilience (00169) and the ID-throughput ceiling decision (00170, docs-only). 00168 (bundle import, the last unlocked path) implemented all 8 tasks but stalled in review with an unresolved CRITICAL — the fix is scoped and recorded, just not yet applied. Live concerns:

1. **00168 parked with a known CRITICAL** — importing a different bundle while a stale `refs/remotes/bundle/master` sits kept from a prior failed import silently merges the OLD bundle's data and reports success. Fix direction is recorded (per-import ref namespacing, or track-and-merge-only-the-delivered-ref); do NOT "fix" it by forcing the refspec (reintroduces the cycle-1 CRITICAL it was trying to avoid).
2. **Nightly red 5+ straight days on a cross-platform singleton-conflict HLC bug** (see GitHub State) — regression from the 00164–00166 range. Not yet triaged into a PRD; recorded in batch deferred items.
3. **v0.2.8 overdue and local master unpushed, now 119 commits ahead of origin** (git push blocked by a locked SSH signing agent — reconfirmed blocked again this session), 55+ unreleased commits since v0.2.7; per `project_nightly_green_is_not_validation` this means zero Tier-2 CI coverage of the last 8 PRDs until a human unlocks the agent and pushes. This is the single highest-leverage blocked item: it gates the release, the Nightly signal, and the fix for concern 2 all at once. Purely an environment/credential gap, not a code defect — no PRD can fix it.

## Project Memories

See the auto-memory system (`~/.claude/projects/-Users-bob-git-src-github-com-doogat-ddb/memory/MEMORY.md`) for durable cross-session facts: PRD numbering convention, the (closed) data-loss write-path class, the two-error-mapping-paths gap, the growth roadmap, nightly-green-is-not-validation trap, rustfmt drift, and the evolution-assessment skills. Do not duplicate their content here — link to them by name if a session surfaces something that updates one.
