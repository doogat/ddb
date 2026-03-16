# Module Structure

## Dependency Graph

```text
error (foundation — no adapter crate imports)
  │
  v
types (depends: error — no adapter crate imports)
  │
  v
traits (depends: error, types — defines ZettelSource, ZettelStore,
  │                               ZettelIndex, ConflictResolver)
  │
  ├──> parser (depends: error, types)
  │      │
  │      └──> crdt_resolver (depends: error, types, parser, traits)
  │
  ├──> git_ops (depends: error, types, traits — implements ZettelSource/Store)
  │      │
  │      ├──> indexer (depends: error, types, traits, parser, sql_engine
  │      │             — accepts &impl ZettelSource, implements ZettelIndex)
  │      │
  │      ├──> sql_engine (depends: error, types, parser, indexer
  │      │                — accepts &dyn ZettelStore)
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
  ├──> ffi (depends: error, types, git_ops, indexer, parser,
  │         sync_manager, compaction, maintenance — UniFFI ZettelDriver facade)
  │
  └──> CLI (depends: all core modules)
```

## Module Summary

| Module | Purpose | Key Dependencies |
|--------|---------|-----------------|
| `error` | `ZettelError` enum + `Result<T>` alias | thiserror only |
| `types` | Domain types (CommitHash, Value, ParsedZettel, TableSchema) | no adapter crates |
| `traits` | Core trait abstractions (ZettelSource, ZettelStore, ZettelIndex, ConflictResolver) | error, types |
| `parser` | Parse/serialize three-zone Markdown | regex, chrono, serde_yaml |
| `git_ops` | Git repository CRUD + merge; implements ZettelSource/Store | git2 |
| `crdt_resolver` | Automerge conflict resolution; implements ConflictResolver | automerge, similar |
| `indexer` | SQLite FTS5 index, type inference, materialization; implements ZettelIndex | rusqlite |
| `sql_engine` | SQL DDL/DML → zettel CRUD, _typedef management | sqlparser, rusqlite |
| `bundled_types` | Built-in _typedef templates (project, contact) | — |
| `sync_manager` | Multi-device sync orchestration | uuid, toml, chrono |
| `compaction` | CRDT cleanup + git gc | — |
| `maintenance` | Git maintenance runner, auto-trigger | — |
| `consistency` | Detect/apply/migrate auto-fixes (tags, titles, keys, types) | — |
| `attachments` | File attachment CRUD (attach, detach, list) on `reference/{id}/` | — |
| `ffi` | UniFFI facade (ZettelDriver) for Swift/Kotlin bindings | uniffi |
| **CLI** | Command-line interface | clap |
| **updater** (CLI) | Self-update from GitHub releases | reqwest, semver, self_replace, sha2, flate2, tar |

## External Dependencies

### Core (`zdb-core`)

| Crate | Version | Purpose |
|-------|---------|---------|
| `automerge` | 0.7 | CRDT conflict resolution |
| `chrono` | 0.4 | Timestamps and date formatting |
| `git2` | 0.19 | libgit2 bindings for Git operations |
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

### CLI (`zdb-cli`)

| Crate | Version | Purpose |
|-------|---------|---------|
| `chrono` | 0.4 | Date formatting for new zettels |
| `clap` | 4 | Argument parsing with derive |
| `flate2` | 1 | Gzip decompression for update archives |
| `reqwest` | 0.12 | HTTP client for GitHub releases API |
| `self_replace` | 1 | Atomic binary self-replacement |
| `semver` | 1 | Version comparison for updates |
| `serde` | 1 | State file serialization |
| `serde_json` | 1 | JSON state file format |
| `sha2` | 0.10 | SHA-256 checksum verification |
| `tar` | 0.4 | Archive extraction for update binaries |
| `zdb-core` | path | Local workspace dependency |

### Dev

| Crate | Version | Purpose |
|-------|---------|---------|
| `criterion` | 0.5 | Benchmarking framework (CRUD + search) |
| `tempfile` | 3 | Temporary directories for integration tests |
