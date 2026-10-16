# PRD 00157 — Phase 0 write-path × SINGLETON-wrap audit

Authoritative protection matrix for every `pub`/`pub(crate)` write method on
`DoogatService` plus the SQL DML entry points and the consistency fixers.
Produced 2026-06-16 against current master (HEAD ff0fd31, post-00156 split,
post-00149 facades). This is the Phase-1 task list: any row marked **GAP**
becomes a Phase-1 wrap task; every other row carries a one-line justification.

PRD acceptance (Phase 0): "matrix lists every `pub`/`pub(crate)` write method on
`DoogatService` + SQL DML entry points; each row has wrapped/justified/
task-created status." Exit criterion: **no write path with unknown status.**

## Threat model (what the audit is proving)

The protected invariant is *at most one materialized row per SINGLETON typedef,
enforced across concurrent processes*. A path is a **concurrent-writer SINGLETON
creator** only if it can add or move a *materialized* row into a SINGLETON type
table from a request-path writer that may race another process. The
check→commit→reindex→store_head sequence of such a path must run inside one
`with_immediate_transaction` window (the 00140 create-path discipline) so a
cross-process loser serializes on the SQLite write lock and sees the winner's
committed row (→ structured `SINGLETON_VIOLATION`, not a duplicate row or a raw
`UNIQUE constraint failed`).

Three things are **not** the same as a gap:
- **Layer-3 backstop.** The materializer's `<table>_singleton_lock` UNIQUE
  expr-index (PRD 00139) rejects a second materialized row at SQLite regardless
  of caller. It guarantees *correctness* (no duplicate row) but yields a raw
  error, not the structured message — so it is a safety net, not a substitute
  for the wrap on request-path writers.
- **Single-process maintenance.** `fix_all`/`migrate_all`/reindex are not
  concurrent request-path writers; they run as one maintenance operation. The
  Layer-3 backstop covers their (pre-existing-data) edge cases.
- **A different enforcement layer.** SQL `INSERT` cross-process safety is the
  dml.rs Layer-2 pre-check + Layer-3 lock-translation, not the service
  `BEGIN IMMEDIATE` window. Its determinism is the Phase 2 §55.D concern.

## Service write methods (`ddb-core/src/service/`)

Reuse pattern reference: `with_immediate_transaction` (indexer/mod.rs:645,
nesting-tolerant) + `check_singleton_constraint` (create) /
`check_singleton_update_constraint` (update), both validation.rs.

| Method | File:line | SINGLETON-reachable creator? | Wrapped? | Status |
|---|---|---|---|---|
| `create_doogat` | create.rs:30 | via `create_doogat_with_extra` | inherits ✓ | **OK** — thin wrapper → wrapped terminal |
| `create_doogat_parsed` | create.rs:42 | via `create_doogat_with_extra` | inherits ✓ | **OK** — thin wrapper → wrapped terminal |
| `create` (facade) | create.rs:56 | via `batch_create_with_message` (L71) | inherits ✓ | **OK** — facade routes through wrapped batch |
| `create_doogat_with_extra` | create.rs:176 | yes | ✓ wrap L488, check L447 | **OK** (00140) |
| `upsert_singleton` | create.rs:285 | yes | ✓ wrap L309 (check inside) | **OK** (00140) |
| `create_doogat_raw` | create.rs:409 | yes | ✓ wrap L488/check L447 region | **OK** (00140) |
| `update_doogat` | update.rs:13 | via `update_doogat_parsed` | inherits ✗ | **GAP 1a** (inherited) |
| `update_doogat_parsed` | update.rs:27 | **yes** (retype-into-singleton) | **✗ no wrap, no check** | **GAP 1a → Phase 1 task** |
| `update` (facade) | update.rs:86 | via `update_doogat_parsed` (L91) | inherits ✗ | **GAP 1a** (inherited) |
| `batch_update` | update.rs:110 | via `batch_update_with_message` | inherits ✗ | **GAP 1b** (inherited) |
| `batch_update_with_message` | update.rs:115 | **yes** | **✗ check L192 present, no wrap** | **GAP 1b → Phase 1 task** |
| `update_doogat_raw` | update.rs:246 | yes | ✓ wrap L283, check L267 | **OK** (00140) — reference pattern |
| `batch_create` | batch.rs:116 | via `batch_create_with_message` | inherits ✓ | **OK** — thin wrapper |
| `batch_create_with_message` | batch.rs:121 | yes | ✓ wrap L160, check L195 | **OK** (00140) |
| `delete_doogat` | delete.rs:19 | no | n/a | **JUSTIFIED** — removes a row, never adds/moves one into a singleton table |
| `execute_sql` | sql.rs:11 | via SQL `INSERT` → dml.rs | see dml row | **JUSTIFIED at service layer** — INSERT safety is dml Layer-2/3; §55.D = Phase 2 |
| `execute_batch` | sql.rs:26 | via SQL `INSERT` → dml.rs | see dml row | **JUSTIFIED at service layer** — same as `execute_sql` |
| `begin_transaction` / `commit_transaction` / `rollback_transaction` | sql.rs:41/51/64 | no (txn control) | n/a | **JUSTIFIED** — open/commit/rollback only; row writes happen via `execute_sql` inside, whose dml SAVEPOINTs nest in the open txn |
| `commit_file` | utility.rs:29 | no (git primitive, no materialize) | n/a | **JUSTIFIED** — writes a file to git but does not index/materialize; a later reindex applies, guarded by Layer-3 |
| `install_bundled_type` | discovery.rs:81 | no (installs a `_typedef`, not a data row; bundled types are non-singleton) | n/a | **JUSTIFIED** — typedef install (DDL-like) |
| `migrate_all` | utility.rs:78 | see consistency row | n/a | **JUSTIFIED** — single-process maintenance; Layer-3 backstop |
| `compact` / `run_maintenance` / `set_auto_maintenance` / `register_node` / `retire_node` / `export_*` | ops.rs:15/81/62/85/94/101/106 | no (gc / node-config / bundle export; no typed-row write) | n/a | **JUSTIFIED** — operational, no singleton data-row creation |
| `nosql_rebuild` | utility.rs:171 | no (rebuilds mirror from git) | n/a | **JUSTIFIED** — read-of-git → mirror, creates no doogats |

