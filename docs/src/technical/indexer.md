# Search Index

**Source**: `ddb-core/src/indexer/` (directory module: mod.rs, filter.rs, graph.rs, materialize.rs, rebuild.rs, resolve.rs, search.rs)

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

After indexing, typed data doogats are also materialized into their type tables (and removed from them on delete) so search filters that go through path (a) or FK routes match newly-indexed rows immediately, without requiring a full `ddb reindex`. Schema lookups are cached per type for the duration of the call. Per-doogat materialization failures are logged and skipped — incremental indexing stays best-effort.

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

### unique_together constraints

Typedefs can declare composite unique constraints via the `unique_together` frontmatter field. The value is a list of column-name lists:

```yaml
unique_together:
  - - code
  - - tenant_id
    - email
```

During materialization (`drop_and_create_materialized_table`), each group generates a `CREATE UNIQUE INDEX` on the materialized type table. These indexes enforce uniqueness at the SQLite level for direct SQL inserts.

For GraphQL mutations, the `batch_create` service method performs a pre-check before writing: it queries the `_ddb_fields` key-value index to detect existing doogats matching the constrained columns. When a match is found, the behavior depends on the `ConflictAction`:

- `Error` (default): returns a validation error
- `Ignore`: skips creation and returns the existing doogat

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

### Search where filter resolution

`build_filter_clauses` resolves `SearchFieldFilter` entries against the best available data source:

