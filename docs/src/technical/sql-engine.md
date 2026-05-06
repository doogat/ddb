# SQL Engine

**Source**: `ddb-core/src/sql_engine/`

Translates SQL DDL/DML statements into doogat CRUD operations. Tables map to doogat types — `CREATE TABLE` produces a `_typedef` doogat, `INSERT` produces a typed data doogat, etc.

## SqlEngine

```rust
pub struct SqlEngine<'a> {
    index: &'a Index,
    repo: &'a dyn DoogatStore,
    txn: Option<TransactionBuffer>,
}
```

All methods take `&mut self`. The CLI creates `SqlEngine` per invocation; the server actor has exclusive access via mpsc.

## Supported SQL

### DDL

| Statement | Effect |
|-----------|--------|
| `CREATE TABLE foo (name TEXT, count INTEGER)` | Creates a `_typedef` doogat for type `foo` |
| `ALTER TABLE foo ADD COLUMN bar TEXT` | Adds column to typedef schema; existing rows get NULL |
| `ALTER TABLE foo DROP COLUMN bar` | Removes column from typedef schema; orphaned data keys ignored |
| `ALTER TABLE foo RENAME COLUMN old TO new` | Renames column in typedef + rewrites all data doogats |
| `ALTER TABLE foo RENAME TO bar` | Renames typedef + moves data folder + rewrites incoming REFERENCES (PRD 00132) |
| `ALTER TABLE foo ALTER COLUMN c TYPE t` | Changes column type; widening is metadata-only, narrowing pre-flights existing data |
| `ALTER TABLE foo SET ZONE frontmatter FOR col` | Override column zone (custom DDL, pre-parse intercepted) |
| `ALTER TABLE foo SET TITLE TEMPLATE 'tpl'` | Set title template on typedef |
| `ALTER TABLE foo DROP TITLE TEMPLATE` | Remove title template from typedef |
| `ALTER TABLE foo SET SEARCH KEY col` | Set the column substring-matched by `field=val` searches |
| `ALTER TABLE foo DROP SEARCH KEY` | Reset to the default (`title`) |
| `DROP TABLE foo` | Strips `type:` from data doogats, deletes typedef |
| `DROP TABLE foo CASCADE` | Deletes typedef + all data doogats |
| `DROP TABLE IF EXISTS foo` | No-op if table doesn't exist |

Column types: `TEXT`, `VARCHAR(n)`, `CHAR(n)`, `TINYTEXT`, `MEDIUMTEXT`, `LONGTEXT`, `INTEGER`, `REAL`, `BOOLEAN`, `BLOB` variants, `BINARY`, `VARBINARY`, `ENUM('a','b')`, `SET('x','y')`. Foreign keys via `REFERENCES other_type(id)`.

`ENUM` and `SET` columns extract `allowed_values` into the typedef schema and store as `TEXT` in SQLite.

### UNIQUE Constraints

Table-level `UNIQUE` constraints are supported in `CREATE TABLE`:

```sql
CREATE TABLE membership (title TEXT, link_id VARCHAR(255), cat VARCHAR(255), UNIQUE(link_id, cat))
CREATE TABLE items (title TEXT, code VARCHAR(255), UNIQUE(code))
```

- Parsed into the typedef's `unique_together` field (a list of column-name lists)
- Enforced via `CREATE UNIQUE INDEX` on the materialized SQLite table
- Survive rematerialization (indexes are recreated from the typedef on reindex)
- Duplicate inserts fail with a UNIQUE constraint violation error
- Used by `INSERT ... ON CONFLICT DO NOTHING` for upsert detection

