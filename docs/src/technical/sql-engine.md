# SQL Engine

**Source**: `zdb-core/src/sql_engine.rs` (~3,400 lines)

Translates SQL DDL/DML statements into zettel CRUD operations. Tables map to zettel types — `CREATE TABLE` produces a `_typedef` zettel, `INSERT` produces a typed data zettel, etc.

## SqlEngine

```rust
pub struct SqlEngine<'a> {
    index: &'a Index,
    repo: &'a dyn ZettelStore,
    txn: Option<TransactionBuffer>,
}
```

All methods take `&mut self`. The CLI creates `SqlEngine` per invocation; the server actor has exclusive access via mpsc.

## Supported SQL

### DDL

| Statement | Effect |
|-----------|--------|
| `CREATE TABLE foo (name TEXT, count INTEGER)` | Creates a `_typedef` zettel for type `foo` |
| `ALTER TABLE foo ADD COLUMN bar TEXT` | Adds column to typedef schema; existing rows get NULL |
| `ALTER TABLE foo DROP COLUMN bar` | Removes column from typedef schema; orphaned data keys ignored |
| `ALTER TABLE foo RENAME COLUMN old TO new` | Renames column in typedef + rewrites all data zettels |
| `DROP TABLE foo` | Strips `type:` from data zettels, deletes typedef |
| `DROP TABLE foo CASCADE` | Deletes typedef + all data zettels |
| `DROP TABLE IF EXISTS foo` | No-op if table doesn't exist |

Column types: `TEXT`, `VARCHAR(n)`, `CHAR(n)`, `TINYTEXT`, `MEDIUMTEXT`, `LONGTEXT`, `INTEGER`, `REAL`, `BOOLEAN`, `BLOB` variants, `BINARY`, `VARBINARY`, `ENUM('a','b')`, `SET('x','y')`. Foreign keys via `REFERENCES other_type(id)`.

`ENUM` and `SET` columns extract `allowed_values` into the typedef schema and store as `TEXT` in SQLite.

### DML

| Statement | Effect |
|-----------|--------|
| `INSERT INTO foo (name, count) VALUES ('Widget', 42)` | Creates a data zettel with `type: foo` |
| `INSERT INTO foo (name) VALUES ('A'), ('B'), ('C')` | Creates N zettels in a single git commit; returns comma-separated IDs |
| `SELECT name, count FROM foo` | Queries the materialized table |
| `SELECT ... WHERE id = '...'` | Filters by zettel ID |
| `UPDATE foo SET count = 43 WHERE id = '...'` | Modifies the zettel and materialized row |
| `UPDATE foo SET status = 'done' WHERE priority > 5` | Bulk update — resolves matching IDs via SQLite |
| `UPDATE foo SET status = 'done'` | Updates all rows |
| `DELETE FROM foo WHERE id = '...'` | Removes the zettel and materialized row |
| `DELETE FROM foo WHERE status = 'done'` | Bulk delete — resolves matching IDs via SQLite |
| `DELETE FROM foo` | Deletes all rows |

## Multi-Row INSERT

`INSERT INTO t (cols) VALUES (...), (...), (...)` creates N zettels in a single git commit.

- **ID generation**: `unique_ids(count)` generates a base timestamp via `generate_unique_id`, then increments by 1 second per subsequent row — no sleeping between rows
- **Single commit**: all N files staged and committed together via `commit_files`
- **Return value**: comma-separated list of ZettelIds (e.g. `20260310120000,20260310120001,20260310120002`)
- **Transaction-aware**: within a `BEGIN`/`COMMIT` block, writes are buffered as usual
- **Folder-aware paths**: if the typedef has `folder: true`, created files go to `zettelkasten/{type}/{id}.md`; otherwise they stay flat at `zettelkasten/{id}.md`

## Zone Mapping

Each column maps to a zettel zone based on explicit `zone` field or inference:

| Zone | Storage | Examples |
|------|---------|----------|
| `frontmatter` | YAML `extra` fields | INTEGER, REAL, BOOLEAN, CHAR(n), VARCHAR(n<=255), TINYTEXT, ENUM, SET |
| `body` | `## heading` sections | TEXT, VARCHAR(n>255), MEDIUMTEXT, LONGTEXT, BLOB variants |
| `reference` | `- key:: value` lines | Wikilinks, FK references |

Zone inference uses `is_short_string_type()` (AST-based) at CREATE TABLE time and `is_short_string_type_str()` (string-based) at runtime in `effective_zone()`. The 255-char boundary follows MySQL convention. `effective_zone()` resolves: explicit zone from `_typedef` wins, then references, then numeric/short-string → frontmatter, else body. Columns with `allowed_values` always infer frontmatter.

