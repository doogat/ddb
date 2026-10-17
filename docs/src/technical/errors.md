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

## Structured Error Codes

`DoogatError::Structured { code, message, context }` carries a stable `code` plus
typed `context` entries that propagate to every transport (GraphQL `extensions.code`,
REST error envelope, FFI `DdbError::{Validation, SqlEngine}::code`). See
`ddb-core/src/error.rs::codes` for the full list.

`TRANSACTION_NOT_SUPPORTED` identifies transaction control that would retain
state across requests on a shared server service. It is an `InvalidInput`
application error with user-safe guidance, `verb`, and `atomic_unit` context.
Use GraphQL `executeBatch` for atomic DML within one request. Exclusive embedded
services retain their transaction API and raw SQL transaction support.

### SINGLETON_VIOLATION `existing_id` sentinel

`SINGLETON_VIOLATION` errors carry `context.existing_id` so clients can render a
link or fetch the blocking row. In the **intra-batch** case — a single
multi-row `INSERT` (or `batch_create`) into an empty `SINGLETON` typedef — there
is no committed blocking row yet, so the field uses the literal sentinel
`<intra-batch>` instead of an id. Clients that resolve `existing_id` to a row
must treat this value as "no fetchable target" rather than passing it back as an id.

Layer 1 (`service/batch.rs::batch_create`) emits this from the `seen_singleton`
HashMap tracker; Layer 2 (`sql_engine/dml.rs::handle_insert`) emits it when
`rows.len() > 1` against an empty SINGLETON table. Both share the same
`<intra-batch>` literal so the contract is uniform.

## Unified Transport Rendering Policy (PRD 00173)

`ddb_core::app_contract::error_rendering(AppError) -> ErrorRendering` is the
**single** code → (HTTP status, redaction, FFI hint) policy. Every server
transport derives its output from it and must never re-decide redaction or
status itself:

```rust
pub struct ErrorRendering {
    pub code: &'static str,
    pub message: String,       // redacted for Internal-category errors
    pub http_status: u16,
    pub category: AppErrorCategory,
    pub field: Option<String>,
    pub details: Vec<(String, AppErrorDetail)>,
    pub ffi_variant: FfiVariant,
}
```

- `GraphQL` (`to_graphql_error_from_app`), `REST` and `NoSQL HTTP`
  (`http_error_response`, shared by both) all call `error_rendering` and
  encode its fields — none of them decide a status code or a redaction rule
  independently any more.
- `AppError.message` is `DoogatError::to_string()`, which for every simple
  variant carries a thiserror `Display` prefix (`"sql engine: "`,
  `"not found: "`, `"validation: "`, `"invalid path: "`, `"conflict: "`,
  `"bad request: "`, `"parse: "`). `error_rendering` strips it once, centrally,
  before any transport sees the message.
- `DoogatError::SqlEngine` now converts to `AppError` with code `SQL_ERROR`
  (category `InvalidInput`) — `SQL_ERROR` is a first-class `codes::SQL_ERROR`
  constant, not a wire-only string. SQL syntax errors render identically on
  GraphQL and REST (`SQL_ERROR` + message, never redacted).
- HTTP status is per-code, not strictly per-category: `SQL_ERROR`,
  `NOT_NULL_VIOLATION`, `UNKNOWN_FIELD`, and `TYPE_NOT_REGISTERED` stay `422`
  while `VALIDATION_ERROR`, `INVALID_PATH`, and `BAD_REQUEST` are `400`, even
  though all are `InvalidInput` category. This is deliberate — it preserves
  every status REST served before unification — and must not be "fixed" into
  a single per-category status without a compatibility review.
- `Internal`-category errors are logged once, centrally, inside
  `error_rendering` via `tracing::error!(code, message, "internal error")`
  with the unredacted message, before the message is replaced with the
  literal `"internal error"` and `field`/`details` are dropped. Transports
  never log this themselves.
- `FfiVariant` (`NotFound` / `Validation` / `Conflict` / `Internal`) is a
  core-only, adapter-neutral hint derived 1:1 from `AppErrorCategory`. It
  exists so a future FFI rewrite can replace the lossy `DoogatError -> DdbError`
  allowlist in `ddb-core/src/ffi/records.rs` with a mapping from this enum,
  and so a future PgWire rewrite can consume `ErrorRendering.message`/`.code`
  directly instead of `Display`-wrapping raw errors. Neither FFI nor PgWire
  consumes it yet — this PRD only defines and unit-tests the surface.
- Cross-transport parity is pinned by `ddb-server/tests/error_parity.rs`,
  which drives a representative code set through all three transports and
  asserts identical code + redaction, plus the documented per-code status.

**Compatibility note:** several response shapes changed when this policy
landed. GraphQL SQL errors are no longer redacted to `INTERNAL_ERROR`
— they now render `SQL_ERROR` with the SQL engine's message, matching REST.
REST `SCHEMA_APPLY_PARTIAL` and any other `Internal`-category structured code
now redacts its message to `"internal error"` instead of passing the raw
detail through — matching GraphQL's pre-existing redaction; its 500 status is
unchanged. Two statuses did change. A structured code absent from
`ERROR_POLICY` classifies `Internal`, so it now renders HTTP 500 with its
message redacted to `"internal error"` and its `field`/`details` dropped,
where before REST gave it 422 with its own message. `DoogatError::Parse` is
now the registered `PARSE_ERROR` row (`InvalidInput`, 422) and exposes its
message with the `"parse: "` prefix stripped, where before it rendered
`INTERNAL_ERROR` with HTTP 500 and a redacted message. The legacy GraphQL
path (`to_graphql_error`) routes through the same policy, so both redaction
changes reach it: an unregistered structured code is redacted there too (its
code is kept, its `context` no longer attached), and `Parse` is exposed as
`PARSE_ERROR`.

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
| indexer | typedef file read / parse / schema extraction failed during materialization | warn |
| service | row dropped in `list_doogats_filtered` due to file read or parse failure | warn |
| indexer | row dropped in `fetch_materialized_fields` due to query or deserialization failure | warn |
| git_ops | `repo_opened`, orphan cleanup | debug/warn |

## Behavior Changes (PRD 00140)

### Typed-update structured-value rejection

`DoogatService::update_doogat` rejects `Value::List` or `Value::Map` in the `fields`
map when the target column is declared in a typedef schema. Returns
`DoogatError::Validation`. Untyped doogats and undeclared extra fields are
unaffected.

### Counter-seed and FK query errors now propagate

`prepare_typed_insert_validate` previously swallowed SQLite errors from counter-seeding
queries (`NEXT`, `NEXT(partition)`) and FK existence checks via `unwrap_or(0)` /
`unwrap_or(false)`. These now return `DoogatError::Sql` so callers see the real failure
instead of a silently wrong counter value or a false-positive `REFERENCES_VIOLATION`.

### FFI mutex poison maps to DdbError instead of panic

All `DoogatDriver` FFI methods acquire the service lock via `with_service` /
`with_service_mut` helpers. A poisoned mutex (thread panic while lock was held) now
returns `DdbError::Io { msg: "service lock poisoned: ..." }` instead of unwinding
through the FFI boundary.

### ID-generation mutex poison recovery

`generate_unique_id` uses `LAST.lock().unwrap_or_else(|e| e.into_inner())` to recover
from a poisoned static mutex. The inner `String` value is still valid after a poison,
so ID generation continues rather than panicking.
