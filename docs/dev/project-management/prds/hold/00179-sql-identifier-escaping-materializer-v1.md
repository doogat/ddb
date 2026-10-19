---
default_model: opus
model_tier_rationale: equivalence obligation - helper promotion plus ~50 escaping sites must emit byte-identical DDL for safe names
---

# Escape SQL identifiers in the materializer and delete paths

**Priority:** P2 · **Roadmap:** Phase 2 (security / data-integrity) · **Depends on:** none

> **Provenance:** the sync-load charset boundary, the extended sink sweep (`dml.rs` / additional `materialize.rs` sites / `relation_filter.rs`), and the validator consolidation were added by the 2026-07-16 assessment (F3) and folded into the body's Features/tasks at the 2026-07-16 backlog review. Line numbers as of `8e4b4aa`; re-verify at execution.

## Overview

### Problem Statement
The schema-apply DDL path already escapes SQL identifiers safely, but the two *derived-index* SQL paths (materializer + delete) do not. This is a distinct, still-open instance of the class fixed for schema-apply in PRD 00161 (doubt review task 17).

- **Escaping exists — but only on the schema-apply path.** `quote_ident` (`ddb-core/src/schema_diff/plan.rs:14`) renders `format!("\"{}\"", name.replace('"', "\"\""))`, doubling any embedded `"`, and `validate_identifier` (`ddb-core/src/schema_diff/desired.rs:124`) rejects any identifier that is not `[A-Za-z_][A-Za-z0-9_-]*`. Both are applied *only* when applying a declarative schema. A **second** helper, `escape_sql_ident` (`ddb-core/src/indexer/filter.rs:7-9`), already exists for the where-filter/search path — but it only doubles quotes WITHOUT adding the surrounding quotes, so its 17 call sites wrap it manually (e.g. `indexer/search.rs:639`); consolidating onto `quote_ident` means converting those call sites, not just aliasing (grounded 2026-09-05).
- **The materializer interpolates identifiers bare-quoted with no escaping.** In `ddb-core/src/indexer/materialize.rs`: `junction_table_ddl` (`:92`-`:99`) interpolates `table`/`parent_id`/`ref_id`; `drop_and_create_materialized_table` (`:103`-`:137`) interpolates `schema.table_name`/`col.name` into the junction DROP (`:108`-`:111`), the main DROP (`:118`), and the main CREATE (`:124`-`:128`); `build_column_definitions` (`:40`) interpolates user column names; `create_unique_indexes` (`:149`-`:173`) interpolates the column list (`:158`-`:162`), index name (`:163`), and CREATE UNIQUE INDEX (`:165`-`:168`); `create_singleton_lock_index` (`:183`-`:196`) interpolates the index name (`:187`) and table name (`:190`). Every one uses `format!("... \"{}\" ...", …)` with no doubling.
- **Delete does the same.** `execute_delete_plan` (`ddb-core/src/service/delete.rs:66`) runs `format!("DELETE FROM \"{}\" WHERE id = ?1", dtype)` (`:104`), where `dtype` is the stored type read from `SELECT type FROM doogats` (`:94`); the statement is fired as `let _ = …execute(…)` (`:103`) so any error is swallowed.
- **The GraphQL where-compiler bare-quotes too.** `ddb-server/src/filter.rs` interpolates `format!("\"{col}\"")` on schema-resolved columns - not attacker-reachable like the materializer names, but the same missing-helper pattern; folded in here for invariant I9 uniformity (previously deferred to a follow-up that never got a PRD).
- **The names are attacker-influenceable.** `table_name`/`dtype` derive from a doogat's `type:` frontmatter and column names from frontmatter field keys. The doogat create path applies no charset guard: `create_doogat_raw` (`ddb-core/src/service/create.rs:421`, doc comment `:412`-`:420`) explicitly accepts unregistered `type:` values verbatim ("authors can write `type: foo`"). So a `"`-bearing type name is not rejected at create.
- **Bounded impact.** `rusqlite::execute` runs a single statement (no stacked `;` `DROP`), and delete swallows the error, so the realistic failure mode is malformed / injected *single-statement* DDL that fails rematerialization or corrupts a table (denial of service / integrity), not data exfiltration.
- **UNVERIFIED:** the full end-to-end reachability of a `"`-bearing `type:` surviving parse + sync and reaching `drop_and_create_materialized_table` / `execute_delete_plan` was not traced. This PRD hardens the sink regardless; the reachability question is stated plainly, not assumed.

