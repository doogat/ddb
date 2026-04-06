# Module Structure

## Dependency Graph

```text
error (foundation — no adapter crate imports)
  │
  v
types (depends: error — no adapter crate imports)
  │
  v
traits (depends: error, types — defines DoogatSource, DoogatStore,
  │                               DoogatIndex, ConflictResolver, GitBackend)
  │
  ├──> parser (depends: error, types)
  │      │
  │      └──> crdt_resolver (depends: error, types, parser, traits)
  │
  ├──> git_ops (depends: error, types, traits — implements DoogatSource/Store/GitBackend)
  │      │
  │      ├──> indexer (depends: error, types, traits, parser, sql_engine
  │      │             — accepts &impl DoogatSource, implements DoogatIndex)
  │      │
  │      ├──> sql_engine (depends: error, types, parser, indexer
  │      │                — accepts &dyn DoogatStore)
  │      │
  │      ├──> sync_manager (depends: error, types, git_ops,
  │      │                           crdt_resolver, indexer)
  │      │
  │      └──> compaction (depends: error, types, git_ops,
  │                                sync_manager, maintenance)
  │
  ├──> maintenance (depends: error, types, git_ops
  │                          — git maintenance runner + auto-trigger)
  │
  ├──> consistency (depends: error, types, parser, indexer, traits
  │                          — detect/apply/migrate auto-fixes)
  │
  ├──> attachments (depends: error, types, git_ops, indexer, parser)
  │
  ├──> bundled_types (standalone, no deps)
  │
  ├──> hlc (depends: error — Hybrid Logical Clock, no external crates)
  │
  ├──> bundle (depends: error, types, git_ops, sync_manager
  │                      — air-gapped sync via tar archives)
  │
  ├──> nosql (depends: error, types — feature-gated redb key-value index)
  │
  ├──> service (depends: error, types, git_ops, indexer, parser,
  │              sql_engine, sync_manager, compaction, maintenance,
  │              consistency, attachments, bundle, nosql
  │              — unified orchestration layer for CLI/FFI/server)
  │
  ├──> ffi (depends: service — UniFFI DoogatDriver facade,
  │         delegates to DoogatService)
  │
  └──> CLI (depends: service — delegates to DoogatService)
```

## Module Summary

