# Search Index

**Source**: `zdb-core/src/indexer.rs` (~1,366 lines)

SQLite-based search index with FTS5 full-text search, type inference, schema merging, and table materialization. The index is a derived cache — always rebuildable from the Git repository. No schema migration framework is needed: on full rebuild, all tables are dropped and recreated from the current schema definitions.

## Index

```rust
pub struct Index {
    conn: Connection,  // rusqlite::Connection
}
```

## Schema

Created on `Index::open()` (idempotent):

```sql
zettels(id TEXT PK, title, date, type, path UNIQUE, body, updated_at)
_zdb_tags(zettel_id FK, tag, source DEFAULT 'frontmatter')  -- index on tag; source: 'frontmatter' or 'body'
_zdb_fields(zettel_id FK, key, value, zone)  -- index on key
_zdb_links(source_id FK, target_path, display, zone)  -- index on target_path
_zdb_aliases(zettel_id FK, alias COLLATE NOCASE)  -- index on alias
_zdb_attachments(zettel_id FK, name, mime, size INTEGER, path)
_zdb_fts(title, body, tags)              -- FTS5 virtual table
_zdb_meta(key PK, value)                 -- staleness tracking
```

FTS5 uses `porter unicode61` tokenizer — porter stemming with Unicode support.

WAL journal mode is enabled for better concurrent read performance.

## Key Operations

### index_zettel

`index_zettel(zettel: &ParsedZettel) -> Result<()>`

Upserts a single zettel into all tables within a transaction:

1. Check if the zettel already exists (for FTS cleanup)
2. Delete old FTS entry if exists
3. `INSERT OR REPLACE` into `zettels`
4. Delete and re-insert `tags`, `fields`, `links`, `aliases`
5. Insert scalar frontmatter extras into `_zdb_fields` with `zone = 'Frontmatter'` (String, Number, Bool — skips List/Map)
6. Insert aliases from frontmatter `aliases` list (if present)
7. Delete and re-insert `attachments` from frontmatter `attachments` list (if present)
8. Insert new FTS entry

Uses a named `SAVEPOINT`/`RELEASE` pair (via `with_savepoint`) for atomic writes that nest correctly within SQL engine transactions.

### rebuild

`rebuild(repo: &GitRepo) -> Result<RebuildReport>`

Full index rebuild. Drops all tables (internal and materialized) and recreates the schema from scratch before re-indexing. This ensures schema changes take effect without migrations — the index is a disposable cache.

Phases:

1. **Drop & recreate** — drop every table, recreate internal schema from `SCHEMA_DDL`
2. **Index** — walk all `zettelkasten/*.md` paths in Git HEAD, parse and index each zettel
3. **Warn** — collect consistency warnings (malformed YAML, missing required fields, cross-zone duplicates)
4. **Materialize** — for each distinct type, merge typedef + inferred schema and create SQLite tables with data
5. Store current HEAD OID in `_zdb_meta` table

Full rebuild is only triggered by:
- Explicit `zdb reindex`
- Index corruption (detected by `check_integrity`)
- Unreachable HEAD OID (e.g. after `git gc`)

Normal operations (after `git pull`, direct file edits) use `incremental_reindex` instead, which only processes changed files without dropping tables.

Returns a `RebuildReport`:

```rust
pub struct RebuildReport {
    pub indexed: usize,
    pub tables_materialized: usize,
    pub types_inferred: Vec<String>,
    pub warnings: Vec<ConsistencyWarning>,
}
```

### incremental_reindex

`incremental_reindex(repo: &GitRepo, old_head: &str) -> Result<RebuildReport>`

Diffs `old_head` against the current HEAD and processes only changed files. Added or modified zettels are re-indexed; deleted zettels are removed. Falls back to full `rebuild` if the diff fails (e.g. old HEAD unreachable after gc).

This is the common path for keeping the index current after `git pull` or direct file edits — fast and non-destructive (no table drops).

### Integrity Check

`check_integrity() -> Result<bool>`

Runs `PRAGMA integrity_check` and verifies core tables exist (`zettels`, `_zdb_fts`, `_zdb_tags`, `_zdb_fields`, `_zdb_links`, `_zdb_aliases`, `_zdb_meta`). Returns `false` if corrupt.

### Staleness Detection

`is_stale(repo) -> Result<bool>`

Compares the HEAD OID stored in `_zdb_meta` table against the current Git HEAD. If they differ, the index is stale and needs rebuilding.

`rebuild_if_stale(repo) -> Result<Option<RebuildReport>>`

Checks integrity first (force rebuild if corrupt), then staleness. Returns `None` if already current and healthy.