1. **Tag**: `field == "tag"` routes to `_ddb_tags` (exact match via `eq`, substring via `contains`, or set membership via `in`).
2. **Core column**: if the field matches a core `doogats` column (e.g. `type`, `title`, `date`), resolves directly against that column.
3. **Materialized columns**: introspects candidate type tables via `PRAGMA table_info`. When `types` filter is set, only those tables are checked. When a field exists in multiple type tables, subqueries are UNIONed. For `Eq` and `In`, compares the raw column value directly. For `Contains` on a REFERENCES-backed column, resolution priority is: (a) if a same-named typedef table exists (convention: `<col>` matches the referenced typedef name), resolve `<val>` against that table's *search-key column* with LIKE — see PRD 00133; (b) else if a junction table `{type}_{field}` exists, JOIN through it on referenced doogat title; (c) else direct LIKE on the materialized column. Path (a) is preferred because the SQL INSERT path populates the materialized column but does not auto-populate the junction table (only full rebuilds do), so freshly-inserted data with no rebuild never matches via path (b).
4. **FK routes (no direct column match)**: when no candidate type table has a column literally named `<field>`, the resolver scans the SQLite catalog for routes that reach a referenced typedef via the `<field>_id` convention. Three shapes are tried, all UNIONed when present (see ddb#15 follow-up):
   - **Self-route** — the type table itself carries `<field>_id` (e.g. `link.category_id` REFERENCES `category(id)`).
   - **Auto-junction** — the materializer-generated `{type}_{field}` table with `{type}_id` + `{field}_id`.
   - **User-junction** — any other materialized table (typically a typedef-defined membership table such as jink's `category-membership`) that carries both `<type>_id` and `<field>_id` (or bare `<field>`). Detection is purely by column shape, not table name.
   `Eq`/`In` compare the raw FK column value to the user-supplied id; `Contains` JOINs to the typed referenced table on its *search-key column* (default `title`, configurable via `ALTER TABLE … SET SEARCH KEY`) — falls back to `doogats.title` when the typedef table is absent.
5. **`_ddb_fields` fallback**: if no FK routes match either, falls back to the generic key-value store. Uses two SQL parameters (key + value).

#### Search key override

Each typedef defaults to `title` as the column substring-matched in path (a) and FK-route Contains. Authors can override that default with `ALTER TABLE <name> SET SEARCH KEY <col>` (and `DROP SEARCH KEY` to reset to the default). The chosen column must exist on the typedef and must not be a `REFERENCES` column. The change persists in the typedef YAML (`search_key: <col>` frontmatter) and in `_ddb_meta(key='search_key:<type>')` for fast filter-time lookup. `refresh_boost_table` re-emits these meta rows on every full rebuild, so a hand-edited typedef YAML reflects in the next rebuild without manual intervention.

Common case: a typedef carrying both a human-readable `title` and a machine-readable canonical identifier (e.g. jink's `category` typedef where `title="Portals"` and `fqn="work.portals"`). Setting `SET SEARCH KEY fqn` makes `category=work.portals` resolve to the row regardless of its title.

#### SearchFieldOp

Each `SearchFieldFilter` carries a `SearchFieldOp` discriminating the comparison:

- `Eq(String)` - exact match (`= ?`)
- `Contains(String)` - substring match (`LIKE '%?%'`)
- `In(Vec<String>)` - set membership (`IN (?, ?, ...)`). An empty vec produces an `AND 0` clause, returning no results.

All five resolution paths (tag, core column, materialized column, junction table, `_ddb_fields` fallback) support all three operators.

This method is an instance method (`&self`) because it needs `self.conn` for type table introspection.

#### Junction table traversal

Junction table traversal is triggered in two situations:

1. The where-filter field does not exist as a materialized scalar column on any candidate type table.
2. The field exists as a materialized column but the op is `Contains` - in this case the materialized column stores raw IDs, not titles, so a JOIN through the junction table is required to match by referenced doogat title.

Auto-junction tables are named `{type}_{field}` and are created during materialization for REFERENCES columns (see `junction_table_ddl`).

In situation (1), the resolver collects **FK routes** (see `collect_fk_routes` in `indexer/filter.rs`). A route is one of three shapes:

- **Self-route**: the type table itself has a `<field>_id` column. SQL uses `table = "<type>"`, `type_id_col = "id"`, `fk_col = "<field>_id"`.
- **Auto-junction**: as above. SQL uses `table = "<type>_<field>"`, `type_id_col = "<type>_id"`, `fk_col = "<field>_id"`.
- **User-junction**: any other materialized table that carries both `<type>_id` and `<field>_id` (or bare `<field>`). Covers user-defined membership tables whose name does not follow the auto-junction convention. SQL uses `table = "<user table>"`, `type_id_col = "<type>_id"`, `fk_col = "<field>_id"` (or `"<field>"`). Detection is by column shape only; the table name is irrelevant.

Each route generates a subquery, all UNIONed:

- **Eq**: `SELECT "<type_id_col>" FROM "<table>" WHERE "<fk_col>" = ?` - direct FK match
- **Contains**: `SELECT jt."<type_id_col>" FROM "<table>" jt JOIN doogats d ON d.id = jt."<fk_col>" WHERE d.title LIKE '%' || ? || '%'` - joins to doogats to match by referenced doogat title
- **In**: `SELECT "<type_id_col>" FROM "<table>" WHERE "<fk_col>" IN (?, ?, ...)` - FK set membership

When multiple types or multiple membership tables produce routes for the same field, all subqueries are UNIONed with a shared parameter placeholder.

If no FK routes are found, the filter falls through to the `_ddb_fields` key-value fallback.

### Search query language

The search query language supports the following constructs:

- **Bare words**: `meeting`, `rust` - matched against FTS5 index
- **Quoted strings**: `"conflict resolution"` - exact phrase match
- **Field filters**: `tag=svelte`, `category:work.portals` - both `=` and `:` syntax, with optional quoted values (`title:"meeting minutes"`). For non-tag fields, in-query `field=value` extracts as `Contains` (substring/LIKE match). The `tag=value` form keeps exact-match. The explicit `where: { field, eq }` API filter remains exact-match for all fields. This asymmetry is intentional — in-query syntax is the user-facing convenience form; explicit `where` filters are programmatic precision (PRD 00133).
- **Boolean operators**: `AND`, `OR`, `NOT` (case-insensitive)
- **Implicit AND**: `meeting minutes` is equivalent to `meeting AND minutes`
- **Parentheses**: `(a OR b) AND c` - override default precedence
- **Operator precedence** (high to low): NOT, AND, OR

Note: field filters (`tag=svelte`) are a normalization-layer concept used for canonical comparison and saved searches. The FTS5 index only sees the bare word and phrase portions of the query.

### FTS negation handling

FTS5's MATCH operator cannot handle standalone negation (`NOT term` is a syntax error, `a NOT b` requires a positive term). The search engine transparently works around this limitation by splitting queries that contain NOT expressions into a compound SQL query.

**How it works**: `search_query::extract_negations()` partitions the parsed AST into positive and negative parts at the top-level AND boundary. The positive part goes to FTS5 MATCH for ranking and snippets. Each negated term becomes a `NOT IN` subquery:

```sql
-- "important NOT meeting" becomes:
WHERE _ddb_fts MATCH 'important'
AND z.id NOT IN (
  SELECT z2.id FROM _ddb_fts
  JOIN doogats z2 ON z2.rowid = _ddb_fts.rowid
  WHERE _ddb_fts MATCH 'meeting'
)
```

**Negated tag filters** (`NOT tag=archive`) use `_ddb_tags` instead of FTS:

```sql
AND z.id NOT IN (SELECT doogat_id FROM _ddb_tags WHERE tag = ?)
```

**All-negative queries** (`NOT archive`) have no positive MATCH clause, so the search scans `doogats` directly with exclusion subqueries, ordered by title instead of relevance rank. No snippets are returned.

Ranking is always based on the positive terms only - negated terms affect filtering but not result ordering.

### Query normalization

`search_query::normalize(query: &str) -> String`

Parses a search query into an AST and serializes it to a canonical form where semantically equivalent queries produce the same string. Used by the GraphQL server's `queryNormalized` and `normalizeSearchQuery` surfaces.

Normalization rules: lowercase all terms, collapse whitespace, make implicit AND explicit, sort AND operands alphabetically, preserve OR order, normalize recursively inside NOT and parentheses. On parse failure, falls back to lowercase + whitespace collapse.

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