| Module | Purpose | Key Dependencies |
|--------|---------|-----------------|
| `error` | `DoogatError` enum + `Result<T>` alias | thiserror only |
| `types` | Domain types (directory module: `mod.rs` config types, `value.rs` Value enum/path utilities, `doogat.rs` domain model types, `schema.rs` schema/consistency types) | no adapter crates |
| `traits` | Core trait abstractions (DoogatSource, DoogatStore, DoogatIndex, ConflictResolver, GitBackend) | error, types |
| `parser` | Parse/serialize three-zone Markdown | regex, chrono, serde_yaml |
| `search_query` | Search query parsing and normalization to canonical form | — (std only) |
| `git_ops` | Git repository CRUD + merge; implements DoogatSource/Store/GitBackend (directory module: `mod.rs` struct/init/CRUD/traits, `read.rs` file reads/diffs/revision queries, `merge.rs` merge/conflict resolution, `remote.rs` push/pull/fetch, `rename.rs` rename with backlink rewrite) | git2 |
| `crdt_resolver` | Automerge conflict resolution; implements ConflictResolver | automerge, similar |
| `indexer` | SQLite FTS5 index (directory module: `mod.rs` core CRUD/schema, `search.rs` FTS5 search/tag queries/filters, `rebuild.rs` rebuild/reindex/staleness, `graph.rs` backlinks/discovery/sequences, `resolve.rs` path/alias/wikilink resolution, `materialize.rs` schema inference/table materialization); implements DoogatIndex | rusqlite |
| `sql_engine` | SQL DDL/DML → doogat CRUD, _typedef management (directory module: `mod.rs` dispatch, `ddl.rs` CREATE/ALTER/DROP, `dml.rs` INSERT/UPDATE/DELETE, `junction.rs` junction tables, `builders.rs` doogat/schema building, `helpers.rs` SQL parsing utilities, `transaction.rs` BEGIN/COMMIT/ROLLBACK) | sqlparser, rusqlite |
| `bundled_types` | Built-in _typedef templates (project, contact) | — |
| `sync_manager` | Multi-device sync orchestration | uuid, toml, chrono |
| `compaction` | CRDT cleanup + git gc | — |
| `maintenance` | Git maintenance runner, auto-trigger | — |
| `consistency` | Detect/apply/migrate auto-fixes (directory module: `mod.rs` detection/fix application, `migrations.rs` versioned data migrations, `zone_migrate.rs` cross-zone field migration) | — |
| `attachments` | File attachment CRUD (attach, detach, list) on `reference/{id}/` | — |
| `hlc` | Hybrid Logical Clock for causal ordering | — (std only) |
| `bundle` | Air-gapped sync via tar archive export/import | tar, sha2, flate2 |
| `nosql` | redb-based key-value index for fast lookups (feature-gated) | redb |
| `service` | Unified orchestration layer (DoogatService) — single entry point for CRUD, search, SQL, sync, discovery with consistent NoSQL dual-write (directory module: `mod.rs` struct/constructors/state, `crud.rs` create/read/update/delete/batch ops, `search.rs` search/filtered queries, `sql.rs` SQL pass-through/transactions, `ops.rs` sync/compact/maintenance/bundles, `discovery.rs` unlinked mentions/sequences/backlinks, `utility.rs` schema queries/attachments/NoSQL reads) | all core modules |
| `ffi` | UniFFI facade (DoogatDriver) wrapping `Mutex<DoogatService>` for Swift/Kotlin | service, uniffi |
| **CLI** | Command-line interface (main.rs CLI structs/dispatch, `commands/` submodules: crud, query, sync, maintenance, discover) | service, clap |
| **updater** (CLI) | Self-update from GitHub releases | reqwest, semver, self_replace, sha2, flate2, tar |

## External Dependencies

### Core (`ddb-core`)

| Crate | Version | Purpose |
|-------|---------|---------|
| `automerge` | 0.7 | CRDT conflict resolution |
| `chrono` | 0.4 | Timestamps and date formatting |
| `git2` | 0.20 | libgit2 bindings for Git operations |
| `regex` | 1 | Inline field and wikilink extraction |
| `rusqlite` | 0.32 | SQLite with FTS5 (bundled) |
| `serde` | 1 | Serialization framework |
| `serde_yaml` | 0.9 | YAML frontmatter parsing |
| `similar` | 2 | Character-level text diffs |
| `sqlparser` | 0.55 | SQL statement parsing (DDL/DML) |
| `thiserror` | 2 | Error derive macros |
| `toml` | 0.8 | Node config serialization |
| `uniffi` | 0.29 | Cross-platform FFI bindings (Swift/Kotlin) |
| `uuid` | 1 | Node UUID generation (v4) |

### CLI (`ddb-cli`)

| Crate | Version | Purpose |
|-------|---------|---------|
| `chrono` | 0.4 | Date formatting for new doogats |
| `clap` | 4 | Argument parsing with derive |
| `flate2` | 1 | Gzip decompression for update archives |
| `reqwest` | 0.12 | HTTP client for GitHub releases API |
| `self_replace` | 1 | Atomic binary self-replacement |
| `semver` | 1 | Version comparison for updates |
| `serde` | 1 | State file serialization |
| `serde_json` | 1 | JSON state file format |
| `sha2` | 0.10 | SHA-256 checksum verification |
| `tar` | 0.4 | Archive extraction for update binaries |
| `ddb-core` | path | Local workspace dependency |

### Dev

| Crate | Version | Purpose |
|-------|---------|---------|
| `criterion` | 0.5 | Benchmarking framework (CRUD + search) |
| `tempfile` | 3 | Temporary directories for integration tests |