All remaining service methods (`utility.rs` `list_*`/`resolve_path`/`head_oid`/
`nosql_get`/`nosql_scan_*`/`nosql_backlinks`/`infer_schema`/`load_config`/
`health_check`/`is_index_stale`, `discovery.rs` readers, `read`/`search`) are
**reads** — not write paths, excluded from the matrix.

## SQL DML entry points (`ddb-core/src/sql_engine/dml.rs`)

| Entry | Line | SINGLETON-reachable creator? | Protection | Status |
|---|---|---|---|---|
| `handle_insert` | dml.rs:125 | **yes** | per-row `SAVEPOINT insert_row` (L348) + Layer-2 pre-check (L157) + Layer-3 `<table>_singleton_lock` translation (`is_singleton_lock_failure`, L1527-1568) | **JUSTIFIED (different layer)** — cross-process determinism is the §55.D Phase-2 task, not a service `BEGIN IMMEDIATE` gap |
| `handle_update` | dml.rs:430 | **no** | `SAVEPOINT update_row` (L1003) | **JUSTIFIED** — `apply_updates_to_doogat` (builders.rs:760-809) writes only Reference/Frontmatter/Body zones + implicit `title`; it **never mutates `doogat_type`**, and a SQL UPDATE is scoped to one materialized table, so it cannot add or move a row into a singleton table |
| `handle_delete` | dml.rs:648 | no | — | **JUSTIFIED** — removes rows only |
| `handle_create_table` (DDL) | dml/ddl.rs | no (typedef install; `SINGLETON DEFAULT VALUES` auto-seed is the *first* row in a fresh table) | — | **JUSTIFIED** — one-time DDL install, not a concurrent data-row writer |

## Consistency fixers (`ddb-core/src/consistency/`)

| Path | File | Can mutate `doogat_type`? | Status |
|---|---|---|---|
| `fix_all` → `apply_fixes` | mod.rs:547 / 450 | **yes** — `Fix::DefaultSet{field:"type"}` (L470, only when type is currently `None`) and `Fix::TypeNormalized` (L521, canonicalises an existing type string) | **JUSTIFIED (out of concurrent-writer scope)** — single-process maintenance op, not a request-path writer; a retype into a singleton table is caught by the Layer-3 UNIQUE backstop. Wrapping it is out of PRD 00157 scope (no concurrent-writer threat model). Recorded as a known caveat, not a silent "cannot reach". |
| `migrate_all` → `built_in_migrations` | migrations.rs:87 / 15 | **yes** — a versioned migration renames a type (L66) | **JUSTIFIED** — same reasoning as `fix_all`; single-process, Layer-3 backstop |
| `singleton_sweep` | singleton_sweep.rs:26 | reads `doogat_type` (L50); quarantines losers | **JUSTIFIED** — convergence-only: reduces an already-conflicted singleton table to one row (post-sync), never creates a second |
| `zone_migrate_all` | zone_migrate.rs:12 | no (moves fields between zones) | **JUSTIFIED** — never touches `doogat_type` |

## Conclusion

**Audit additions beyond the two PRD-named gaps: ZERO.** The complete set of
unprotected concurrent-writer SINGLETON-creation paths is exactly:

1. **GAP 1a — `update_doogat_parsed` (update.rs:27)**: no `BEGIN IMMEDIATE` wrap
   **and** no `check_singleton_update_constraint` call at all. Needs **both**.
   Schema resolution at L44 is gated on the *current* `parsed.meta.doogat_type`,
   so the new check must broaden to the **result** type or it is a silent no-op
   on the retype-into-singleton scenario the PRD targets.
2. **GAP 1b — `batch_update_with_message` (update.rs:115)**: already calls
   `check_singleton_update_constraint` per update (L192 via `prepare_update`),
   but Phases 1/2/3 run without an `IMMEDIATE` window, so check→commit→index is
   not atomic across processes. Needs the **window** (wrap when *any* update's
   result type is a registered singleton), not a new check.

Inherited entry points (`update_doogat`, `update`, `batch_update`) are fixed
transitively once 1a/1b are wrapped.

**Phase 2 (separate layer, not a service-wrap gap):** SQL `INSERT` determinism
(§55.D, CLI-create racing a PgWire INSERT) is owned by the dml.rs Layer-2
pre-check + Layer-3 lock-translation timing; the two-server startup upsert race
is in the actor/startup window around `ensure_fresh`, not the steady-state
`upsert_singleton` lock. Both are investigation-first Phase-2 tasks.
