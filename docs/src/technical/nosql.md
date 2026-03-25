# NoSQL Index (redb)

Doogat DB includes an optional redb-based key-value index behind the `nosql` feature flag. It complements SQLite (which provides FTS5 full-text search and SQL queries) with fast O(1) key lookups and prefix scans.

## When to use

- **redb**: fast single-doogat lookups by ID, type, or tag; backlink traversal; mobile/embedded scenarios where SQLite overhead is unnecessary
- **SQLite**: full-text search, complex SQL queries, materialized type tables

## Build

```bash
cargo build -p ddb-core --features nosql
```

## Table design

| Table | Key | Value | Purpose |
|-------|-----|-------|---------|
| `doogats` | doogat ID | JSON-serialized `ParsedDoogat` | Primary store |
| `by_type` | `{type}/{id}` | empty | Type index for prefix scan |
| `by_tag` | `{tag}/{id}` | empty | Tag index for prefix scan |
| `links` | `{target_id}/{source_id}` | empty | Backlink index |

Secondary tables use composite string keys with "/" separator. Prefix scans on `{prefix}/` efficiently return all matching IDs.

## API

```rust
use ddb_core::nosql::RedbIndex;

let idx = RedbIndex::open(Path::new(".ddb/index.redb"))?;

// Index a doogat (upsert — cleans old secondary entries first)
idx.index_doogat(&parsed_doogat)?;

// Single lookup
let doogat = idx.get("20240101120000")?;

// Prefix scans
let project_ids = idx.scan_by_type("project")?;
let rust_ids = idx.scan_by_tag("rust")?;
let backlink_ids = idx.backlinks("20240102000000")?;

// Remove
idx.remove_doogat("20240101120000")?;

// Full rebuild from git
let count = idx.rebuild(&git_repo)?;
```

## Serialization

Values use JSON (`serde_json`) rather than bincode. This avoids compatibility issues with Doogat DB's polymorphic deserializers (e.g., `DoogatId` accepts both string and integer formats from YAML frontmatter).

## Server Integration

The GraphQL server (`ddb-server`) enables `nosql` by default and provides:

- **Dual-write**: every create/update/delete that touches SQLite also writes to redb. The actor holds an `Option<RedbIndex>` alongside `Index`.
- **REST endpoints** at `/nosql/`:
  - `GET /nosql/:id` — get doogat by ID (O(1) lookup)
  - `GET /nosql?type=<type>` — scan by type prefix
  - `GET /nosql?tag=<tag>` — scan by tag prefix
  - `GET /nosql/:id/backlinks` — backlinks for a doogat

## CLI Integration

The CLI (`ddb-cli`) also enables `nosql` by default:

```bash
ddb get <id>              # fetch doogat by ID via redb
ddb scan --type project   # prefix scan by type
ddb scan --tag rust       # prefix scan by tag
ddb backlinks <id>        # list backlinks
```

CLI NoSQL commands rebuild the redb index on each invocation to ensure consistency with git. The server rebuilds once at startup and keeps in sync via dual-writes.

## Implementation

`ddb-core/src/nosql.rs` — gated behind `#[cfg(feature = "nosql")]`. All operations are single-transaction for consistency. Upserts clean old secondary index entries before re-inserting to handle type/tag/link changes.
