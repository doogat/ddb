# Design: Unify server error-mapping paths

## Architecture fit

`ddb-core/src/app_contract/error.rs` already owns the adapter-neutral
`AppError`/`AppErrorCategory` conversion (`From<DoogatError> for AppError`) —
this is the one layer with no transport dependency, so the single policy
table belongs there, not in `ddb-server`. `ddb-server/src/error.rs` and
`http_error.rs` currently re-decide classification themselves (`classify`,
`http_error_response`'s `match &e`); they become thin encoders over the core
policy, matching the existing `to_graphql_error_from_app` pattern that
already routes through `AppError`. No new crate, no new module boundary —
this closes the gap between `app_contract::error` (the intended chokepoint,
per the Architecture Decisions capsule entry) and the two surfaces that
still bypass it.

## Module placement

- **Edit** `ddb-core/src/app_contract/error.rs`:
  - add the `SqlEngine` arm to `From<DoogatError> for AppError`.
  - add `pub const SQL_ERROR: &str = "SQL_ERROR";` to `ddb-core/src/error.rs`'s
    `codes` module (new file edit, same module as the other code constants).
  - add `FfiVariant` enum and `ErrorRendering` struct + `error_rendering(AppError) -> ErrorRendering` function.
- **Edit** `ddb-server/src/error.rs`: `classify` and `to_graphql_error_from_app`
  become thin wrappers over `error_rendering`; `classify`'s whole per-variant
  match is deleted (every variant, including the old `SqlEngine` arm, is now
  covered by `AppError::from(&DoogatError)` + `error_rendering`); `classify`
  keeps its existing `&DoogatError` signature — see Interfaces & contracts
  below for why a new `From<&DoogatError>` impl is needed instead of `Clone`.
- **Edit** `ddb-server/src/http_error.rs`: `http_error_response` derives
  `StatusCode` from `ErrorRendering.http_status` instead of its own `match`.
- **No edits** to `ddb-server/src/pgwire.rs` or `ddb-core/src/ffi/records.rs`
  in this PRD — out of scope (00180, 00177); this PRD only guarantees
  `ErrorRendering.ffi_variant` exists so those PRDs have a stable target.
- **New** `ddb-server/tests/error_parity.rs`: cross-transport parity tests.

## Interfaces & contracts

In `ddb-core/src/error.rs`, add to the existing `codes` module:
```rust
pub const SQL_ERROR: &str = "SQL_ERROR";
```

In `ddb-core/src/app_contract/error.rs`, add the `SqlEngine` arm to the
existing `match err` in `From<DoogatError> for AppError`, placed alongside
the other simple-variant arms:
```rust
DoogatError::SqlEngine(_) => AppError {
    code: codes::SQL_ERROR,
    message,
    category: AppErrorCategory::InvalidInput,
    field: None,
    details: vec![],
},
```

Add a transport-neutral FFI hint enum (core-only, no UniFFI dependency —
00177 maps it to its own `DdbError` variants):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiVariant {
    NotFound,
    Validation,
    Conflict,
    Internal,
}

impl From<AppErrorCategory> for FfiVariant {
    fn from(c: AppErrorCategory) -> Self {
        match c {
            AppErrorCategory::NotFound => FfiVariant::NotFound,
            AppErrorCategory::InvalidInput => FfiVariant::Validation,
            AppErrorCategory::Conflict => FfiVariant::Conflict,
            AppErrorCategory::Internal => FfiVariant::Internal,
        }
    }
}
```

Add the rendering struct and the sole policy function, in the same file:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorRendering {
    pub code: &'static str,
    pub message: String,
    pub http_status: u16,
    pub category: AppErrorCategory,
    pub field: Option<String>,
    pub details: Vec<(String, AppErrorDetail)>,
    pub ffi_variant: FfiVariant,
}

/// The single code -> (status, redaction, ffi-hint) policy. Every transport
/// adapter (GraphQL, REST, NoSQL HTTP; PgWire/FFI in 00180/00177) derives its
/// output from this function and must not re-decide redaction or status.
///
/// `AppError.message` is `DoogatError::to_string()`, which for every simple
/// variant carries a thiserror `Display` prefix (`"sql engine: "`,
/// `"not found: "`, `"validation: "`, `"invalid path: "`, `"conflict: "`,
/// `"bad request: "`, `"parse: "` — `ddb-core/src/error.rs`'s `#[error(...)]`
/// attributes). Legacy `classify` never carried this prefix (it matches
/// `DoogatError` variants directly and clones the inner `String`), so this
/// is the one place that strips it before any transport sees the message —
/// moved here (not deleted) from `to_graphql_error_from_app`'s old
/// per-code `prefix` table, and now shared by REST/NoSQL too, which never
/// had this stripping and would otherwise regress by gaining the prefix.
pub fn error_rendering(app: AppError) -> ErrorRendering {
    let AppError { code, message, category, field, details } = app;
    let prefix = match code {
        "NOT_FOUND" => Some("not found: "),
        "VALIDATION_ERROR" => Some("validation: "),
        "INVALID_PATH" => Some("invalid path: "),
        "CONFLICT" => Some("conflict: "),
        "BAD_REQUEST" => Some("bad request: "),
        "SQL_ERROR" => Some("sql engine: "),
        "PARSE_ERROR" => Some("parse: "),
        _ => None,
    };
    let message = match prefix {
        Some(p) => message.strip_prefix(p).unwrap_or(&message).to_string(),
        None => message,
    };
    let is_internal = matches!(category, AppErrorCategory::Internal);
    let http_status: u16 = match code {
        "SQL_ERROR" | "NOT_NULL_VIOLATION" | "UNKNOWN_FIELD" | "TYPE_NOT_REGISTERED" => 422,
        "VALIDATION_ERROR" => 400,
        _ => match category {
            AppErrorCategory::NotFound => 404,
            AppErrorCategory::Conflict => 409,
            AppErrorCategory::Internal => 500,
            AppErrorCategory::InvalidInput => 422,
        },
    };
    let ffi_variant = FfiVariant::from(category.clone());
    if is_internal {
        ErrorRendering {
            code,
            message: "internal error".to_string(),
            http_status,
            category,
            field: None,
            details: vec![],
            ffi_variant,
        }
    } else {
        ErrorRendering { code, message, http_status, category, field, details, ffi_variant }
    }
}
```

`DoogatError` is not `Clone` (its `Io` variant wraps `std::io::Error`, which
isn't `Clone`), and `to_graphql_error`/`to_server_error` need the original
`DoogatError` again after classifying it (to read `Structured { context, .. }`
for `attach_context`), so `classify` cannot take `DoogatError` by value either.
Add a second conversion, `impl From<&DoogatError> for AppError`, next to the
existing by-value one in `ddb-core/src/app_contract/error.rs`: same arms,
but matching `err: &DoogatError` and `.clone()`-ing only the `String`/`Vec`
payload fields each arm already needs (never the whole error) — `message`
becomes `err.to_string()` (works on a reference) and `Structured`'s `context`
clones via `context.clone().into_iter()`. This keeps both `classify(&e)` call
sites in `error.rs` unchanged and sidesteps the `Clone` requirement entirely.

`ddb-server/src/error.rs`:
```rust
pub fn classify(e: &DoogatError) -> (&'static str, String) {
    let rendering = ddb_core::app_contract::error_rendering(AppError::from(e));
    (rendering.code, rendering.message)
}
```
`to_graphql_error_from_app` keeps its own signature (`AppError -> Error`) but
its body becomes: call `error_rendering(app)`, then emit `code`/`field`/
`details` from the `ErrorRendering` exactly as it does today from `AppError`
fields. Its own copy of the `prefix` match is deleted — `error_rendering`
now does that stripping once, for every transport, instead of only for
GraphQL.

`ddb-server/src/http_error.rs`:
```rust
pub fn http_error_response(e: DoogatError) -> (StatusCode, Json<ErrorBody>) {
    let rendering = ddb_core::app_contract::error_rendering(AppError::from(e));
    let status = StatusCode::from_u16(rendering.http_status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(ErrorBody { error: rendering.code.into(), message: rendering.message }))
}
```

## Data flow

`DoogatError` (any surface) → `AppError::from(DoogatError)` (adds the new
`SqlEngine` arm) → `error_rendering(AppError)` (the one policy: redaction +
status + ffi hint) → transport encoding:
- GraphQL: `to_graphql_error_from_app` emits `async_graphql::Error` with
  `extensions.code`/`field`/`details` from the `ErrorRendering`.
- REST/NoSQL: `http_error_response`/`classify` emit `(StatusCode, ErrorBody)`
  from the same `ErrorRendering`.
- FFI/PgWire (future, 00177/00180): consume `ErrorRendering.ffi_variant` /
  `.message` the same way once wired.

## Reuse inventory

- `ddb_core::app_contract::{AppError, AppErrorCategory, AppErrorDetail}` —
  already the adapter-neutral envelope; extended in place rather than
  replaced. (`rg "AppErrorCategory" ddb-core/src ddb-server/src`.)
- `ddb-server/src/error.rs::classify` / `to_graphql_error_from_app` and
  `ddb-server/src/http_error.rs::http_error_response` — kept as the
  transport-facing function names/signatures so REST/GraphQL call sites
  (`rest.rs`, `nosql_api.rs`, resolver `?` sites) need no edits beyond the
  body of these three functions.
- `ddb-core/src/error.rs::codes` module — existing constant registry;
  `SQL_ERROR` added alongside the other 9 structured-code constants rather
  than a parallel list (`rg "pub const" ddb-core/src/error.rs`).
- `nothing found` for an existing error-policy/rendering abstraction: greps
  tried `rg -i "rendering|error.?polic"` across `ddb-core/src` and
  `ddb-server/src` return only this PRD's planned names.

## Alternatives considered

1. **Smallest diff — add the `SqlEngine` arm only, leave 4 mappers as-is.**
   Fixes the GraphQL/REST SQL-error disagreement (the PRD's headline bug) but
   leaves the fan-out (and the next-code-needs-4-edits maintenance hazard)
   unresolved; rejected because the PRD's Success Metrics require the single
   table, not just the one symptom.
2. **New `ddb-server::error_policy` module, re-exporting from core.** Keeps
   `ddb-core` dependency-free of HTTP concerns by construction, at the cost
   of one more file and one more hop for readers. Rejected: `AppError` /
   `AppErrorCategory` already live in `ddb-core::app_contract` with zero HTTP
   types in `ErrorRendering` (status is a bare `u16`), so there is no
   dependency to isolate — the extra module would just be indirection.
3. **Chosen: extend `app_contract::error.rs` in place** with `ErrorRendering`
   + `error_rendering()`, and make the three `ddb-server` functions thin
   wrappers. Smallest total diff that still satisfies "one table, every
   transport reads it," and keeps the module boundary the capsule already
   documents (`app_contract` is adapter-neutral; `ddb-server` encodes).

## Risks & edge cases

- **Two real public-shape changes, both already accepted in the PRD's Risks
  section**: GraphQL SQL errors stop being redacted as `INTERNAL_ERROR` (now
  `SQL_ERROR` + message); any REST `Internal`-category code that today leaks
  a raw message instead gets redacted. Both must land in the same commit as
  the `docs/src/technical/errors.md` compatibility note the PRD requires.
- **CLI inherits the new `SqlEngine` arm** (`app_err` renders via `AppError`
  directly) — SQL errors there also become `SQL_ERROR` not `INTERNAL_ERROR`.
  Covered by the PRD's own Risks row; search `ddb-cli/src` for any pinned
  `INTERNAL_ERROR` expectation on a SQL-syntax-error test before closing.
- **Next likely changes this design should not box in**: (1) 00177 replacing
  `ffi/records.rs`'s lossy `From<DoogatError> for DdbError` with a mapping
  from `FfiVariant` — the enum is deliberately small (4 variants) and
  core-only so 00177 can widen or map it without touching this PRD's code;
  (2) 00180 making PgWire consume `ErrorRendering.message`/`.code` instead of
  `Display`-wrapping — `ErrorRendering` already carries everything PgWire
  needs, no further core change anticipated; (3) a future per-code HTTP
  status table growing past the two special-cased code strings — if a third
  one-off code needs special status, moving the `match code` arm to a
  `HashMap<&str, u16>` table is a pure refactor, not a contract change.
- **Every simple `DoogatError` variant's `Display` carries a thiserror prefix**
  (`ddb-core/src/error.rs`'s `#[error("sql engine: {0}")]` etc.) that
  `AppError.message = err.to_string()` inherits; `error_rendering` must strip
  it (see Interfaces & contracts) or REST/NoSQL — which today get the raw
  unprefixed message straight from `classify`'s direct `DoogatError` match —
  would regress by gaining a visible prefix nothing asked for.
- **`DoogatError` is not `Clone`** (its `Io` variant wraps `std::io::Error`).
  `classify` stays `&DoogatError` and needs an `AppError` from a borrow;
  resolved by adding `impl From<&DoogatError> for AppError` (see Interfaces &
  contracts) rather than deriving `Clone` on the error type.

## Test strategy outline

- Unit tests in `ddb-core/src/app_contract/error.rs` (colocated, Tier 1):
  `AppError::from(DoogatError::SqlEngine(..)).code == "SQL_ERROR"` and
  category `InvalidInput`; `error_rendering` unit tests per registered code
  covering category, `http_status`, redaction, and `ffi_variant`, including
  `SCHEMA_APPLY_PARTIAL` (Internal→500, redacted), `SCHEMA_DESTRUCTIVE_BLOCKED`
  (Conflict→409), `SQL_ERROR` (InvalidInput, 422, message preserved), and
  `VALIDATION_ERROR` (InvalidInput, 400).
- Existing `ddb-server/src/error.rs` and `http_error.rs` test modules stay
  green with their current assertions (they test input/output pairs, not
  implementation, so a correct thin-wrapper rewrite does not change their
  expected values) except where the PRD's accepted shape change requires an
  update (the `SqlEngine`-via-GraphQL redaction test and any REST
  `Internal`-category verbatim-message test must be updated to the new
  redacted/unredacted expectation).
- New `ddb-server/tests/error_parity.rs` (Tier 2 integration): drives a
  representative code set (Validation, NotFound, Conflict, Internal,
  `SCHEMA_APPLY_PARTIAL`, `SCHEMA_DESTRUCTIVE_BLOCKED`, `SQL_ERROR`, plus
  `UNIQUE_VIOLATION` and `SINGLETON_NOT_FOUND` — today's REST status for
  these comes from a direct code-level `match` in `http_error.rs` rather
  than the category fallback, so they pin that the category-driven rewrite
  reproduces the same 409/404) through `classify`/`to_graphql_error_from_app`
  and `http_error_response`, asserting identical code + redaction and the
  documented per-code HTTP status.

## Review log

- non-blocker: Module Placement's "folded into the generic Structured/AppError path" phrasing described a smaller edit than Interfaces & contracts actually specifies (classify's whole per-variant match is replaced, not partially folded) — reworded.
- question: confirm `http_error_response`'s signature `fn(DoogatError) -> (StatusCode, Json<ErrorBody>)` is unchanged so `rest.rs`/`nosql_api.rs` call sites need no edits — unchanged by this design; both already pass by value.
dispatch 1 (claude): cardinal-sin 0, blocker 1, non-blocker 2, question 1
- non-blocker: owned (`From<DoogatError>`) and borrowed (`From<&DoogatError>`) conversions duplicate the same match arms, a second drift point — accepted: the two impls are mechanical and small; a future shared-helper refactor is noted as a non-blocking follow-up, not required for this PRD.
- non-blocker: the parity test matrix doesn't explicitly exercise `to_graphql_error`/`to_server_error`'s post-classify `attach_context` path for internal-category `Structured` errors — recorded, not required to block planning; existing `app_err_internal_category_exposes_only_code_extension` test already covers this shape.
dispatch 2 (codex): cardinal-sin 0, blocker 1, non-blocker 2, question 0
- question: dispatch 3 (codex, sandboxed read-only) could not execute file reads itself (`sandbox_apply: Operation not permitted` on both its tool attempts) and asked for the 7 prefix strings to be independently confirmed against `ddb-core/src/error.rs`. Confirmed directly (`rg -n "#\[error" ddb-core/src/error.rs`, run earlier this session): lines 36/39/42/45/48/51/54/57/60/63/66/69/72/75/78/81/85/93 give exactly `git: `, `yaml: `, `sql: `, `automerge: `, `io: `, `toml: `, `parse: `, `not found: `, `validation: `, `invalid path: `, `sql engine: `, `conflict: `, `sync: `, `index: `, `bad request: `, `version mismatch: ...`, `redb: `, and `{message}` (no prefix, `Structured`) — the design's 7-entry prefix table (`not found: `/`validation: `/`invalid path: `/`conflict: `/`bad request: `/`sql engine: `/`parse: `) matches exactly; no change needed.
- non-blocker: test strategy should assert prefix stripping removes only the outer Display prefix, not a coincidentally-matching inner message (e.g. inner text itself starting with "sql engine: ") — add one such case to the Tier 1 `error_rendering` unit tests at plan time.
- non-blocker: no explicit test asserts `AppError::from(&err)` and `AppError::from(err)` agree for the same error — accepted as a non-blocking follow-up, not required for this PRD's scope.
dispatch 3 (codex): cardinal-sin 0, blocker 0, non-blocker 2, question 1
