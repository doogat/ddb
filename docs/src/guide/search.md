# Search & Queries

## Full-Text Search

```bash
ddb search "your query"
```

Searches doogat titles, bodies, and tags using SQLite FTS5 with porter stemming. Results are ranked by relevance with highlighted snippets.

### Query syntax

FTS5's full query syntax is available:

| Syntax | Example | Meaning |
|--------|---------|---------|
| Terms | `rust crdt` | Both terms must appear (implicit AND) |
| AND | `rust AND crdt` | Both terms must appear |
| OR | `rust OR golang` | Either term matches |
| NOT | `rust NOT draft` | Exclude doogats containing a term |
| Phrase | `"conflict resolution"` | Exact phrase match |

These operators work in both the CLI and the GraphQL `search` query.

Malformed queries (e.g., `AND AND`) return an error rather than silently failing.

### Example

```bash
ddb search "conflict resolution"
```

Output:

```text
[20260226120000] CRDT Conflict Resolution (ddb/20260226120000.md)
  CRDTs resolve <b>conflict</b>s by ensuring all replicas converge to the same state...
```

The index is automatically rebuilt if stale (Git HEAD has changed since last rebuild).

## Raw SQL Queries

```bash
ddb query "SQL"
```

Execute arbitrary SQL against the index database. Useful for advanced queries combining multiple tables.

### Available Tables

| Table | Columns |
|-------|---------|
| `doogats` | `id`, `title`, `date`, `type`, `path`, `body`, `updated_at` |
| `_ddb_tags` | `doogat_id`, `tag`, `source` |
| `_ddb_fields` | `doogat_id`, `key`, `value`, `zone` |
| `_ddb_links` | `source_id`, `target_path`, `display`, `zone`, `kind` |

> The underscore-prefixed `_ddb_*` tables are internal index tables and may change between releases. The `doogats` table and the materialized per-type tables are the stable query surface.

### Examples

List all doogats:

```bash
ddb query "SELECT id, title FROM doogats ORDER BY date DESC"
```

Find doogats by tag:

```bash
ddb query "SELECT z.id, z.title FROM doogats z JOIN _ddb_tags t ON t.doogat_id = z.id WHERE t.tag = 'crdt'"
```

Find backlinks to a doogat:

```bash
ddb query "SELECT z.title FROM doogats z JOIN _ddb_links l ON l.source_id = z.id WHERE l.target_path = '20260226120000'"
```

Find doogats with a specific inline field:

```bash
ddb query "SELECT z.title, f.value FROM doogats z JOIN _ddb_fields f ON f.doogat_id = z.id WHERE f.key = 'source'"
```

Count doogats by type:

```bash
ddb query "SELECT type, COUNT(*) FROM doogats GROUP BY type"
```

## Rebuilding the Index

```bash
ddb reindex
```

Forces a full rebuild — parses every doogat and repopulates all tables. The index is derived from Git; it can be safely deleted and rebuilt.
