# Error Handling

**Source**: `ddb-core/src/error.rs`

## DoogatError

A unified error enum using `thiserror` for all fallible operations. All variants use `String` payloads — adapter-specific error types are converted at module boundaries via `From` impls in each adapter module:

```rust
pub enum DoogatError {
    Git(String),           // Git operations (from git2::Error in git_ops/mod.rs)
    Yaml(String),          // YAML parsing (from serde_yaml::Error in parser/mod.rs)
    Sql(String),           // SQLite queries (from rusqlite::Error in indexer/mod.rs)
    Automerge(String),     // CRDT operations (from AutomergeError in crdt_resolver.rs)
    Io(std::io::Error),    // File I/O
    Toml(String),          // TOML parsing (from toml::de::Error in sync_manager/mod.rs)
    Parse(String),         // Generic parse failures
    NotFound(String),      // File/ref not found
    Validation(String),    // Cross-zone duplicate fields, invalid data
    InvalidPath(String),   // Non-UTF-8 or invalid file paths (from bundle.rs path handling)
    SqlEngine(String),     // SQL engine translation errors
    Conflict(String),      // Merge conflicts requiring manual resolution
    Sync(String),          // Multi-device sync failures
    Index(String),         // Index corruption or rebuild failures
    BadRequest(String),    // Invalid API request parameters
    VersionMismatch { repo: u32, driver: u32 },  // Repo format ahead of driver version
    Redb(String),          // NoSQL index errors (feature-gated: nosql)
}
```

## Result Type

```rust
pub type Result<T> = std::result::Result<T, DoogatError>;
```

All public functions in `ddb-core` return `Result<T>`.

## Conversion

External error types convert via `From` impls in their respective adapter modules (not in error.rs):
- `git2::Error` → `DoogatError::Git` (in `git_ops/mod.rs`)
- `serde_yaml::Error` → `DoogatError::Yaml` (in `parser/mod.rs`)
- `rusqlite::Error` → `DoogatError::Sql` (in `indexer/mod.rs`)
- `automerge::AutomergeError` → `DoogatError::Automerge` (in `crdt_resolver.rs`)
- `toml::de::Error` → `DoogatError::Toml` (in `sync_manager/mod.rs`)
- `std::io::Error` → `DoogatError::Io` (via `#[from]` in error.rs)

This keeps `error.rs` free of adapter crate imports — it depends only on `thiserror` and `std::io`.

Application-level errors use:
- `DoogatError::Parse(msg)` for parsing failures
- `DoogatError::NotFound(path)` for missing files or references
- `DoogatError::Validation(msg)` for data integrity issues (e.g., cross-zone duplicate inline fields)
- `DoogatError::InvalidPath(msg)` for non-UTF-8 or invalid file paths
- `DoogatError::SqlEngine(msg)` for SQL translation errors
- `DoogatError::Conflict(msg)` for merge conflicts requiring manual resolution
- `DoogatError::Sync(msg)` for multi-device sync failures
- `DoogatError::Index(msg)` for index corruption or rebuild failures
- `DoogatError::BadRequest(msg)` for invalid API request parameters
- `DoogatError::VersionMismatch { repo, driver }` for repo format version ahead of driver

## CLI Error Handling

The CLI's `main()` function calls `run(cli)` which returns `Result<()>`. On error, it prints `"error: {e}"` to stderr and exits with code 1.

## Structured Logging

Uses `tracing` (library) + `tracing-subscriber` (CLI) for structured observability.

### Configuration

- `--log-dir <path>` or `DDB_LOG_DIR=<path>` — write NDJSON logs to `{dir}/ddb-{date}.ndjson`
- Without `--log-dir` — stderr with `RUST_LOG` env filter (default: `info` for ddb crates, `warn` for dependencies)
- `--log-level <level>` or `DDB_LOG_LEVEL=<level>` — set log level for ddb crates (`RUST_LOG` takes precedence)

### NDJSON Format

```json
{"timestamp":"...","level":"INFO","target":"ddb_core::sync_manager","fields":{"remote":"origin","branch":"master","message":"sync_start"}}
```

### Instrumented Events

| Module | Event | Level |
|---|---|---|
| sync_manager | `sync_start`, `fetch_complete`, `push_complete` | info/debug |
| sync_manager | `merge_result` (up-to-date/conflicts) | info |
| sync_manager | `delete_edit_resolved`, `cascade_step2_crdt` | info/debug |
| sync_manager | CRDT invalid/failed fallback to LWW | warn |
| compaction | `shared_head_computed`, `crdt_temp_cleanup`, `gc_result` | info/debug |
| indexer | `rebuild_triggered`, `rebuild_complete`, `corruption_detected` | info/warn |
| git_ops | `repo_opened`, orphan cleanup | debug/warn |