`CREATE INDEX` is not supported as a standalone statement (see [Not Supported](#not-supported)). Use `UNIQUE()` in `CREATE TABLE` instead.

`CREATE INDEX IF NOT EXISTS ...` and `CREATE UNIQUE INDEX IF NOT EXISTS ...` are tolerated as no-ops (info-level log, no error) so apps with legacy startup migrations keep booting after upgrade. Plain `CREATE [UNIQUE] INDEX` (no `IF NOT EXISTS`) continues to reject — that's an intentional declaration the caller should drop. PRD 00129 §3b.

UNIQUE-constraint violations carry `extensions.code = "UNIQUE_VIOLATION"` on the GraphQL surface plus structured `table` / `columns` / `values` fields. The legacy `message` text continues to mirror SQLite's `"UNIQUE constraint failed: <table>.<col>[, <table>.<col>]..."` so callers still string-matching the substring keep working. PRD 00129 §3a + §6.

`BOOLEAN` columns are stored as `INTEGER` (1/0) in SQLite. SQL `SELECT` queries against materialized type tables automatically coerce these values to `"true"`/`"false"` in the response. This coercion applies only to tables with typedefs; queries against raw internal tables (`_ddb_*`, `doogats`) return uncoerced values.

### DEFAULT NEXT (auto-increment)

A ddb SQL extension for auto-incrementing INTEGER columns. Not standard SQL.

```sql
CREATE TABLE items (name TEXT, pos INTEGER DEFAULT NEXT)
CREATE TABLE memberships (cat TEXT, sort_order INTEGER DEFAULT NEXT(cat))
```

**Simple NEXT**: On INSERT, columns with `DEFAULT NEXT` resolve to `MAX(col) + 1` across the entire table (or 1 if empty). Explicit values are respected; the next auto value will be `MAX + 1` after any manual insert.

**Partitioned NEXT(col)**: `DEFAULT NEXT(partition_col)` scopes the MAX query with `WHERE partition_col = ?`, giving independent sequences per partition value. Useful for per-category sort ordering.

**Behavior**:
- MAX-based, not gap-filling. Deleting row 2 and inserting again yields 4, not 2.
- Simple NEXT: multi-row INSERT computes MAX once and increments in-memory across rows.
- Partitioned NEXT(col): queries MAX per row (previous rows visible via SQLite within same connection).
- Only valid on INTEGER columns. `VARCHAR DEFAULT NEXT` is rejected at CREATE TABLE time.
- Stored as `"NEXT"` or `"NEXT(col)"` in the typedef's `default_value` field.

### DML

| Statement | Effect |
|-----------|--------|
| `INSERT INTO foo (name, count) VALUES ('Widget', 42)` | Creates a data doogat with `type: foo` |
| `INSERT INTO foo (name) VALUES ('A'), ('B'), ('C')` | Creates N doogats in a single git commit; returns comma-separated IDs |
| `INSERT INTO foo (code) VALUES ('X') ON CONFLICT DO NOTHING` | Skips insert if `unique_together` constraint matches; returns existing ID |
| `SELECT name, count FROM foo` | Queries the materialized table |
| `SELECT ... WHERE id = '...'` | Filters by doogat ID |
| `UPDATE foo SET count = 43 WHERE id = '...'` | Modifies the doogat and materialized row |
| `UPDATE foo SET status = 'done' WHERE priority > 5` | Bulk update — resolves matching IDs via SQLite |
| `UPDATE foo SET status = 'done'` | Updates all rows |
| `DELETE FROM foo WHERE id = '...'` | Removes the doogat, cascades junction + reference cleanup |
| `DELETE FROM foo WHERE status = 'done'` | Bulk delete with cascade — resolves matching IDs via SQLite |
| `DELETE FROM foo` | Deletes all rows with cascade |

## Multi-Row INSERT

`INSERT INTO t (cols) VALUES (...), (...), (...)` creates N doogats in a single git commit.

- **ID generation**: `unique_ids(count)` generates a base timestamp via `generate_unique_id`, then increments by 1 second per subsequent row — no sleeping between rows
- **Single commit**: all N files staged and committed together via `commit_files`
- **Return value**: comma-separated list of DoogatIds (e.g. `20260310120000,20260310120001,20260310120002`)
- **Transaction-aware**: within a `BEGIN`/`COMMIT` block, writes are buffered as usual
- **Folder-aware paths**: if the typedef has `folder: true`, created files go to `ddb/{type}/{id}.md`; otherwise they stay flat at `ddb/{id}.md`

## Shared typed-insert helper (PRD 00133)

The data-shaping core of `INSERT` lives in `sql_engine::typed_insert::prepare_typed_insert_validate` (default resolution + `allowed_values` + FK existence) and `builders::build_data_doogat` (zone routing + `ParsedDoogat` construction). These helpers are also called by the service layer's `batch_create::prepare_create` and `create_doogat_with_extra` so that every typed-create entry point — SQL `INSERT`, GraphQL `createDoogat` / `createMany`, CLI `ddb create` — produces identical doogats for identical inputs. Notably:

- REFERENCES column values land in the *reference zone* (`- col:: [[id]]`), not in frontmatter `extra`. Junction-style typedefs (a typedef whose columns are mostly REFERENCES) populate cleanly via every entry point.
- FK existence is validated against the *referenced typedef's* materialized table (`SELECT 1 FROM "<ref_type>" WHERE id = ?`), not the generic `doogats` index. An FK pointing at an id of the wrong type rejects with `REFERENCES_VIOLATION`.
- `allowed_values` (ENUM) constraints are checked once, in the helper, instead of being duplicated across the SQL and service paths.

## Date Defaulting

`INSERT` without an explicit `date` column defaults the doogat's `date` field to the date portion of its ID. Since DoogatId is a 14-digit timestamp (`YYYYMMDDHHmmss`), slicing `id[0..4]-id[4..6]-id[6..8]` produces a `YYYY-MM-DD` date.

- **No date column in INSERT**: `date` derived from the doogat ID (e.g. ID `20260401074007` produces `date: 2026-04-01`)
- **Explicit date column in INSERT**: the provided value is used as-is
- **Schema column named `date`**: promoted to `meta.date` (not duplicated in frontmatter extras)

This ensures `created_at` in GraphQL responses is always non-null for SQL-inserted doogats, matching the behavior of CLI-created doogats (which use `chrono::Local::now()`).

## Zone Mapping

Each column maps to a doogat zone based on explicit `zone` field or inference:

| Zone | Storage | Examples |
|------|---------|----------|
| `frontmatter` | YAML `extra` fields | INTEGER, REAL, BOOLEAN, CHAR(n), VARCHAR(n<=255), TINYTEXT, ENUM, SET |
| `body` | `## heading` sections | TEXT, VARCHAR(n>255), MEDIUMTEXT, LONGTEXT, BLOB variants |
| `reference` | `- key:: value` lines | Wikilinks, FK references |

Zone inference uses `is_short_string_type()` (AST-based) at CREATE TABLE time and `is_short_string_type_str()` (string-based) at runtime in `effective_zone()`. The 255-char boundary follows MySQL convention. `effective_zone()` resolves: explicit zone from `_typedef` wins, then references, then numeric/short-string → frontmatter, else body. Columns with `allowed_values` always infer frontmatter.

## _typedef Doogat Format

A `_typedef` doogat defines a table schema:

```yaml
---
id: 20260226143000
title: project
type: _typedef
columns:
  - name: completed
    data_type: BOOLEAN
    zone: frontmatter
  - name: parent
    data_type: TEXT
    zone: reference
    references: project
crdt_strategy: preset:append-log
template_sections:
  - Description
  - Log
title_template: name-template
origin: prd-00030
---
```

### Key Functions

- `build_typedef_doogat(id, schema)` — serialize a `TableSchema` to a `ParsedDoogat`
- `schema_from_parsed(doogat)` — deserialize a `_typedef` doogat back to `TableSchema`
- `data_type_to_string(dt)` — convert AST `DataType` to stored string (preserves VARCHAR size)
- `is_short_string_type(dt)` — AST-based check for frontmatter-eligible string types
- `extract_allowed_values(dt)` — extract ENUM/SET variant names from AST

## Bulk Operations

UPDATE and DELETE support arbitrary WHERE clauses beyond `WHERE id = '...'`. The flow:

1. Try `extract_where_id` — fast path for single-row by ID
2. Fall back to `resolve_matching_ids` — delegates WHERE evaluation to SQLite, returns `Vec<(id, path)>` of matching rows
3. Apply changes to each doogat and commit in batch

Bare UPDATE/DELETE (no WHERE) operates on all rows of the table.

## ALTER TABLE

- **ADD COLUMN**: Appends to typedef schema, rematerializes. Existing data doogats untouched (NULL for new column).
- **DROP COLUMN**: Removes from typedef schema, rematerializes. Orphaned data keys in doogats are ignored.
- **RENAME COLUMN**: Rewrites typedef + all data doogats in a single commit. Uses `rename_key_in_doogat` for zone-aware renaming (frontmatter extra keys, body `## heading`, reference `- key::` lines).
- **RENAME TO** (PRD 00132): Renames the typedef itself. One git commit covers (a) the typedef's `title:` field, (b) every data doogat under `ddb/{old}/` moved to `ddb/{new}/` with its `type:` frontmatter rewritten (folder-typed only — flat-layout doogats keep their path), (c) every other typedef whose `columns:` lists `references: {old}` rewritten to `references: {new}`, and (d) any path-based wikilinks pointing into `ddb/{old}/` rewritten to `ddb/{new}/`. ID-based wikilinks (bare 14-digit IDs) are unaffected because doogat IDs don't move. The materialized SQLite table is renamed via `ALTER TABLE` after the git commit succeeds, then `rematerialize_type` rebuilds reference subtables under the new name. Validation runs up front: target name must be a valid identifier (passes the same shape rules as `is_valid_graphql_name`), must not be reserved (`doogats`, `_typedef`, `_ddb_*`, `sqlite_*`), and must not already correspond to a typedef. Out of scope for v1: column-level REFERENCES redirect (`ALTER COLUMN ... SET REFERENCES`), the MySQL alias `RENAME TABLE foo TO bar` (rejected with an explicit hint), renaming internal tables. Crash recovery: a kill between the git commit and the SQLite ALTER is handled by the next rebuild — `materialize_all_types` drops any orphan materialized table whose name no longer matches a typedef.
- **ALTER COLUMN TYPE**: Changes a column's declared data type on the typedef. Widening (`VARCHAR(N)` → wider `VARCHAR`, `VARCHAR`/`CHAR` → `TEXT`) is metadata-only. Narrowing (`VARCHAR` → smaller `VARCHAR`, `TEXT` → `VARCHAR(N)`) and numeric cross-conversion (`INTEGER` ↔ `REAL`) run a pre-flight scan over the materialized table; any existing row that would violate the new type rejects the statement with a row-count message and no change is persisted. REFERENCES columns only accept widening. `BOOLEAN` conversions are not supported. The PostgreSQL shorthand (`ALTER COLUMN c TYPE t`) is normalized to the standard `SET DATA TYPE` form via regex rewrite before parsing.
- **SET ZONE**: Custom DDL (pre-parse intercepted). Updates column zone in typedef, rematerializes. Existing data doogats are NOT migrated — they stay in the old zone until next update.
- **SET/DROP TITLE TEMPLATE**: Custom DDL. Sets or removes `title_template` on the typedef. No rematerialization needed.
- **SET/DROP SEARCH KEY**: Custom DDL (ddb#15 follow-up). Sets `search_key` on the typedef so `field=val` substring searches resolve through `<col>` instead of the default `title`. Validates that `<col>` exists on the typedef and is not a `REFERENCES` column. Mirrored into `_ddb_meta(key='search_key:<type>')` for fast filter-time lookup. No rematerialization needed.

## Pre-Parse Interception

Five custom DDL statements are intercepted via regex before sqlparser parsing: `SET ZONE`, `SET TITLE TEMPLATE`, `DROP TITLE TEMPLATE`, `SET SEARCH KEY`, `DROP SEARCH KEY`. These use `try_custom_ddl()` in `execute()` with OnceLock-cached regexes. Supports quoted identifiers for hyphenated names.

## DROP TABLE

- Without CASCADE: strips `type:` from data doogats (they become untyped), deletes typedef via `commit_batch`
- With CASCADE: deletes typedef + all data doogats via `delete_files`
- IF EXISTS: no-op when table doesn't exist

## Transactions

`BEGIN`, `COMMIT`, and `ROLLBACK` wrap multiple DML statements into a single git commit.

### Execution Model

- `execute_batch(sql)` parses multiple semicolon-separated statements and executes them sequentially
- `execute(sql)` is a single-statement convenience wrapper

### How It Works

1. **BEGIN**: Creates a SQLite `SAVEPOINT ddb_txn` and initializes a `TransactionBuffer`
2. **DML within txn**: SQLite changes applied immediately (read-your-writes). Git writes buffered as `PendingWrite`/`PendingDelete` entries
3. **COMMIT**: Flushes buffered writes/deletes to git via `commit_batch` in a single commit (message: `"transaction"`), then `RELEASE ddb_txn`
4. **ROLLBACK**: Executes `ROLLBACK TO ddb_txn; RELEASE ddb_txn` to undo SQLite changes. Buffer discarded, git untouched

### Buffer Types

```rust
struct PendingWrite { path: String, content: String }
struct PendingDelete { path: String, doogat_id: String }
struct TransactionBuffer { writes: Vec<PendingWrite>, deletes: Vec<PendingDelete> }
```

### read_content Helper

DML handlers use `read_content(path)` instead of `repo.read_file(path)`. This checks the transaction buffer first (reverse search for latest write), falls back to git. This enables read-your-writes within a transaction.

### Commit Deduplication

On COMMIT, cancelled operations are filtered: if a path was written then deleted within the same transaction, neither the write nor the delete is sent to git (the file never existed in git).

### Safety

- **No nested transactions**: `BEGIN` while a transaction is active returns an error
- **Drop auto-rollback**: `impl Drop for SqlEngine` rolls back the savepoint if a transaction is still active (prevents dangling savepoints on panic or early return)
- **Error within transaction**: Errors propagate but the transaction stays active. User can still `ROLLBACK` explicitly
- **Process crash**: Implicit rollback — buffer is lost, savepoint is never released, SQLite auto-recovers

### Example

```sql
BEGIN;
INSERT INTO tasks (name) VALUES ('design');
INSERT INTO tasks (name) VALUES ('implement');
UPDATE tasks SET name = 'design v2' WHERE id = '20260304120000';
COMMIT;
-- Single git commit with message "transaction"
```

## Junction Tables (Multi-Value References)

Columns declared with `REFERENCES` produce junction tables that enable many-to-many relationships between doogat types. A single doogat can reference multiple targets for the same column, stored as multiple `- col:: [[target]]` lines in the reference section.

### Naming Convention

Junction tables follow the pattern `{type}_{column}`. For example:

```sql
CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)
```

produces a junction table named `bookmark_category`.

### DDL Auto-Creation

Junction tables are created automatically during `materialize_tables()` for every column with a `references` field in the typedef schema. Each junction table has two columns:

| Column | Type | Description |
|--------|------|-------------|
| `{type}_id` | TEXT | Foreign key to the owning doogat |
| `{column}_id` | TEXT | Foreign key to the referenced doogat |

Both columns together form the effective composite key. The junction table is populated during materialization by scanning each data doogat's reference section for matching `- col:: [[id]]` lines.

### Auto-Junction Sync on Typed INSERT and UPDATE (PRD 00134)

`INSERT INTO <typed_table>` and `UPDATE <typed_table> SET <ref_col> = ...` keep the auto-junction `{type}_{ref_col}` table in sync atomically with the row write. Both paths share `populate_junction_tables` / `sync_junction_tables_for_columns` on `Index` (see [`indexer.md`](indexer.md) for materialization details):

- **INSERT** — `SqlEngine::build_and_index_row` calls `Index::populate_junction_tables` after `insert_materialized_row` succeeds. Both writes land inside the existing `SAVEPOINT insert_row`, so a junction-insert failure rolls back the typed-table row and the `doogats` index row together.
- **UPDATE** — Both UPDATE call sites (`update_bulk_rows` and the single-row WHERE-id path) call `Index::sync_junction_tables_for_columns` after `update_materialized_row`, scoped to the columns named in the `SET` list. For each REFERENCES column in that set, the helper deletes existing junction rows for the doogat and reinserts using the parsed doogat's current values.

Junction tables therefore reflect the latest typed write without a `ddb reindex` step. Before this fix, only full rebuilds populated junctions, so SQL JOIN, GraphQL plural relation traversal, and PgWire JOIN against `<type>_<ref_col>` returned 0 rows after a fresh INSERT or stale rows after an UPDATE.

### INSERT Write-Through

```sql
INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('20260310120000', '20260310120001')
```

Appends a `- category:: [[20260310120001]]` line to the bookmark doogat's reference section and inserts a row into the materialized junction table. Multiple inserts add multiple reference lines.

### DELETE Write-Through

```sql
DELETE FROM bookmark_category WHERE bookmark_id = '20260310120000' AND category_id = '20260310120001'
```

Removes the matching `- category:: [[id]]` line from the bookmark doogat's reference section and deletes the row from the materialized junction table.

### DROP CASCADE

`DROP TABLE bookmark CASCADE` deletes all bookmark data doogats, the bookmark typedef, the materialized `bookmark` table, **and** all associated junction tables (`bookmark_category`, etc.).

### Cascade on Data Delete

When a doogat is deleted via `DELETE FROM`, two cascade operations happen automatically:

1. **Junction cleanup**: all junction table rows where the deleted ID appears as a referenced target are removed. For example, deleting a category removes all `bookmark_category` rows where `category_id` matches.

2. **Dangling reference removal**: all doogats that link to the deleted doogat via wikilinks in their reference section have those lines removed and are re-committed.

Both operations, plus the original delete, land in a single atomic git commit. Inside a transaction, they are buffered and committed together with other transaction operations.

### RESTRICT for NOT NULL REFERENCES

Columns declared as `NOT NULL REFERENCES other(id)` enforce **RESTRICT** semantics on the referenced parent's delete. If any row in any typed table currently holds the parent's id in such a column, the delete is rejected with `cannot delete '<id>': NOT NULL REFERENCES from <table>.<column> in row '<blocker>'` — the parent and the child stay intact.

The check fires for every delete entry point: SQL `DELETE FROM`, the `deleteDoogat` GraphQL mutation, and `ddb delete <id>` on the CLI. Bulk SQL deletes are atomic: if any matched id has a required-FK dependent, the whole statement is rejected and no rows are deleted.

Nullable `REFERENCES` columns are unaffected — the existing wikilink-strip cascade still applies, and the parent delete still proceeds. Issue [#10](https://github.com/doogat/ddb/issues/10).

### ON DELETE CASCADE (PRD 00129 §2)

A `REFERENCES` column can opt into `ON DELETE CASCADE` at typedef declaration time. When the referenced parent is deleted, every row that holds the parent's id through a CASCADE-marked column is also deleted. Cascade walks recursively through chains of CASCADE references, all in a single git commit.

```sql
CREATE TABLE "category-membership" (
  link     VARCHAR(255) NOT NULL REFERENCES link(id)     ON DELETE CASCADE,
  category VARCHAR(255) NOT NULL REFERENCES category(id) ON DELETE RESTRICT
)
```

Supported actions for v1: `RESTRICT` (default, behavior above) and `CASCADE`. `SET NULL`, `SET DEFAULT`, and `ON UPDATE` are rejected at `CREATE TABLE` time.

Mixed actions on one table behave per-column independently — the example above lets a `link` delete cascade through the membership rows while a `category` delete with live memberships still rejects.

**Cycle detection**: a cascade walk that would re-enter a node already in the in-progress set rejects with `cascade delete would form a cycle through <tables>` (`extensions.code = "CASCADE_CYCLE"`). No arbitrary depth limit; only true cycles reject.

The action persists on the typedef as a per-column `on_delete: cascade` field; absent or any other value parses as `RESTRICT`.

## Expressions in INSERT and UPDATE

INSERT VALUES and UPDATE SET positions accept expressions beyond literal values.

### Scalar Functions

An allowlist of deterministic scalar functions is supported:

| Function | Description |
|----------|-------------|
| `COALESCE(a, b, ...)` | First non-NULL argument |
| `IFNULL(a, b)` | `a` if non-NULL, else `b` |
| `NULLIF(a, b)` | NULL if `a = b`, else `a` |
| `ABS(x)` | Absolute value |
| `LENGTH(s)` | String length |
| `LOWER(s)` | Lowercase |
| `UPPER(s)` | Uppercase |
| `TRIM(s)` | Strip leading/trailing whitespace |
| `TYPEOF(x)` | SQLite type name |
| `MIN(a, b)` | Smaller of two values |
| `MAX(a, b)` | Larger of two values |

Unlisted functions are rejected with a descriptive error.

### Subqueries

Scalar subqueries work in expression positions:

```sql
INSERT INTO items (sort_order) VALUES ((SELECT MAX(sort_order) FROM items))
UPDATE items SET sort_order = (SELECT MAX(sort_order) FROM items) + 1 WHERE id = '20260401120000'
```

### Arithmetic Operators

Binary operators `+`, `-`, `*`, `/`, and `||` (concatenation) work in expression positions, including nested/parenthesized expressions:

```sql
INSERT INTO items (sort_order) VALUES (COALESCE((SELECT MAX(sort_order) FROM items), 0) + 1)
UPDATE items SET name = LOWER(name) || '-archived' WHERE status = 'done'
```

## Not Supported

These SQL features are explicitly rejected with descriptive error messages. They either operate only on the materialized cache (lost on reindex) or bypass git storage (causing doogat-cache divergence).

| Statement | Reason |
|-----------|--------|
| `CREATE INDEX` | Cache optimization only; indexes are rebuilt from doogat data on reindex |
| `CREATE VIEW` | Views store queries, not data; no doogat representation; lost on reindex |
| `CREATE VIRTUAL TABLE` | No doogat representation for virtual tables |
| `CREATE TRIGGER` | Triggers fire on cache mutations, not git commits |
| `ALTER INDEX` | Indexes are managed automatically |
| `DROP INDEX` / `DROP VIEW` | Cannot be created, so cannot be dropped |
| `INSERT OR REPLACE` / `REPLACE INTO` | Bypasses git; use explicit `DELETE` + `INSERT` |
| `UPDATE ... FROM` | Ambiguous join-to-document mapping; decompose into `SELECT` + individual `UPDATE`s |

## Self-Contained Type Tables

Materialized type tables include all core doogat columns directly: `id`, `title`, `date`, `updated_at`, plus any type-specific columns from the typedef. No JOIN against the internal `doogats` table is needed.

```sql
-- All core fields available directly on the type table
SELECT id, title, date, updated_at, status FROM task
```

### Internal table visibility

Internal tables (`doogats`, `_ddb_tags`, `_ddb_fts`, `_ddb_links`, etc.) are hidden from schema introspection:

- **PgWire**: psql `\dt` shows only user type tables. Queries referencing `pg_catalog` return filtered results.
- **GraphQL**: introspection exposes only user-defined typed queries. The `doogats` query field is an intentional user-facing query for listing all doogats.
- **Direct access**: `SELECT * FROM _ddb_tags` still works for power users. Tables are hidden from catalog listings, not blocked from queries.

## Constraint enforcement

The SQL write path validates declared constraints before any side effects. Validation runs in `validate_row_against_schema` (see `ddb-core/src/sql_engine/dml.rs`) after `fill_defaults_and_validate` and before `build_and_index_row`, so a rejected write never produces a row in the materialized table or the `doogats` index.

### Enforcement matrix

| Constraint | INSERT | UPDATE | Notes |
|---|---|---|---|
| `NOT NULL` | enforced | enforced | INSERT: column absent (and no DEFAULT) or `VALUES (NULL, ...)`. UPDATE: only `SET col = NULL`; UPDATEs that leave the column untouched are fine. |
| `VARCHAR(N)` length | enforced | enforced | Character count, not byte count. No silent truncation — overflow rejects the write. |
| `CHAR(N)` length | enforced | enforced | Same rule as `VARCHAR(N)`. |
| `INTEGER` type | enforced | enforced | Value must parse as `i64`. |
| `REAL` / `FLOAT` / `DOUBLE` type | enforced | enforced | Value must parse as `f64`. |
| `BOOLEAN` type | enforced | enforced | Accepts `0`, `1`, `true`, `false`, `TRUE`, `FALSE`. Stored as `0`/`1` in SQLite. |
| `TEXT` / `BLOB` | accepted | accepted | Opaque, no shape check. |
| `DATE` / `DATETIME` / `TIMESTAMP` | not enforced | not enforced | Out of scope for now. Tracked as a follow-up. |
| Unknown columns | rejected | rejected | INSERT/UPDATE referencing a column not in `schema.columns` (and not in the reserved set `id`, `title`, `type`, `date`, `created_at`, `updated_at`, `tags`) is rejected. The SQL path now matches the GraphQL `Where` input typing behavior. |
| `ENUM` / `SET` (`allowed_values`) | enforced | enforced | Existing behavior, unchanged. |
| `REFERENCES` existence | enforced | (not re-checked) | Existing behavior, unchanged. |
| `unique_together` | enforced | n/a | Backed by a SQLite `UNIQUE INDEX` on the materialized table. |
| `CHECK` / `FOREIGN KEY` | not supported | not supported | Out of scope. |

### Error message format

All five constraint classes return `DoogatError::Validation` with one of these exact strings (clients can match on the prefix):

- `NOT NULL constraint violated: <table>.<column>`
- `value too long for <table>.<column>: <actual> chars exceeds limit <limit>`
- `type mismatch for <table>.<column>: expected <TYPE>, got '<value>'` (where `<TYPE>` is `INTEGER`, `REAL`, or `BOOLEAN`)
- `unknown column: <table>.<column>`

Through the GraphQL surface, these surface as `errors[].message` with `extensions.code = "VALIDATION_ERROR"`. Through the pgwire surface, they surface as a `SqlEngine`-classified error.

### Title is a real column

Before PRD 00122, declaring `title VARCHAR(255) NOT NULL` in `CREATE TABLE` was silently dropped — `extract_columns` skipped it because `title` is also stored in `meta.title`. The constraints went nowhere.

`title` is now retained in `schema.columns` with all its declared constraints. The materialized-row writer and the GraphQL schema builder skip core columns explicitly (`is_core_column` in `ddb-core/src/indexer/materialize.rs`) so the field still lives only in `meta.title`, but the validator sees its `required` flag and length cap.

### Reserved columns

The reserved set (`id`, `title`, `type`, `date`, `created_at`, `updated_at`, `tags`) is owned by the doogat pipeline. The names are exempt from the unknown-column check so existing INSERTs/UPDATEs that pass them through don't break, but they are **not** type-checked by the validator unless they were explicitly declared in `CREATE TABLE`. A value passed for `created_at` is accepted as a raw string at the SQL boundary; the pipeline derives the actual stored value from the doogat's id timestamp, and the user-supplied value may not survive.

To enforce constraints on a reserved column, declare it explicitly in `CREATE TABLE`. Only `title` is currently kept in `schema.columns` after declaration; the other reserved names are absorbed by the pipeline. If you need typed enforcement of `title`, write `title VARCHAR(255) NOT NULL` (or any other constraint combination) in the `CREATE TABLE` statement and the validator will check it on every INSERT/UPDATE.

### Removed: silent title fallback

Prior versions of `resolve_insert_title` had a 5-level priority chain:

1. Explicit `title` column
2. `title_template` interpolation
3. **First body column value (removed)**
4. **First frontmatter string column value (removed)**
5. `"{type} {id}"` last-resort fallback

Priorities 3 and 4 silently coerced unrelated fields like `url` or `description` into the title slot, masking what should have been a NOT NULL violation. They were removed in PRD 00122. The current chain is 1 → 2 → 5.

**Behavioral change**: a table whose `title` is `NOT NULL` and which has no `title_template` now rejects INSERTs that omit the title with `NOT NULL constraint violated: <table>.title`. Clients that relied on the fallback should either provide an explicit `title`, declare a `title_template`, or make the title nullable.

### REFERENCES-aware title_template (PRD 00127)

A `title_template` placeholder of the form `{col.field}` dereferences a `REFERENCES` column and pulls `field` off the target doogat. The column list in `parse_title_template` (ddb-core/src/sql_engine/helpers.rs) is reused by both the validator and the INSERT/UPDATE resolvers.

- Parsing rejects multi-hop paths (`{a.b.c}`) and malformed identifiers.
- `handle_title_template` validates dotted paths against the current schema (column exists, is `REFERENCES`) and against the target type's schema (field exists). `title` is accepted on any reference.
- `resolve_insert_title` (builders.rs) runs a `SELECT "field" FROM "target_type" WHERE id = ?1` per dotted placeholder, guarded by `pragma_table_info` to avoid SQLite's legacy double-quoted-string fallback. Missing target or NULL field renders empty.
- `recompute_template_title` (builders.rs) runs on `UPDATE` when the SET list touches any template-referenced column and no explicit `title` was supplied. It merges the updated values with the current materialized row and re-renders the template.
- Cascading re-title when the **target** doogat's field changes is out of scope. Consumers can fall back to `ddb fix` or an explicit `UPDATE` to repair stale junction titles.

### Limitations

- Pre-existing rows that violate a newly-added constraint are **not** validated retroactively at index rebuild time. The validator only runs on the SQL write path (INSERT and UPDATE). A `cargo test ... reindex` does not re-check stored data.

### Expression-synthesized NULL

`COALESCE(NULL, NULL)`, `IFNULL(NULL, NULL)`, `NULLIF(x, x)`, subqueries returning NULL, and other expression forms that resolve to SQL NULL **are** treated as NULL by the validator. The eval pipeline (`eval_values_nullable` in helpers.rs) preserves NULL through the round-trip and the validator rejects them on `NOT NULL` columns the same way it rejects bare `NULL` literals. Legitimate uses like `IFNULL(NULL, 42)` or `COALESCE(NULL, 'fallback')` resolve to non-NULL and pass validation as expected.

## Test Coverage

65+ unit tests covering CREATE TABLE, INSERT (single and multi-row), SELECT, UPDATE, DELETE, FK validation, zone mapping (type-aware inference, VARCHAR boundary, ENUM/SET extraction, blob types), duplicate rejection, reserved name rejection, ALTER TABLE (ADD/DROP/RENAME COLUMN), DROP TABLE (CASCADE, IF EXISTS), bulk UPDATE, bulk DELETE, 8 transaction tests, 8 rejection tests for unsupported SQL features, and 7 type-aware inference tests.

PRD 00122 added (all in `ddb-core/src/sql_engine/tests.rs`):

- 14 unit tests for `validate_row_against_schema` covering all 5 error message formats with explicit null/absent distinction.
- 4 INSERT-rejection unit tests + 4 UPDATE-rejection unit tests via `executeSql`, each asserting the row is absent from BOTH the materialized table AND the `doogats` index after rejection.
- 1 positive test for `title NOT NULL` + `title_template` + INSERT-without-explicit-title (cycle 2 D1 fix).
- 5 expression-synthesized NULL tests (`COALESCE(NULL, NULL)`, `IFNULL(NULL, NULL)`, etc.) for both INSERT and UPDATE paths (blind review C1 fix).
- 3 multi-row INSERT atomicity tests proving that a validation failure on row N writes none of rows 1..N (blind review C2 fix).
- 2 explicit empty-string tests pinning that `''` is rejected on INTEGER columns and accepted on TEXT.
- 6 integration-script checks (D1-D6 in section 43 of `tests/integration.sh` and `tests/integration.ps1`).

9 E2E tests in `tests/e2e/sql_lifecycle.rs`. 3 E2E tests for junction tables in `tests/e2e/junction_tables.rs` (round-trip CRUD, reindex survival, multiple REFERENCES columns). 4 E2E tests for upsert/conflict handling in `tests/e2e/upsert.rs` (onConflict argument, IGNORE returns existing, ERROR on duplicate, mixed new/existing batch).

### PRD 00124 regression test suite (groups A-G)

PRD 00124 closed the regression-coverage gap exposed by the jink-feedback integration sweep (issues #4-#8). Groups A-G map to specific files and line ranges; any new SQL correctness bug should extend the same cluster so future regressions trace back to both the originating issue and the jink repro.

**Upstream source.** The ported tests originate from shell scripts at `dev/local/specs/jink-feedback/ddb-repros/` (gitignored). The scripts use an inverted-assertion convention where exit 0 = bug reproduces. Running `bash dev/local/specs/jink-feedback/ddb-repros/run-all.sh` against a healthy build reports all four as FIXED and is a valid independent verification channel — the in-repo tests assert the opposite polarity.

**Coverage map.**

| Group | Bug | Rust cluster (`ddb-core/src/sql_engine/tests.rs`) | Integration (`tests/integration.sh`) | Smoke (`tests/smoke.sh`) |
|-------|-----|---------------------------------------------------|--------------------------------------|--------------------------|
| A1 | #4 cross-mutation parity | `update_after_unique_failure_succeeds_issue_4_a1`, `insert_after_unique_failure_succeeds_issue_4_a1`, `delete_after_unique_failure_succeeds_issue_4_a1` | Section 45 sub-block `45.A1` | Section 11 ghost-row pin |
| A2 | #4 restart persistence | — (integration only) | Section 45 sub-block `45.A2` (kill + restart) | — |
| A3 | #4 cross-table isolation | `failed_insert_on_table_a_does_not_corrupt_table_b_issue_4_a3` | Section 45 sub-block `45.A3` | — |
| B1-B5 | #5 UPDATE/DELETE no-match | `update_with_missing_id_returns_affected_zero` and neighbours | Section 18 sub-block `18z` (GraphQL) + Section 30 (CLI) | — |
| C1 | #6 search/normalize parity | `normalize_and_search_accept_same_inputs_issue_6_c1` in `search_query.rs` | Section 18h (PRD 00121) | — |
| D1-D6 | #7 constraint enforcement | `executesql_*_rejects_*` cluster + `validate_*` | Section 43.D (PRD 00122) | — |
| E1 | #8 JOIN pinning | `select_join_returns_joined_rows_issue_8_e1` plus CTE/subquery/UNION/window audit | Section 44 sub-block `44.E1` | — |
| F1 | #9 composite UNIQUE error | `composite_unique_duplicate_rejected_with_clear_error_issue_9_f1` | Section 30 `30.F1` | — |
| F2 | #9 single-col UNIQUE error | `single_column_unique_duplicate_rejected_with_clear_error_issue_9_f2` | — (pre-existing `create_table_with_unique_constraint_enforced` covers rejection) | — |
| F3 | #9 CREATE INDEX rejected | Pre-existing `create_index_rejected_with_reason` | Section 44 (pre-existing DDL consistency check) | — |
| F4 | #9 executeBatch atomicity | — | Section 18 sub-block `18z2` | — |
| F5-F7 | #9 updateDoogat tag semantics | — | Section 18 sub-block `18z3` | — |
| F9 | #9 SQL feature smoke | — | Section 18 sub-block `18z4` | — |
| F10 | #9 search limit boundaries | — | Section 18 sub-block `18z5` | — |
| F11 | #9 ALTER + typeDefs | — | Section 18 sub-block `18z6` | — |
| G1, G2 | #9 GraphQL schema contract | — | Section 18 sub-block `18z7` | — |

**Jink full-sweep port.** The 40+ checks from `validate-full-sweep.sh` are distributed across the existing section 17 and section 18 numbered neighbourhoods via sub-blocks `17.J1`, `17.J2`, `18z8`, `18z9`, `18z10`. The jink tables (`link`, `category`, `category-membership`, `quote`, `saved-search`, `pinned-result`, `jink-config`) are created once in `17.J1` and dropped at the end of `18z10`; they persist across the F-group and G-group sub-blocks between them.

**Property tests.** `ddb-core/tests/property_tests.rs` carries four SQL engine invariant properties under the `// SQL engine invariants` section header:

- P1 `sql_update_delete_by_id_invariant_p1` — forall valid_id ⇒ affected=1; forall invalid_id ⇒ affected=0
- P2 `search_normalize_round_trip_invariant_p2` — normalize ∘ compile_search_plan preserves validity
- P3 `sql_unknown_column_insert_rejected_p3` — INSERT with an unlisted column errors
- P4 `sql_unique_rollback_no_ghost_index_row_p4` — failed UNIQUE INSERT leaves no ghost row in the `doogats` index

Thorough runs: `PROPTEST_CASES=5000 cargo test -p ddb-core --test property_tests`.