## _typedef Zettel Format

A `_typedef` zettel defines a table schema:

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

- `build_typedef_zettel(id, schema)` — serialize a `TableSchema` to a `ParsedZettel`
- `schema_from_parsed(zettel)` — deserialize a `_typedef` zettel back to `TableSchema`
- `data_type_to_string(dt)` — convert AST `DataType` to stored string (preserves VARCHAR size)
- `is_short_string_type(dt)` — AST-based check for frontmatter-eligible string types
- `extract_allowed_values(dt)` — extract ENUM/SET variant names from AST

## Bulk Operations

UPDATE and DELETE support arbitrary WHERE clauses beyond `WHERE id = '...'`. The flow:

1. Try `extract_where_id` — fast path for single-row by ID
2. Fall back to `resolve_matching_ids` — delegates WHERE evaluation to SQLite, returns `Vec<(id, path)>` of matching rows
3. Apply changes to each zettel and commit in batch

Bare UPDATE/DELETE (no WHERE) operates on all rows of the table.

## ALTER TABLE

- **ADD COLUMN**: Appends to typedef schema, rematerializes. Existing data zettels untouched (NULL for new column).
- **DROP COLUMN**: Removes from typedef schema, rematerializes. Orphaned data keys in zettels are ignored.
- **RENAME COLUMN**: Rewrites typedef + all data zettels in a single commit. Uses `rename_key_in_zettel` for zone-aware renaming (frontmatter extra keys, body `## heading`, reference `- key::` lines).

## DROP TABLE

- Without CASCADE: strips `type:` from data zettels (they become untyped), deletes typedef via `commit_batch`
- With CASCADE: deletes typedef + all data zettels via `delete_files`
- IF EXISTS: no-op when table doesn't exist

## Transactions

`BEGIN`, `COMMIT`, and `ROLLBACK` wrap multiple DML statements into a single git commit.

### Execution Model

- `execute_batch(sql)` parses multiple semicolon-separated statements and executes them sequentially
- `execute(sql)` is a single-statement convenience wrapper

### How It Works

1. **BEGIN**: Creates a SQLite `SAVEPOINT zdb_txn` and initializes a `TransactionBuffer`
2. **DML within txn**: SQLite changes applied immediately (read-your-writes). Git writes buffered as `PendingWrite`/`PendingDelete` entries
3. **COMMIT**: Flushes buffered writes/deletes to git via `commit_batch` in a single commit (message: `"transaction"`), then `RELEASE zdb_txn`
4. **ROLLBACK**: Executes `ROLLBACK TO zdb_txn; RELEASE zdb_txn` to undo SQLite changes. Buffer discarded, git untouched

### Buffer Types

```rust
struct PendingWrite { path: String, content: String }
struct PendingDelete { path: String, zettel_id: String }
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

## Not Supported

These SQL features are explicitly rejected with descriptive error messages. They either operate only on the materialized cache (lost on reindex) or bypass git storage (causing zettel-cache divergence).

| Statement | Reason |
|-----------|--------|
| `CREATE INDEX` | Cache optimization only; indexes are rebuilt from zettel data on reindex |
| `CREATE VIEW` | Views store queries, not data; no zettel representation; lost on reindex |
| `CREATE VIRTUAL TABLE` | No zettel representation for virtual tables |
| `CREATE TRIGGER` | Triggers fire on cache mutations, not git commits |
| `ALTER INDEX` | Indexes are managed automatically |
| `DROP INDEX` / `DROP VIEW` | Cannot be created, so cannot be dropped |
| `INSERT OR REPLACE` / `REPLACE INTO` | Bypasses git; use explicit `DELETE` + `INSERT` |
| `INSERT ... ON CONFLICT` | Bypasses git; use explicit `INSERT` + `UPDATE` |
| `UPDATE ... FROM` | Ambiguous join-to-document mapping; decompose into `SELECT` + individual `UPDATE`s |

## Test Coverage

65+ unit tests covering CREATE TABLE, INSERT (single and multi-row), SELECT, UPDATE, DELETE, FK validation, zone mapping (type-aware inference, VARCHAR boundary, ENUM/SET extraction, blob types), duplicate rejection, reserved name rejection, ALTER TABLE (ADD/DROP/RENAME COLUMN), DROP TABLE (CASCADE, IF EXISTS), bulk UPDATE, bulk DELETE, 8 transaction tests, 9 rejection tests for unsupported SQL features, and 7 type-aware inference tests. 9 E2E tests in `tests/e2e/sql_lifecycle.rs`.
