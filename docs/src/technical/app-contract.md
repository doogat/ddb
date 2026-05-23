# Application Contract

**Source**: `ddb-core/src/app_contract/`

## Purpose

The `app_contract` module defines the shared, adapter-neutral surface that every transport (CLI, GraphQL, REST, PgWire, FFI, NoSQL HTTP) routes through. It sits above storage/index adapters and below transport adapters.

Goals (from PRD 00147):

- Define shared app-level commands and results for core workflows.
- Make warnings first-class so partial success and best-effort behavior are visible.
- Define a shared structured error envelope suitable for transport adapters.
- Keep transport adapters focused on auth, serialization, request parsing, and response formatting.
- Provide the contract surface that conformance tests can target.

## Module layout

| File | Owns |
|------|------|
| `mod.rs` | Module re-exports and the adapter-neutrality invariant note. |
| `commands.rs` | Input command DTOs (`CreateCommand`, `ReadCommand`, `UpdateCommand`, `DeleteCommand`, `SearchCommand`). |
| `results.rs` | Result payloads (`BrokenBacklink`, `DeleteResult`). |
| `output.rs` | `AppOutput<T>` envelope and `AppWarning`. |
| `error.rs` | `AppError`, `AppErrorCategory`, and the `From<DoogatError>` mapping. |

The adapter-neutrality invariant is enforced by the integration test `ddb-core/tests/app_contract_adapter_guard.rs`.

## Command DTOs

Commands are plain structs of domain/shared types. They follow a `<Verb>Command` naming pattern:

```rust
pub struct CreateCommand {
    pub title: String,
    pub tags: Vec<String>,
    pub doogat_type: Option<String>,
    pub body: String,
    pub fields: BTreeMap<String, Value>,
}

pub struct ReadCommand { pub id: String }
pub struct UpdateCommand { /* id + optional title/tags/type/body + fields */ }
pub struct DeleteCommand { pub id: String }
pub struct SearchCommand { pub query: String, pub limit: Option<usize>, pub offset: Option<usize> }
```

Per AGENTS.md: "no `rusqlite`, `git2`, `redb`, `axum`, or `async_graphql` in `ddb-core/src/types/**`; convert at adapter boundaries." The same rule applies here. Commands carry only domain types (e.g. `crate::types::Value`) and `std` types.

## `AppOutput<T>`

The success envelope for app-facing operations:

```rust
pub struct AppOutput<T> {
    pub value: T,
    pub warnings: Vec<AppWarning>,
}
```

Contract: "best-effort behavior, skipped rows, mirror issues, repair hints, and degraded reads are represented as structured warnings" (PRD 00147). A successful `AppOutput` with a non-empty `warnings` vector is the normal way partial success surfaces.

## `AppWarning`

```rust
pub struct AppWarning {
    pub code: &'static str,
    pub message: String,
}
```

`code` is a stable identifier; `message` is human-readable. Per PRD 00147: "transport adapters may format warnings differently but may not discard them for promised workflows."

## `AppError` envelope

```rust
pub enum AppErrorCategory {
    NotFound,
    InvalidInput,
    Conflict,
    Internal,
}

pub enum AppErrorDetail {
    String(String),
    List(Vec<String>),
}

pub struct AppError {
    pub code: &'static str,
    pub message: String,
    pub category: AppErrorCategory,
    pub field: Option<String>,
    pub details: Vec<(String, AppErrorDetail)>,
}
```

- `code` is a stable static string (e.g. `"NOT_FOUND"`, `"VALIDATION_ERROR"`, `"CONFLICT"`, `"INTERNAL_ERROR"`).
- `category` lets transports map to status without parsing `code`.
- `field` carries an optional field/path detail for input errors (`NOT_NULL_VIOLATION` and single-column `UNIQUE_VIOLATION` populate this with the column name).
- `details` preserves the full structured context from `DoogatError::Structured` as adapter-neutral key-value pairs. Non-structured errors produce an empty `details` vec.

`impl From<DoogatError> for AppError` maps existing `DoogatError` variants (`NotFound`, `Validation`, `Parse`, `InvalidPath`, `BadRequest`, `Conflict`) and the structured codes from `error::codes` (e.g. `SINGLETON_NOT_FOUND`, `UNIQUE_VIOLATION`, `REFERENCES_VIOLATION`, `NOT_NULL_VIOLATION`, `UNKNOWN_FIELD`, `TYPE_NOT_REGISTERED`) into the right `AppErrorCategory`. Anything else falls through to `AppErrorCategory::Internal` with code `"INTERNAL_ERROR"`.

## `DoogatService` facade

Transports route through `DoogatService` app-command entrypoints rather than calling lower-level service methods directly. The `create` entrypoint:

```rust
pub fn create(&self, cmd: CreateCommand) -> Result<AppOutput<ParsedDoogat>>
```

(`Result<T>` is `ddb-core`'s alias for `std::result::Result<T, DoogatError>`.) Other CRUD verbs follow the same `cmd -> Result<AppOutput<...>>` shape as they migrate.

## Transport adapter responsibilities

Transports own "auth, serialization, request parsing, and response formatting" (PRD 00147). Business policy lives in the contract/service layer.

Per AGENTS.md: "New cross-interface behavior belongs in the app contract first; transports should adapt commands/results, not own business policy." A transport adapter:

- Parses an incoming request into a `*Command`.
- Calls the matching `DoogatService` entrypoint.
- Renders the returned `AppOutput<T>` (value plus warnings) and any `AppError` in its native shape.
- Must surface warnings; discarding them on promised workflows is a contract violation. CLI prints warnings to stderr (one per line as `warning: <code>: <message>`). REST surfaces warnings in the `warnings` response field (always present, empty array when none). GraphQL warning forwarding is deferred (see PRD 00154, graphql-response-extension-warnings-v1).

## Worked example: createDoogat

A `CreateCommand` flows uniformly through both transports:

1. **GraphQL**: `ddb-server/src/actor/handlers.rs` parses the `createDoogat` mutation input, builds a `CreateCommand`, and forwards it to the actor.
2. **CLI**: `ddb-cli/src/commands/crud.rs` parses `ddb create` flags into a `CreateCommand`.
3. Both call `DoogatService::create(cmd)` and receive `Result<AppOutput<ParsedDoogat>, DoogatError>`.
4. On `Ok(AppOutput { value, warnings })`, each transport renders `value` in its native format (GraphQL object, CLI text, REST JSON) and emits `warnings`: CLI prints to stderr; REST includes a `warnings` array in the response body (always present). GraphQL warning forwarding is deferred to PRD 00154.
5. On `Err(e)`, each transport maps via `AppError::from(e)` to the right status/exit code and message.

The business policy (typedef routing, validation, ID generation, git commit) lives in `DoogatService::create`; both transports stay thin.