### Target Users
Downstream apps (jink) that sync content from multiple, not-fully-trusted sources. A malicious or merely odd synced `type:` name or field key must not be able to corrupt the SQLite index or wedge reindex.

### Success Metrics
- Every identifier interpolation in `materialize.rs` and `delete.rs` is escaped through a single shared quote-doubling helper, matching the schema-apply DDL path.
- One canonical identifier-quoting helper lives in a `ddb-core` util and is reused by `plan.rs`, `indexer/filter.rs` (replacing `escape_sql_ident`), `materialize.rs`, and `delete.rs` — no divergent copies.
- The GraphQL where-compiler (`ddb-server/src/filter.rs`) routes its column-identifier interpolations through the same canonical helper - no bare `format!("\"{col}\"")` remains in any derived-SQL path.
- The doogat write boundary rejects a `type:` name or field key containing `"` or control characters with a clear error, while still accepting unknown-but-safe types (`type: foo`, `type: saved-search`) so the legacy contract is preserved.
- The SYNC-LOAD boundary (`schema_from_parsed`) enforces the same charset rule, so a hostile/odd typedef arriving via sync cannot reach the DDL builders unvalidated; one consolidated validator owns the rule.
- No bare identifier interpolation remains in ANY derived-SQL path (`materialize.rs`, `delete.rs`, `dml.rs`, `filter.rs`, `relation_filter.rs`) — asserted by a repo-wide pattern check in the tests.
- Regression test: a doogat whose `type:` or a field key contains `"` does not corrupt the DDL — it is either rejected at the write boundary or safely quoted so the table still materializes.

## Functional Decomposition

### Capability: Safe identifier quoting in derived-SQL paths
#### Feature: Shared quote_ident util in ddb-core
- **Description**: promote the private `quote_ident` into a shared `ddb-core` util so every derived-SQL path reuses one implementation.
- **Inputs**: an identifier `&str` (table, column, or index name).
- **Outputs**: a double-quoted, quote-doubled identifier string.
- **Behavior**: move `quote_ident` from `schema_diff/plan.rs` into `sql_engine` (e.g. `sql_engine::helpers::quote_ident`); `plan.rs` re-imports it (drops its private copy); fold `escape_sql_ident` (`indexer/filter.rs:7`) into the same canonical helper — its 17 manually-wrapped call sites switch from `format!("\"{}\"", escape_sql_ident(x))` to `quote_ident(x)` so the where-filter/search path uses it too; ALSO consolidate the duplicated VALIDATOR pair — `is_safe_sql_identifier` (`sql_engine/helpers.rs:96`) and `validate_identifier` (`schema_diff/desired.rs:124`) — into the same promoted module so the charset rule has one home; preserve each caller's accepted grammar via explicit validation modes rather than broadening or narrowing inputs.

#### Feature: Escape identifiers in the materializer
- **Description**: wrap every identifier interpolation in `materialize.rs` with the shared helper.
- **Inputs**: `schema.table_name`, `col.name`, junction parent/ref column names, unique-index and singleton-lock index names.
- **Outputs**: DDL whose identifiers are quote-escaped.
- **Behavior**: replace each `format!("\"{}\"", …)` in `junction_table_ddl`, `drop_and_create_materialized_table`, `build_column_definitions`, `create_unique_indexes`, and `create_singleton_lock_index` with `quote_ident(...)`.

#### Feature: Escape the delete DELETE-FROM identifier
- **Description**: escape the type name in `execute_delete_plan`'s `DELETE FROM`.
- **Inputs**: `dtype` (stored type name).
- **Outputs**: `DELETE FROM {quoted-type} WHERE id = ?1`.
- **Behavior**: interpolate `quote_ident(dtype)`; keep the parameterized `id`; keep the existing swallow-on-error semantics unchanged.

#### Feature: Escape where-compiler column identifiers
- **Description**: the GraphQL where-compiler quotes its column identifiers through the shared helper.
- **Inputs**: schema-resolved column names interpolated in `ddb-server/src/filter.rs`.
- **Outputs**: where-clause SQL whose identifiers are quote-escaped.
- **Behavior**: replace each bare `format!("\"{col}\"")` with the canonical helper. Columns here are schema-resolved (not raw user input), so this leg is uniformity/defense-in-depth for invariant I9, not a reachable-injection fix.