## Type Inference

### infer_schema

`infer_schema(type_name: &str, repo: &GitRepo) -> Result<TableSchema>`

Scans all zettels of a given type and infers a `TableSchema`:

- **Frontmatter** extra keys → frontmatter columns (inferred as INTEGER, REAL, BOOLEAN, or TEXT)
- **Body** `## headings` → body TEXT columns
- **Reference** `key:: value` fields → reference columns

Type widening: if any zettel of the type has a non-matching value for a field, the type widens (INTEGER+REAL → REAL, any mismatch → TEXT).

### merge_schemas

`merge_schemas(typedef: Option<TableSchema>, inferred: TableSchema) -> TableSchema`

Merges an explicit `_typedef` with inferred columns:
- Typedef columns take precedence (type, zone, required flags preserved)
- Inferred columns fill gaps (new fields not defined in typedef)

### materialize_all_types

`materialize_all_types(repo: &GitRepo) -> Result<(usize, Vec<String>)>`

For each distinct type in the index:
1. Load `_typedef` if it exists
2. Infer schema from data zettels
3. Merge schemas (typedef wins)
4. Create SQLite table and populate with data
5. Log advisory for inferred-only types

Also creates empty tables for typedef-only types with no data zettels.

## Consistency Warnings

`collect_consistency_warnings(repo: &GitRepo) -> Vec<ConsistencyWarning>`

Scans all zettels and produces advisory warnings:

```rust
pub enum ConsistencyWarning {
    MalformedYaml { path: String, error: String },
    CrossZoneDuplicate { path: String, key: String },
    MissingRequired { path: String, type_name: String, field: String },
}
```

Warnings don't prevent indexing — zettels are always indexed best-effort.

## Alias Resolution

### resolve_alias

`resolve_alias(name: &str) -> Result<Option<String>>`

Case-insensitive lookup in `_zdb_aliases`. Returns the zettel ID if found.

Aliases are populated from the frontmatter `aliases` list during `index_zettel()` and cleaned up on `remove_zettel()`.

### resolve_wikilink

`resolve_wikilink(target: &str) -> Result<Option<String>>`

Three-step resolution chain:

1. **Path lookup** — check if `target` matches a `zettels.path` directly
2. **ID lookup** — try `resolve_path(target)` (exact zettel ID match)
3. **Alias lookup** — try `resolve_alias(target)`, then `resolve_path()` on the result

Returns `None` if no match found at any step.

### search

`search(query: &str) -> Result<Vec<SearchResult>>`

Runs the FTS query directly and returns just the hits.

Unlike `search_paginated`, this path does not issue a separate `COUNT(*)` query, so callers that only need ranked results avoid the extra pass over the FTS table.

### search_paginated

`search_paginated(query: &str, limit: usize, offset: usize) -> Result<PaginatedSearchResult>`

FTS5 `MATCH` query with:
- Snippets from body (32 tokens, `<b>` highlight tags)
- Rank ordering (FTS5 rank, lower = better match)
- `LIMIT`/`OFFSET` for pagination
- Separate `COUNT(*)` query for total count

```rust
pub struct SearchResult {
    pub id: String,
    pub title: String,
    pub path: String,
    pub snippet: String,
    pub rank: f64,
}

pub struct PaginatedSearchResult {
    pub hits: Vec<SearchResult>,
    pub total_count: usize,
}
```

### by_tag

`by_tag(prefix: &str) -> Result<Vec<String>>`

Hierarchical tag prefix query using `LIKE`. For example, `"client/"` matches `"client/acme"`, `"client/bigcorp"`, etc.

### backlinks

`backlinks(target_path: &str) -> Result<Vec<String>>`

Find all zettel IDs that link to the given target path.

### query_raw

`query_raw(sql: &str) -> Result<Vec<Vec<String>>>`

Execute arbitrary SQL. Returns rows as string vectors. Handles all SQLite value types (null, integer, real, text, blob).

## Test Coverage

20+ tests covering:
- Schema creation (idempotent)
- Index and query round-trip
- FTS search with term matching
- Tag prefix queries (hierarchical)
- Backlink queries
- Raw SQL join queries
- Upsert replaces old data
- Rebuild with staleness detection and `RebuildReport`
- Materialization of typed tables from `_typedef` zettels
- Type inference (frontmatter types, body headings, reference fields, empty type, type widening)
- Schema merging (typedef-only, inferred-only, overlap, no overlap)
- Consistency warnings (valid zettel, missing required)
- Integration: full cycle with inferred type
- Integration: typedef + inferred merge
- Integration: external edit reconciliation
- Integration: consistency warnings in rebuild
