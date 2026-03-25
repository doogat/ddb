# Search & Queries

## Full-Text Search

```bash
ddb search "your query"
```

Searches doogat titles, bodies, and tags using SQLite FTS5 with porter stemming. Results are ranked by relevance with highlighted snippets.

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
| `tags` | `doogat_id`, `tag` |
| `fields` | `doogat_id`, `key`, `value`, `zone` |
| `links` | `source_id`, `target_path`, `display`, `zone` |

### Examples

List all doogats:

```bash
ddb query "SELECT id, title FROM doogats ORDER BY date DESC"
```

Find doogats by tag:

```bash
ddb query "SELECT z.id, z.title FROM doogats z JOIN tags t ON t.doogat_id = z.id WHERE t.tag = 'crdt'"
```

Find backlinks to a doogat:

```bash
ddb query "SELECT z.title FROM doogats z JOIN links l ON l.source_id = z.id WHERE l.target_path = '20260226120000'"
```

Find doogats with a specific inline field:

```bash
ddb query "SELECT z.title, f.value FROM doogats z JOIN fields f ON f.doogat_id = z.id WHERE f.key = 'source'"
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