### Capability: Write-boundary identifier validation
#### Feature: Validate type name + field keys on create
- **Description**: reject clearly-unsafe type names / field keys at the create write boundary (defense-in-depth on top of escaping).
- **Inputs**: the resolved `doogat_type` and frontmatter field keys on every create verb.
- **Outputs**: a structured validation error for names containing `"` or control characters; otherwise unchanged.
- **Behavior**: add a charset guard in the service create/batch write path (`create.rs`, `batch.rs`) that fails loud for quote/control-char names but still accepts unknown safe types, preserving the legacy contract.

#### Feature: Charset guard at the sync-load boundary
- **Description**: the same charset rule enforced where SYNCED typedefs are loaded — the create-verb guard never runs for content synced from another device, which is exactly this PRD's stated threat model.
- **Inputs**: `schema_from_parsed` (`sql_engine/builders.rs:685-703`) — the single convergence point every synced `_typedef` passes through (table name from the doogat title; column names from the parsed `columns:` sequence).
- **Outputs**: an unsafe synced typedef name/column is rejected (or quarantined per the 00169 lenient policy) with a structured error naming the offending identifier — never silently loaded into DDL builders.
- **Behavior**: apply the consolidated validator inside `schema_from_parsed` before the `TableSchema` is constructed; safe-but-unknown names keep loading unchanged; interaction with 00169's per-item leniency documented (a rejected typedef degrades that type, not the whole reindex).

## Structural Decomposition

### Repository Structure

```
ddb-core/src/sql_engine/
├── dml.rs               # Maps to: remaining derived-SQL identifier sinks
├── builders.rs          # Maps to: synced typedef validation
└── helpers.rs           # Maps to: Shared quote_ident util (promoted from schema_diff/plan.rs)
ddb-core/src/schema_diff/
├── desired.rs           # Maps to: consolidated validator with typedef grammar
└── plan.rs              # Maps to: reuse the shared quote_ident (drop the private copy)
ddb-core/src/indexer/
├── filter.rs            # Maps to: replace escape_sql_ident and its callers
├── search.rs            # Maps to: shared identifier quoting at search sinks
└── materialize.rs       # Maps to: Escape identifiers in the materializer
ddb-core/src/service/
├── delete.rs            # Maps to: Escape the delete DELETE-FROM identifier
├── create.rs            # Maps to: Validate type name + field keys on create
└── batch.rs             # Maps to: Validate type name + field keys on create (batch path)
ddb-server/src/
├── relation_filter.rs   # Maps to: shared quoting at relation sinks
└── filter.rs            # Maps to: Escape where-compiler column identifiers
```

### Module: sql_engine (ddb-core/src/sql_engine/helpers.rs)
- **Maps to capability**: Safe identifier quoting in derived-SQL paths
- **Responsibility**: own the one canonical `quote_ident` used by every derived-SQL path.
- **Exports**: `quote_ident(name: &str) -> String`

### Module: indexer/materialize (ddb-core/src/indexer/materialize.rs)
- **Maps to capability**: Safe identifier quoting in derived-SQL paths
- **Responsibility**: emit table/junction/index DDL with escaped identifiers.
- **Exports**: `junction_table_ddl`, `build_column_definitions`, and the `Index` materialize methods (now escaping).

### Module: service/delete + service/create (ddb-core/src/service/)
- **Maps to capability**: Safe identifier quoting; Write-boundary identifier validation
- **Responsibility**: escape the delete identifier; validate type/field-key charset on create.
- **Exports**: `execute_delete_plan` (escaped); create/batch write path (validated).

### Module: remaining SQL producers and validation callers (ddb-core/src/schema_diff/plan.rs, desired.rs; indexer/filter.rs, search.rs; sql_engine/dml.rs, builders.rs; service/batch.rs; ddb-server/src/filter.rs, relation_filter.rs)
- **Maps to capability**: Safe identifier quoting in derived-SQL paths; Write-boundary identifier validation
- **Responsibility**: consume one quoting helper and explicit shared validation modes across every inventoried sink, including synced typedefs.
- **Exports**: existing query/schema/write APIs; obsolete helper copies removed.

