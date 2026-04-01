# Search Index

**Source**: `ddb-core/src/indexer.rs` (~1,366 lines)

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
doogats(id TEXT PK, title, date, type, path UNIQUE, body, updated_at)
_ddb_tags(doogat_id FK, tag, source DEFAULT 'frontmatter')  -- index on tag; source: 'frontmatter' or 'body'
_ddb_fields(doogat_id FK, key, value, zone)  -- index on key
_ddb_links(source_id FK, target_path, display, zone)  -- index on target_path
_ddb_aliases(doogat_id FK, alias COLLATE NOCASE)  -- index on alias
_ddb_attachments(doogat_id FK, name, mime, size INTEGER, path)
_ddb_checkboxes(doogat_id FK, state, content, date, due_date, line_number INTEGER, indent_level INTEGER)
                                         -- indexes on state, doogat_id
_ddb_fts(title, body, tags, fields)      -- FTS5 virtual table
_ddb_boost(type_name TEXT PK, max_boost REAL DEFAULT 1.0) -- per-type FTS weight
_ddb_meta(key PK, value)                 -- staleness tracking
```

FTS5 uses `porter unicode61` tokenizer -- porter stemming with Unicode support.

The `fields` column (4th FTS5 column) contains space-joined frontmatter extra values (strings, numbers, booleans -- skipping `aliases` and `attachments`). This makes frontmatter field values searchable alongside title, body, and tags.

The `_ddb_boost` table stores `max(search_boost)` per type, derived from typedef column definitions during materialization. When a search is filtered to a single type that has boosted columns, the FTS5 `bm25()` weight for the `fields` column is set to that type's max boost value (e.g. `bm25(_ddb_fts, 1.0, 1.0, 1.0, 2.0)` for a type with `search_boost: 2.0`). Unfiltered searches and types without boosts use a weight of 1.0.

**Auto-upgrade**: on open, the indexer detects old 3-column FTS5 schemas (missing `fields`) or missing `_ddb_boost` table. When detected, all tables are dropped and recreated from the current schema. FTS5 virtual tables cannot be ALTERed, so a full drop/recreate is required.

WAL journal mode is enabled for better concurrent read performance.

## Key Operations

### index_doogat

`index_doogat(doogat: &ParsedDoogat) -> Result<()>`

Upserts a single doogat into all tables within a transaction:

1. Check if the doogat already exists (for FTS cleanup)
2. Delete old FTS entry if exists
3. `INSERT OR REPLACE` into `doogats`
4. Delete and re-insert `tags`, `fields`, `links`, `aliases`, `checkboxes`
5. Insert scalar frontmatter extras into `_ddb_fields` with `zone = 'Frontmatter'` (String, Number, Bool — skips List/Map)
6. Insert aliases from frontmatter `aliases` list (if present)
7. Delete and re-insert `attachments` from frontmatter `attachments` list (if present)
8. Insert checkbox items from `parsed.checkboxes` into `_ddb_checkboxes`
9. Insert new FTS entry

Uses a named `SAVEPOINT`/`RELEASE` pair (via `with_savepoint`) for atomic writes that nest correctly within SQL engine transactions.

### rebuild

`rebuild(repo: &impl DoogatSource) -> Result<RebuildReport>`

Full index rebuild using a parallel pipeline. Drops all tables (internal and materialized) and recreates the schema from scratch. The index is a disposable cache — no migration framework needed.

Phases:

1. **Drop & recreate** — drop every table (FK checks disabled for drop order), recreate internal schema from `SCHEMA_DDL`
2. **Parallel parse** — `parallel_parse()` reads all files from git sequentially via `read_files_batch()` (optimal for pack I/O), then parses in parallel using rayon `par_iter()`. Parse errors become warnings, not failures.
3. **Batch index** — `batch_index()` writes all parsed doogats to SQLite in a single `BEGIN IMMEDIATE`/`COMMIT` transaction. Per-doogat errors are logged and skipped.
4. **Consistency warnings** — detect malformed YAML, cross-zone duplicates, missing required fields
5. **Cached materialization** — `materialize_all_types_from()` creates typed SQLite tables using the already-parsed data (no redundant git reads). Schema inference and row population both filter the in-memory `Vec<ParsedDoogat>` by type.
6. Store current HEAD OID in `_ddb_meta` table

Full rebuild is only triggered by:
- Explicit `ddb reindex`
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

`incremental_reindex(repo: &impl DoogatSource, old_head: &str) -> Result<RebuildReport>`

Diffs `old_head` against the current HEAD and processes only changed files. Added or modified doogats are re-indexed; deleted doogats are removed. Falls back to full `rebuild` if the diff fails (e.g. old HEAD unreachable after gc).

When multiple files are changed (2+), uses `batch_index()` (single transaction) instead of per-doogat `index_doogat()` for better throughput.

This is the common path for keeping the index current after `git pull` or direct file edits — fast and non-destructive (no table drops).

### Integrity Check

`check_integrity() -> Result<bool>`

Runs `PRAGMA integrity_check` and verifies core tables exist (`doogats`, `_ddb_fts`, `_ddb_tags`, `_ddb_fields`, `_ddb_links`, `_ddb_aliases`, `_ddb_checkboxes`, `_ddb_meta`). Returns `false` if corrupt.

### Staleness Detection

`is_stale(repo) -> Result<bool>`

Compares the HEAD OID stored in `_ddb_meta` table against the current Git HEAD. If they differ, the index is stale and needs rebuilding.

`rebuild_if_stale(repo) -> Result<Option<RebuildReport>>`

Checks integrity first (force rebuild if corrupt), then staleness. Returns `None` if already current and healthy.

## Type Inference

### infer_schema

`infer_schema(type_name: &str, repo: &GitRepo) -> Result<TableSchema>`

Scans all doogats of a given type and infers a `TableSchema`:

- **Frontmatter** extra keys → frontmatter columns (inferred as INTEGER, REAL, BOOLEAN, or TEXT)
- **Body** `## headings` → body TEXT columns
- **Reference** `key:: value` fields → reference columns

