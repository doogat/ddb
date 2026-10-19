# Design: SQL identifier escaping (materializer, delete, dml, filters) + write-boundary charset guard

## Architecture fit

This work lands entirely inside `ddb-core`'s derived-SQL layer and its two
transport-level where-compilers; it touches no git/CRDT/sync code.

- `ddb-core/src/sql_engine/` owns SQL generation and validation. A new
  `sql_engine::helpers::quote_ident` becomes the one canonical identifier
  escaper every derived-SQL producer calls.
- `ddb-core/src/indexer/` (`materialize.rs`, `filter.rs`, `cascade.rs`,
  `search.rs`) owns the materialized-table DDL and the where-filter/search SQL.
- `ddb-core/src/schema_diff/` (`plan.rs`, `desired.rs`) owns schema-apply DDL
  and already has the escaping/validation this PRD generalizes.
- `ddb-core/src/service/` (`create.rs`, `batch.rs`, `delete.rs`) is the write
  boundary; `create.rs`/`batch.rs` get a new charset guard, `delete.rs` is
  already escaped (see Reuse inventory — correct the PRD's claim here).
- `ddb-server/src/filter.rs` and `ddb-server/src/relation_filter.rs` are the
  GraphQL where/relation compilers; they get the same helper for uniformity
  (schema-resolved columns, not attacker input).

## Module placement

- **New**: `quote_ident` function, added to `ddb-core/src/sql_engine/helpers.rs`
  (existing file that already owns `is_safe_sql_identifier`/`unquote_identifier`
  — no new file needed). Exported as `pub fn quote_ident`, with
  `pub use helpers::quote_ident;` added to `ddb-core/src/sql_engine/mod.rs`'s
  existing re-export list — **mandatory**, not optional: `ddb-server` is a
  separate crate (an ordinary path dependency on `ddb-core`, not a
  workspace-internal module), so `pub(crate)` visibility is invisible to
  `ddb-server/src/filter.rs` / `relation_filter.rs` regardless of module
  publicity. Without this re-export the design cannot deliver the PRD's "one
  canonical helper... no divergent copies" metric for those two call sites.
- **Edit — promote/consolidate**:
  - `ddb-core/src/schema_diff/plan.rs`: delete its private `quote_ident`
    (lines 13-18 doc+fn), `use crate::sql_engine::helpers::quote_ident;` instead.
  - `ddb-core/src/indexer/filter.rs`: delete `escape_sql_ident` (lines 7-9);
    every call site that today does `escape_sql_ident(x)` (used bare, for
    interpolation inside a clause fragment that itself supplies the quotes)
    becomes `quote_ident(x)` directly (it now supplies its own quotes) — so
    callers that previously wrote `format!("\"{}\"", escape_sql_ident(x))`
    simplify to `quote_ident(x)`, and callers that used
    `escape_sql_ident(x)` bare inside an already-quoted template change to
    insert `quote_ident(x)` and drop the surrounding literal quotes.
  - `ddb-core/src/indexer/mod.rs`: drop `pub(crate) use filter::escape_sql_ident;`
    re-export; its 5 call sites (`mod.rs:829,869,870,872,885,886,888`) switch
    to `quote_ident` the same way.
  - `ddb-core/src/indexer/search.rs`, `ddb-core/src/indexer/cascade.rs`: same
    mechanical switch (search.rs:580,631,639; cascade.rs:62-63,94-95).
  - `ddb-core/src/sql_engine/helpers.rs`: `is_safe_sql_identifier` stays (it is
    a validator, not a quoter — different contract, see Alternatives); add a
    doc comment cross-referencing `quote_ident` and `schema_diff::desired::validate_identifier`
    so the three related functions in two modules are discoverable from any one of them.
  - `ddb-core/src/schema_diff/desired.rs`: `validate_identifier` (line 124)
    stays as today's behavior (reject non-`[A-Za-z_][A-Za-z0-9_-]*`); no
    merge into one function — see Alternatives for why a single validator
    with modes is rejected.
- **Edit — escape identifier interpolation** (no helper consolidation, just
  wrap with `quote_ident`):
  - `ddb-core/src/indexer/materialize.rs`: `build_column_definitions` (col
    name at line 71, plus the CHECK clause's two `col.name` interpolations at
    62-66 — those are *value* literals via `'...'` quoting, already correct,
    leave alone); `drop_and_create_materialized_table` (junction DROP
    `:108-111`, main DROP `:118`, main CREATE `:124-128` via
    `build_column_definitions`); `create_unique_indexes` (col list `:160`,
    index name `:166-168`, table name `:167`); `create_singleton_lock_index`
    (index name `:190`, table name `:190`); `junction_table_ddl` (`:96-98`,
    table/parent_id/ref_id); the DROP at `:410`; the INSERT column list at
    `:1027`.
  - `ddb-core/src/sql_engine/dml.rs`: every bare `\"{table_name}\"` /
    `\"{col}\"` interpolation. Neither the PRD's 16-line inventory nor this
    design's own spot-check (`:164,:489,:692,:696,:809,:1120,:1305,:1371,
    :1415`) is authoritative — the two lists partially disagree because both
    were taken at different SHAs. **The `rg` command in Interfaces & contracts
    is the binding inventory at task-execution time; every line number
    anywhere in this doc is illustrative only.**
  - `ddb-server/src/filter.rs`: `:295,:496,:519,:539,:998-999` (column/alias
    interpolations).
  - `ddb-server/src/relation_filter.rs`: `:73-74,:149,:221,:226` (junction/
    column/alias interpolations).
  - **`ddb-core/src/service/delete.rs` and `ddb-core/src/sql_engine/delete.rs`:
    NO CHANGE NEEDED.** Both already call `escape_sql_ident(table)` wrapped in
    `format!("DELETE FROM \"{}\" ...", ...)`. The PRD's Problem Statement
    (written before this escaping landed) is stale on this one sink; the
    mechanical rename `escape_sql_ident` → `quote_ident` (dropping the
    surrounding literal quotes, since `quote_ident` supplies its own) is the
    only edit here, folded into the Phase-0 consolidation task, not a
    standalone "escape delete" task.
- **New — write boundary**. **Placement correction (post-review): the guard
  does NOT live under `service/`.** `sql_engine`/`indexer` never depend on
  `service` (`rg "use crate::service" ddb-core/src/sql_engine/*.rs
  ddb-core/src/indexer/*.rs` returns nothing outside test modules; `rg "use
  crate::sql_engine" ddb-core/src/service/*.rs` returns 10 hits — `service`
  depends on `sql_engine`, never the reverse). Since
  `sql_engine::builders::schema_from_parsed` must also call this guard (see
  Write-boundary flow below), a `service/`-owned guard would be the only
  `sql_engine` → `service` edge in the codebase, violating the mandatory
  Clean Architecture layering (`AGENTS.md`). The guard instead lives in
  `ddb-core/src/sql_engine/helpers.rs`, next to the new `quote_ident` and the
  existing `is_safe_sql_identifier` — both callers (`service::create`/
  `service::batch` and `sql_engine::builders`) call *up* into `sql_engine`,
  consistent with today's direction. No new file.
  - `ddb-core/src/service/create.rs`: calls
    `sql_engine::helpers::validate_write_identifiers(doogat_type, field_keys)`
    (or re-exported as `sql_engine::validate_write_identifiers`, matching
    `quote_ident`'s re-export) from `create_doogat_with_extra` (`:178`) before
    any zone/DDL work, and from `create_doogat_raw`'s caller path where a
    `type:` is resolved for an unregistered type (the doc-commented "authors
    can write `type: foo`" case at `:412-421` must still pass through the
    SAME guard — the PRD requires unknown-but-safe names to keep working,
    only `"`/control chars are rejected).
  - `ddb-core/src/service/batch.rs`: `batch_create_body` (`:169`) calls the
    same guard per input before `build_batch_create_from_parsed`/zone routing.
  - `ddb-core/src/sql_engine/builders.rs`: `schema_from_parsed` (`:685`) calls
    the same guard on the table name (doogat title) and every parsed
    `columns:` entry's name before constructing `TableSchema`; a rejection
    returns a structured error from `schema_from_parsed` itself, which its
    caller (the sync/reindex path) already treats per-item under 00169's
    lenient policy (skip-and-warn, not abort-the-batch).

## Interfaces & contracts

```rust
// ddb-core/src/sql_engine/helpers.rs
/// Double-quote and quote-double an identifier for safe SQL interpolation.
/// `foo` -> `"foo"`; `fo"o` -> `"fo""o"`. Never validates charset — pair with
/// `is_safe_sql_identifier` / `validate_identifier` at a write boundary when
/// rejecting is the goal; this function's job is only to make interpolation
/// safe for whatever string it is given.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
```

```rust
// ddb-core/src/sql_engine/mod.rs — add to the existing re-export list
pub use helpers::{quote_ident, validate_write_identifiers};
```

```rust
// ddb-core/src/sql_engine/helpers.rs — same file as quote_ident and the
// existing is_safe_sql_identifier; NOT a new service/ module (see Module
// placement — sql_engine never depends on service, and sql_engine::builders
// is itself a caller, so the guard must live on the sql_engine side of that
// boundary).
/// Error code: `INVALID_IDENTIFIER`. Rejects a name containing `"` or any
/// ASCII control character (0x00-0x1F, 0x7F). Accepts any other name,
/// including unknown-but-safe type names (`foo`, `saved-search`) — this is
/// NOT `is_safe_sql_identifier`'s grammar and must not be merged with it (see
/// Alternatives).
pub(crate) fn reject_unsafe_identifier(role: &str, name: &str) -> Result<()>;

/// Runs `reject_unsafe_identifier` over a doogat type name and its frontmatter
/// field keys. Called (via re-export, same mechanism as `quote_ident`) from
/// service::create, service::batch, and sql_engine::builders::schema_from_parsed.
pub fn validate_write_identifiers<'a>(
    doogat_type: &str,
    field_keys: impl Iterator<Item = &'a str>,
) -> Result<()>;
```

```rust
// ddb-core/src/error.rs — new variant (exact enum per existing error-mapping
// policy, PRD 00173): DoogatError::InvalidIdentifier { role: String, name: String }
// routes through the single error-mapping table to VALIDATION_ERROR / 400,
// same family as existing UNKNOWN_FIELD / NOT_NULL_VIOLATION codes.
```

Exhaustive current-sweep pattern (run at task execution, not pinned to the
PRD's line numbers which were taken at `8e4b4aa` and have since drifted):

```
rg -n 'format!\("[^"]*\\"\{' ddb-core/src/indexer/materialize.rs ddb-core/src/sql_engine/dml.rs ddb-server/src/filter.rs ddb-server/src/relation_filter.rs
```

Treat this exact command's output as the task's acceptance surface, not the
PRD's quoted line numbers (which are correct in spirit but off by small
amounts after intervening PRDs).

## Data flow

- **Materialize/DDL flow** (unchanged shape, now escaped): `TableSchema` →
  `drop_and_create_materialized_table` → `build_column_definitions` /
  `create_junction_tables` / `create_unique_indexes` /
  `create_singleton_lock_index`, each now wrapping every identifier through
  `quote_ident` before `format!`.
- **Delete flow** (already escaped, unchanged): `execute_delete_plan` reads
  `dtype` from `SELECT type FROM doogats`, escapes it (today via
  `escape_sql_ident`, after this PRD via `quote_ident`), issues
  `DELETE FROM "<dtype>" WHERE id = ?1`.
- **Write-boundary flow** (new gate, inserted before existing flow):
  `create_doogat_with_extra` / `batch_create_body` → NEW
  `validate_write_identifiers(doogat_type, field_keys)` → (on reject) return
  `DoogatError::InvalidIdentifier` immediately, no git write, no zone routing,
  no DDL; (on accept) existing flow unchanged.
- **Sync-load flow** (new gate): `schema_from_parsed(parsed_typedef)` → NEW
  `validate_write_identifiers(table_name, column_names)` before constructing
  `TableSchema` → (on reject) `schema_from_parsed` returns `Err`, caller (the
  per-typedef load loop feeding `materialize_all_types`/incremental reindex)
  treats it exactly like any other per-item schema-load failure under 00169's
  lenient policy — the type degrades (skipped, warned), reindex of other types
  continues.

## Reuse inventory

- `ddb-core/src/schema_diff/plan.rs:14` — `quote_ident` (private). Promote to
  `sql_engine::helpers`, drop the private copy. **Found, reuse by promotion.**
- `ddb-core/src/indexer/filter.rs:7` — `escape_sql_ident` (quote-doubles,
  caller supplies the surrounding quotes). Fold into the promoted
  `quote_ident` (which supplies its own quotes); its 20 call sites across
  `filter.rs`, `search.rs`, `cascade.rs`, `indexer/mod.rs` convert mechanically.
  **Found, reuse by consolidation.**
- `ddb-core/src/sql_engine/helpers.rs:96` — `is_safe_sql_identifier` (charset
  validator, `[A-Za-z_][A-Za-z0-9_-]*`-ish). **Found — stays separate from
  `quote_ident`**; it is the existing charset grammar for the typed-insert
  path (`typed_insert.rs`), not the new write-boundary guard's grammar (which
  is deliberately looser — rejects only `"`/control chars, see Risks).
- `ddb-core/src/schema_diff/desired.rs:124` — `validate_identifier` (rejects
  non-`[A-Za-z_][A-Za-z0-9_-]*`, used for schema-apply `name`/`rename`/
  `references`/`unique_together`/`search key`). **Found — stays**, same
  reasoning as above; this PRD does not touch schema-apply's stricter grammar.
- `ddb-core/src/service/delete.rs:72`, `ddb-core/src/sql_engine/delete.rs:72`
  — already call `escape_sql_ident(table)` inside `format!("DELETE FROM \"{}\"
  ...")`. **Found — already escaped.** The PRD's claim that delete is
  unescaped is stale; this PRD's only touch here is the mechanical
  `escape_sql_ident`→`quote_ident` rename (drop the now-redundant literal
  quotes), bundled with the Phase-0 consolidation task, not a separate fix.
- `nothing found, greps tried: "sanitize ident", "escape identifier", "quote
  column", "quote table"` — no third helper exists beyond the two found above.

## Alternatives considered

1. **Chosen: keep quoting (`quote_ident`) and charset validation
   (`is_safe_sql_identifier` / `validate_identifier` / the new
   `reject_unsafe_identifier`) as separate functions, with one quoter shared
   by all derived-SQL sinks and three validators left distinct because they
   enforce three different, deliberately different grammars** (typed-insert's
   internal charset, schema-apply's stricter declared-schema charset, and the
   new write-boundary's looser "just not `"`/control-chars" charset).
2. **Smallest-diff alternative**: escape only `materialize.rs` and
   `delete.rs` (the PRD's originally-scoped sinks), skip the `dml.rs`/
   `filter.rs`/`relation_filter.rs` sweep and the write-boundary guard
   entirely. Rejected: the PRD's Success Metrics explicitly require "No bare
   identifier interpolation remains in ANY derived-SQL path" and a
   write-boundary + sync-load guard; stopping at the smallest diff leaves the
   attacker-reachable `dml.rs` SQL-engine sinks (reachable via raw `INSERT`/
   `UPDATE`/`DELETE` SQL with attacker-controlled table/column names coming
   from a synced typedef) unescaped, defeating the PRD's stated goal.
3. **Rejected: one validator with explicit "modes" instead of three
   functions.** Collapsing `is_safe_sql_identifier`, `validate_identifier`,
   and the new write-boundary guard into one function with a mode enum was
   considered (the PRD's Phase 0 task literally proposes this: "consolidate
   ... with explicit modes"). Rejected in this design because the three
   grammars differ in more than strictness level — `validate_identifier`
   rejects on `role`-tagged field names for error messages, and the new guard
   must accept unknown-but-safe type names outright, a requirement the other
   two don't share. A mode-enum would turn into a grammar-selection
   indirection with three call sites each using a different mode, which reads
   as "three functions pretending to be one." Three small functions with
   clear names and cross-referencing doc comments (see Module placement) is
   the smaller real diff and the easier-to-audit outcome. This deliberately
   diverges from the PRD's literal task wording; call it out as a scoped
   decision at plan time, not silently drop the PRD's stated acceptance
   criterion — the task's *acceptance* ("old quoting/validator copies have no
   consumers" for `quote_ident`/`escape_sql_ident`) is still met; only the
   validator-consolidation half of that task is redirected to "add
   cross-referencing docs" instead of a literal merge.

## Risks & edge cases

- **Legacy unknown-type contract** (PRD's own risk): `create_doogat_raw`
  accepts arbitrary `type: foo`. The new guard must accept it; a unit test
  pins `type: foo` and `type: saved-search` still succeeding while `type:
  foo"bar` is rejected.
- **Double-escaping**: converting `escape_sql_ident(x)` call sites that were
  used bare (caller supplied the quotes) to `quote_ident(x)` (which supplies
  its own) risks one site ending up with a literal `""..""` double-quote pair
  if the surrounding literal quotes aren't also removed. Mitigation: the
  mechanical-conversion task's acceptance criterion requires a unit test
  asserting exactly one level of quoting on a `"`-bearing name for each
  converted call site family (materialize, filter/search, cascade, dml,
  relation_filter, graphql filter).
- **`dml.rs`'s raw-SQL surface is the actual attacker-reachable leg** beyond
  materialize/delete: a synced typedef's table/column name flows into
  `sql_engine::dml` via ordinary `INSERT`/`UPDATE`/`SELECT` against the
  materialized table. The write-boundary + sync-load guards close the create
  path; `dml.rs`'s own escaping closes the "already-synced-bad-name" residual
  (a name that got in before this PRD shipped, or arrived through a future
  bypass of the guard). Both legs are required; neither alone is sufficient.
- **Next likely changes**: (1) a future PRD may want the write-boundary
  guard's charset exposed as a typedef-declarable policy (stricter than
  "just not quote/control") — this design's `reject_unsafe_identifier` taking
  a `role` string (not yet a mode enum) leaves room to add a mode without
  reshaping call sites; (2) `relation_filter.rs`/`filter.rs` may gain
  additional interpolation sites as GraphQL filtering grows — the sweep
  pattern in Interfaces is the durable check future PRDs should re-run, not a
  one-time grep; (3) the error-mapping table (PRD 00173) gains one new
  variant here — any PRD that later restructures that table must carry
  `InvalidIdentifier` forward explicitly.

## Test strategy outline

- Unit tests (colocated, `cargo test-ci`):
  - `quote_ident` pins quote-doubling on a `"`-bearing name and the no-`"`
    passthrough case.
  - One unit test per converted call-site family (materialize DDL, delete,
    dml INSERT/UPDATE/SELECT, GraphQL filter, GraphQL relation_filter) with a
    `"`-bearing identifier producing valid, single-escaped DDL/SQL and (for
    materialize) a table that actually materializes.
  - `reject_unsafe_identifier` / `validate_write_identifiers`: `type: foo"bar`
    rejected; `type: foo`, `type: saved-search` accepted; a control-char field
    key rejected.
  - `schema_from_parsed` with a synced typedef title containing `"`: rejected
    with a structured error, `TableSchema` never constructed; a safe-but-
    unknown synced type still loads.
- Integration/e2e (Rust harness, Tier 2): a CLI create with an odd (`"`-
  bearing) type name must not corrupt the index — asserted end-to-end per the
  PRD's Critical Scenarios.
- Repo-wide pattern check: the `rg` command in Interfaces & contracts, run as
  part of the test suite (a `#[test]` that shells out to `rg` and asserts
  empty output, or a build-time check) — Success Metrics explicitly ask for
  this as an assertion, not a one-time manual sweep.

## Review log

- question: dml.rs sweep inventory listed illustrative line numbers inconsistent with the PRD's own list, risking confusion at task-authoring time about which sites exist — resolved by making the `rg` command binding and line numbers explicitly illustrative only.
dispatch 1 (claude): cardinal-sin 0, blocker 1, non-blocker 0, question 1
dispatch 2: codex unavailable, Claude fallback
- non-blocking: delete.rs already-escaped correction confirmed correct.
- non-blocking: quote_ident pub/re-export mechanics confirmed correct against ddb-server/Cargo.toml.
- non-blocking: is_safe_sql_identifier's `pub(super)` visibility may need a line addressing how the new guard relates to it now that it lives alongside it in the same file — addressed via the doc-comment cross-reference already called for in Module placement.
- question: design reinterprets PRD Success Metric "one consolidated validator owns the rule" rather than literally merging three validators — flagged as needing one explicit sentence naming the reinterpreted metric (already present in Alternatives #3; cross-referenced here for traceability).
dispatch 2 (claude-fallback): cardinal-sin 0, blocker 2, non-blocker 3, question 1
dispatch 3: codex unavailable, Claude fallback
dispatch 3 (claude-fallback): cardinal-sin 0, blocker 0, non-blocker 0, question 0