## Dependency Graph

### Foundation Layer (Phase 0)
No dependencies - built first.

- **sql_engine::quote_ident (promotion)**: the one helper every later path reuses.

### Core Layer (Phase 1)
- **materialize + delete escaping**: Depends on [sql_engine::quote_ident] - escape every identifier interpolation.
- **plan.rs reuse**: Depends on [sql_engine::quote_ident] - drop the private copy.

### Integration Layer (Phase 2)
- **write-boundary validation + tests + docs**: Depends on [materialize + delete escaping].

## Implementation Phases

### Phase 0: Foundation
**Goal**: one canonical escaping helper in ddb-core.

**Tasks**:
- [ ] Promote `quote_ident` from `schema_diff/plan.rs` into `sql_engine::helpers`; re-import in `plan.rs` (no deps) - Acceptance: `plan.rs` uses the shared helper, its private copy is gone, schema-apply DDL tests stay green, a unit test pins quote-doubling on a `"`-bearing name.
- [ ] Consolidate escape_sql_ident and all 17 manually wrapped filter/search callers onto quote_ident; consolidate is_safe_sql_identifier and validate_identifier with explicit modes preserving write-boundary safe names and existing typedef grammar (depends on: quote_ident promotion) - Acceptance: old quoting/validator copies have no consumers, quoted names receive exactly one outer quote pair with doubled interior quotes, values remain bound parameters, and safe unknown foo/saved-search names plus stricter typedef cases retain their previous acceptance/rejection.

**Exit Criteria**: single shared `quote_ident`; schema-apply behavior unchanged.

### Phase 1: Core
**Goal**: escape every identifier in the materializer + delete paths.

**Tasks**:
- [ ] Replace every bare `format!("\"{}\"", …)` identifier interpolation in `materialize.rs` (junction, DROP/CREATE, column defs, unique + singleton-lock indexes) with `quote_ident` (depends on: Phase 0) - Acceptance: a materialize unit test with a `"`-bearing table/column name produces valid quote-doubled DDL and the table still materializes.
- [ ] Escape `dtype` in `execute_delete_plan`'s `DELETE FROM` via `quote_ident`, keeping the parameterized `id` and swallow-on-error semantics (depends on: Phase 0) - Acceptance: a delete unit test with a `"`-bearing type issues valid DDL; no corruption.
- [ ] Replace the bare `format!("\"{col}\"")` column interpolations in `ddb-server/src/filter.rs` with the shared `quote_ident` (depends on: Phase 0) - Acceptance: no bare-quoted identifier interpolation remains in the where-compiler; the existing where-filter tests stay green.
- [ ] Sweep the remaining bare identifier interpolations through `quote_ident`: `sql_engine/dml.rs` (INSERT `:1420`, UPDATE `:1456`, DELETE `:689,:734`, existence `:489`, singleton `:164,:1551`, cascade-junction `:1314,:1325`, unique-check `:1204,:1208`, col builders `:1617,:1661`, plus `:776,:780,:893` — 16 sites at `8fe51c9`), the additional `indexer/materialize.rs` sites (`:410,:711,:754,:873,:897,:924,:956,:979,:990,:1103`), and `ddb-server/src/relation_filter.rs` (`:73-74,:149,:167-168,:221,:226`); line refs re-verified at execution (depends on: Phase 0) - Acceptance: `rg 'format!\("[^"]*\\"\{' ` (or equivalent) finds no bare-quoted identifier interpolation in any derived-SQL path; existing dml/materialize/relation-filter tests stay green.

**Exit Criteria**: no unescaped identifier interpolation remains in `materialize.rs` / `delete.rs` / `dml.rs` / `ddb-server/src/filter.rs` / `relation_filter.rs`.

### Phase 2: Integration
**Goal**: write-boundary validation + regression coverage + docs.