Type widening: if any doogat of the type has a non-matching value for a field, the type widens (INTEGER+REAL → REAL, any mismatch → TEXT).

### merge_schemas

`merge_schemas(typedef: Option<TableSchema>, inferred: TableSchema) -> TableSchema`

Merges an explicit `_typedef` with inferred columns:
- Typedef columns take precedence (type, zone, required flags preserved)
- Inferred columns fill gaps (new fields not defined in typedef)

### materialize_all_types

`materialize_all_types(repo: &GitRepo) -> Result<(usize, Vec<String>)>`

For each distinct type in the index:
1. Load `_typedef` if it exists
2. Infer schema from data doogats
3. Merge schemas (typedef wins)
4. Create SQLite table and populate with data
5. Log advisory for inferred-only types

Also creates empty tables for typedef-only types with no data doogats.

## Consistency Warnings

`collect_consistency_warnings(repo: &GitRepo) -> Vec<ConsistencyWarning>`

Scans all doogats and produces advisory warnings:

```rust
pub enum ConsistencyWarning {
    MalformedYaml { path: String, error: String },
    CrossZoneDuplicate { path: String, key: String },
    MissingRequired { path: String, type_name: String, field: String },
}
```

Warnings don't prevent indexing — doogats are always indexed best-effort.

## Alias Resolution

### resolve_alias

`resolve_alias(name: &str) -> Result<Option<String>>`

Case-insensitive lookup in `_ddb_aliases`. Returns the doogat ID if found.

Aliases are populated from the frontmatter `aliases` list during `index_doogat()` and cleaned up on `remove_doogat()`.

### resolve_wikilink

`resolve_wikilink(target: &str) -> Result<Option<String>>`

Three-step resolution chain:

1. **Path lookup** — check if `target` matches a `doogats.path` directly
2. **ID lookup** — try `resolve_path(target)` (exact doogat ID match)
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
    pub updated_at: String,
    pub tags: Vec<String>,
    pub doogat_type: Option<String>,
    pub fields: Option<BTreeMap<String, String>>,
    pub created_at: Option<String>,
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

Find all doogat IDs that link to the given target path.

### query_raw

`query_raw(sql: &str) -> Result<Vec<Vec<String>>>`

Execute arbitrary SQL. Returns rows as string vectors. Handles all SQLite value types (null, integer, real, text, blob).

### Sequence Navigation

Doogats form ordered chains via the `sequence` frontmatter field (stored in `_ddb_fields`). No schema migration needed.

- `sequence_children(id)` — children sorted by ID (chronological). Queries `_ddb_fields` WHERE key='sequence' AND value=?id, JOINs doogats for title.
- `sequence_breadcrumb(id)` — walks parent chain to root via repeated `_ddb_fields` lookups. Returns root-to-self path. Cycle detection breaks after 100 iterations using a HashSet of visited IDs.
- `sequence_info(id)` — combines parent lookup, `sequence_children`, and `sequence_breadcrumb` into `SequenceInfo { parent, children, breadcrumb }`.
- `broken_sequences()` — LEFT JOIN `_ddb_fields` (key='sequence') against `doogats` to find references to non-existent parents.

MOC (Map of Content) doogats use `role: moc` in frontmatter and serve as natural sequence roots. The `role` field is indexed in `_ddb_fields` automatically — no special handling required.

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
- Materialization of typed tables from `_typedef` doogats
- Type inference (frontmatter types, body headings, reference fields, empty type, type widening)
- Schema merging (typedef-only, inferred-only, overlap, no overlap)
- Consistency warnings (valid doogat, missing required)
- Integration: full cycle with inferred type
- Integration: typedef + inferred merge
- Integration: external edit reconciliation
- Integration: consistency warnings in rebuild
- Sequence navigation: children, breadcrumb, info, broken detection, cycle safety
