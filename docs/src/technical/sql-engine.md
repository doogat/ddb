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
| `ALTER TABLE foo SET ZONE frontmatter FOR col` | Override column zone (custom DDL, pre-parse intercepted) |
| `ALTER TABLE foo SET TITLE TEMPLATE 'tpl'` | Set title template on typedef |
| `ALTER TABLE foo DROP TITLE TEMPLATE` | Remove title template from typedef |
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
- **SET ZONE**: Custom DDL (pre-parse intercepted). Updates column zone in typedef, rematerializes. Existing data doogats are NOT migrated — they stay in the old zone until next update.
- **SET/DROP TITLE TEMPLATE**: Custom DDL. Sets or removes `title_template` on the typedef. No rematerialization needed.

## Pre-Parse Interception

Three custom DDL statements are intercepted via regex before sqlparser parsing: `SET ZONE`, `SET TITLE TEMPLATE`, `DROP TITLE TEMPLATE`. These use `try_custom_ddl()` in `execute()` with OnceLock-cached regexes. Supports quoted identifiers for hyphenated names.

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

## Test Coverage

65+ unit tests covering CREATE TABLE, INSERT (single and multi-row), SELECT, UPDATE, DELETE, FK validation, zone mapping (type-aware inference, VARCHAR boundary, ENUM/SET extraction, blob types), duplicate rejection, reserved name rejection, ALTER TABLE (ADD/DROP/RENAME COLUMN), DROP TABLE (CASCADE, IF EXISTS), bulk UPDATE, bulk DELETE, 8 transaction tests, 8 rejection tests for unsupported SQL features, and 7 type-aware inference tests. 9 E2E tests in `tests/e2e/sql_lifecycle.rs`. 3 E2E tests for junction tables in `tests/e2e/junction_tables.rs` (round-trip CRUD, reindex survival, multiple REFERENCES columns). 4 E2E tests for upsert/conflict handling in `tests/e2e/upsert.rs` (onConflict argument, IGNORE returns existing, ERROR on duplicate, mixed new/existing batch).