**Tasks**:
- [ ] Add the charset guard on the create/batch write boundary (`create.rs`, `batch.rs`): reject `"`/control-char type names + field keys, still accept unknown safe types (depends on: Phase 1) - Acceptance: `type: foo"bar` is rejected with a clear error; `type: foo` and `type: saved-search` still succeed.
- [ ] Add the charset guard inside `schema_from_parsed` (`sql_engine/builders.rs:685-703`) for SYNCED typedefs — reject/quarantine unsafe table/column names with a structured error before `TableSchema` construction; document the interaction with 00169's per-item leniency (depends on: Phase 1) - Acceptance: a synced `_typedef` whose title contains `"` degrades that type with a structured error (never silently loads); a safe-but-unknown synced type still loads; a unit test pins both.
- [ ] Author regression coverage: core unit tests (above) + an e2e test in the Rust harness (`tests/e2e/`; the shell harness was removed by 00171) — a CLI create with an odd type name asserting no index corruption (depends on: Phase 1) - Acceptance: coverage authored and asserts the safe outcome.
- [ ] Update `docs/src/technical/indexer.md` + `docs/src/technical/errors.md` for the escaping + write-boundary validation; add a `CHANGELOG.md` `Fixed` entry; push `master` after commits land (depends on: Phase 1) - Acceptance: docs match; CHANGELOG entry present.

**Exit Criteria**: validation + tests + docs landed; behavior pinned.

## Test Strategy

**Validation:** per-task local gates are `cargo build`, `cargo clippy --workspace --all-targets`, and `cargo test-ci`, including colocated unit regressions. Author required e2e/integration/conformance scenarios and executable walkthroughs; their heavy execution belongs to Tier 2 CI, not the per-task gate. Retain only CLAUDE.md's conditional e2e run for Claude-routed tasks deleting/replacing code paths. Walkthrough creation uses showboat; any verification uses `dev/bin/safe-showboat-verify`, never the project root. This backlog assumes 00171 has completed the shell-to-Rust harness migration.

### Critical Scenarios
- **Happy path**: materialize/delete a normal type (`project`, `saved-search`) → Expected: identical DDL to today; all existing materialize/delete tests green.
- **Edge case**: a field key or type name containing `"` reaches materialization → Expected: `quote_ident` doubles the quote; the table materializes; no SQL error.
- **Error case**: create a doogat with `type: foo"bar` → Expected: rejected at the write boundary with a structured error, before any git write or materialization.

## Risks

- **Legacy unknown-type contract**: `create_doogat_raw` accepts arbitrary `type: foo`. Mitigation: the guard rejects only quote/control-char names (fail loud), never unknown-but-safe names; a test pins `type: foo` still working.
- **Where-compiler leg is uniformity, not injection**: `ddb-server/src/filter.rs` columns are schema-resolved, so that swap is defense-in-depth (folded in 2026-07-08 because the "separate follow-up" it was deferred to never got a PRD, leaving invariant I9 partly untracked). Mitigation: mechanical swap; the existing where-filter suite is the oracle. Note 00183 later splits `filter.rs` as a pure move; the helper usage moves with it.
- **Double-escaping**: promoting `quote_ident` must not leave a path that quotes twice. Mitigation: callers pass raw identifiers only; a unit test asserts a single level of quoting.

## Transport Impact

The escaping fix is internal to the indexer and delete path: it runs on every write and reindex regardless of transport, so it protects all interfaces uniformly. The write-boundary charset guard is a CRUD-create change every public interface's create verb inherits.

- **CLI**: Guaranteed — `ddb create` / typed create route through the validated write boundary; materialize/reindex now escape identifiers.
- **GraphQL**: Guaranteed — create/upsert mutations route through the same core write boundary and the same materializer.
- **REST**: Guaranteed — `POST` create routes through the same boundary; identical escaping.
- **FFI**: Guaranteed — `DoogatDriver` create / `create_doogat_raw` validate at the same core boundary.
- **PgWire**: Guaranteed — SQL DDL already validates via `validate_identifier`; SQL `INSERT` and reindex now escape identifiers on the derived-table path.
- **NoSQL HTTP**: Specialized — the document mirror emits no identifier DDL of its own, but it is rebuilt from the same escaped materialization, so it inherits the integrity fix transitively (no new surface).
- **Cross-interface conformance plan**: the odd-identifier regression is asserted at the core (materialize + delete unit tests) and pinned end-to-end through one transport (a CLI e2e create with a `"`-bearing type name that must not corrupt the index); the write-boundary guard is asserted on the create verb. jink impact: a malicious or merely odd synced type name can no longer corrupt jink's index.
