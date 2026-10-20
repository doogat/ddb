# Design: 00185 ddb-client Rust crate (MVP)

## Architecture fit

New leaf workspace crate, `ddb-client`, outside the `ddb-core` / `ddb-server` / `ddb-cli` chain. It is a pure HTTP/GraphQL consumer of a running `ddb serve`: no git, no SQLite, no actor. It depends on nothing from this workspace at runtime; `ddb-core` is a **dev-dependency only**, used by one parity test that pins the client's error-code string constants to `ddb_core::error::codes`.

The PRD's Success Metrics (line 31) require `ddb-client` be added to both root `Cargo.toml` `members` **and** `default-members`. The crate is added to both, per the PRD, even though its dev-only `ddb-core` dependency means a default `cargo build` now also builds `ddb-core` as part of resolving `ddb-client`'s dev-dependency graph for workspace commands that build dev-deps (e.g. `cargo test --workspace`); a plain `cargo build` (no tests) does not compile dev-dependencies, so the added crate stays cheap for the common case the PRD's "fast local loop" comment (root `Cargo.toml`'s own `cargo build` doc line) describes.

## Module placement

New files only:

```
ddb-client/Cargo.toml
ddb-client/src/lib.rs          # DdbClient facade, re-exports, config resolution
ddb-client/src/transport.rs    # DdbClient::{new,with_token,typed_query,typed_query_with_vars,execute_sql,query_sql}
ddb-client/src/parse.rs        # envelope + partial-error policy, parse_connection_items, parse_nested, parse_sql_rows
ddb-client/src/error.rs        # DdbClientError enum + graphql_code() extraction
ddb-client/src/readiness.rs    # wait_ready()
```

Edits to existing files:

- `Cargo.toml` (root): add `"ddb-client"` to both `members` and `default-members`, per the PRD's Success Metrics.

No other existing file changes. `ddb-server` is untouched — the client only ever talks to it over HTTP.

## Interfaces & contracts

### `ddb-client/Cargo.toml`

```toml
[package]
name = "ddb-client"
version = "0.1.0"
edition = "2021"

[dependencies]
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["time"] }

[dev-dependencies]
ddb-core = { path = "../ddb-core" }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time"] }
wiremock = "0.6"
```

`reqwest` version and `thiserror = "2"` match `ddb-server/Cargo.toml` and `ddb-core/Cargo.toml` respectively (reuse inventory). `rustls-tls` is added because `ddb-server`'s existing `reqwest` entry has no TLS backend configured (it never makes outbound HTTPS calls); a downstream client talking to a real deployment needs one — rustls avoids an OpenSSL system-dependency tax on downstream builds. `wiremock` is the mock HTTP transport the MVP tests against (dev-dependency only, matching the PRD's "tested against a MOCK HTTP transport" MVP cut).

### `ddb-client/src/error.rs`

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DdbClientError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("graphql error [{code}]: {message}")]
    GraphQl { code: DdbErrorCode, message: String, context: Vec<(String, serde_json::Value)> },

    #[error("malformed response at {location}: {reason}")]
    Parse { location: String, reason: String },

    #[error("timed out waiting for readiness after {0:?}")]
    ReadinessTimeout(std::time::Duration),

    #[error("no auth token configured (checked {0})")]
    NoToken(String),
}

/// Typed mirror of the server's stable error-code vocabulary. String values
/// are parity-tested (tests/codes_parity.rs) against `ddb_core::error::codes`
/// for every variant that has a core constant. `NotFound`, `Validation`, and
/// `Internal` map to the server's `"NOT_FOUND"`/`"VALIDATION_ERROR"`/
/// `"INTERNAL_ERROR"` literals, which `ddb_core::app_contract::error` keeps as
/// inline strings in its private `ERROR_POLICY` table, not exported
/// constants — there is nothing in core to pin those three against, so the
/// parity test checks only that the client's literal matches the exact
/// string the docs' "Stable error codes" list publishes. See Risks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DdbErrorCode {
    NotFound,
    Validation,
    SqlError,
    UniqueViolation,
    ReferencesViolation,
    NotNullViolation,
    UnknownField,
    TypeNotRegistered,
    CascadeCycle,
    SingletonViolation,
    SingletonNotFound,
    SchemaApplyPartial,
    SchemaDestructiveBlocked,
    TransactionNotSupported,
    InvalidZone,
    RemoteTimeout,
    /// Any `extensions.code` string not in the table above (including the
    /// server's `"INTERNAL_ERROR"`).
    Unknown(String),
}

impl DdbErrorCode {
    pub fn as_str(&self) -> &str { /* match -> the exact wire string per variant */ }
    pub fn from_wire(code: &str) -> Self { /* reverse of as_str, Unknown(code) fallback */ }
}

pub fn graphql_code(errors: &[serde_json::Value]) -> Option<DdbErrorCode> {
    // first error's extensions.code, mirroring jink's single-code extraction
}
```

`DdbClientError::GraphQl.context` carries the error envelope's other `extensions` entries (e.g. `columns`/`column` from `app_contract::error`'s `entry("column")`/`entry("columns")` population) as raw JSON rather than typed fields — downstream code that needs a specific field reads `context` by key; this avoids re-deriving a typed shape per error code in the MVP.

### `ddb-client/tests/codes_parity.rs` (dev-only, workspace parity test)

```rust
#[test]
fn error_code_strings_match_core() {
    use ddb_client::error::DdbErrorCode;
    assert_eq!(DdbErrorCode::SqlError.as_str(), ddb_core::error::codes::SQL_ERROR);
    assert_eq!(DdbErrorCode::UniqueViolation.as_str(), ddb_core::error::codes::UNIQUE_VIOLATION);
    assert_eq!(DdbErrorCode::ReferencesViolation.as_str(), ddb_core::error::codes::REFERENCES_VIOLATION);
    assert_eq!(DdbErrorCode::NotNullViolation.as_str(), ddb_core::error::codes::NOT_NULL_VIOLATION);
    assert_eq!(DdbErrorCode::UnknownField.as_str(), ddb_core::error::codes::UNKNOWN_FIELD);
    assert_eq!(DdbErrorCode::TypeNotRegistered.as_str(), ddb_core::error::codes::TYPE_NOT_REGISTERED);
    assert_eq!(DdbErrorCode::CascadeCycle.as_str(), ddb_core::error::codes::CASCADE_CYCLE);
    assert_eq!(DdbErrorCode::SingletonViolation.as_str(), ddb_core::error::codes::SINGLETON_VIOLATION);
    assert_eq!(DdbErrorCode::SingletonNotFound.as_str(), ddb_core::error::codes::SINGLETON_NOT_FOUND);
    assert_eq!(DdbErrorCode::SchemaApplyPartial.as_str(), ddb_core::error::codes::SCHEMA_APPLY_PARTIAL);
    assert_eq!(DdbErrorCode::SchemaDestructiveBlocked.as_str(), ddb_core::error::codes::SCHEMA_DESTRUCTIVE_BLOCKED);
    assert_eq!(DdbErrorCode::TransactionNotSupported.as_str(), ddb_core::error::codes::TRANSACTION_NOT_SUPPORTED);
    assert_eq!(DdbErrorCode::InvalidZone.as_str(), ddb_core::error::codes::INVALID_ZONE);
    assert_eq!(DdbErrorCode::RemoteTimeout.as_str(), ddb_core::error::codes::REMOTE_TIMEOUT);
    // NotFound/Validation have no core constant to pin against (see Risks);
    // this only guards against an accidental typo drifting the two literals
    // this test itself defines and ddb-server's ERROR_POLICY table uses.
    assert_eq!(DdbErrorCode::NotFound.as_str(), "NOT_FOUND");
    assert_eq!(DdbErrorCode::Validation.as_str(), "VALIDATION_ERROR");

    // every core code constant has a client-side variant (catches additions)
    for core_code in ddb_core::error::codes::ALL {
        assert!(
            [
                DdbErrorCode::SqlError, DdbErrorCode::UniqueViolation, DdbErrorCode::ReferencesViolation,
                DdbErrorCode::NotNullViolation, DdbErrorCode::UnknownField, DdbErrorCode::TypeNotRegistered,
                DdbErrorCode::CascadeCycle, DdbErrorCode::SingletonViolation, DdbErrorCode::SingletonNotFound,
                DdbErrorCode::SchemaApplyPartial, DdbErrorCode::SchemaDestructiveBlocked,
                DdbErrorCode::TransactionNotSupported, DdbErrorCode::InvalidZone, DdbErrorCode::RemoteTimeout,
            ].iter().any(|v| v.as_str() == *core_code),
            "ddb_core::error::codes::{core_code} has no DdbErrorCode variant"
        );
    }
}
```

The reverse-coverage loop over `codes::ALL` is what makes the test an actual drift guard (a new core code with no client variant fails loud) rather than a one-way spot check.

### `ddb-client/src/parse.rs`

```rust
#[derive(Debug, serde::Deserialize)]
pub struct GraphQlResponse {
    pub data: Option<serde_json::Value>,
    #[serde(default)]
    pub errors: Vec<serde_json::Value>,
}

/// `data` non-null -> Ok(data) even with `errors` present (logged partial,
/// pinned to jink's `typed_query_with_vars_partial_errors_with_data_returns_ok`
/// policy). `data` null -> Err from the first error's extensions.code.
pub fn envelope_result(resp: GraphQlResponse) -> Result<serde_json::Value, DdbClientError>;

/// Typed connection items: `{ edges: [{ node: T }] }` or `{ items: [T] }`
/// shape (supports both to match existing GraphQL connection conventions).
/// A malformed item returns `Parse { location: "items[<i>]", .. }`, never
/// dropped or substituted; a valid empty array returns `Ok(vec![])`.
pub fn parse_connection_items<T: serde::de::DeserializeOwned>(
    data: &serde_json::Value,
    field: &str,
) -> Result<Vec<T>, DdbClientError>;

/// A single nested mutation payload, e.g. `{ createDoogat: { doogat: {...} } }`.
pub fn parse_nested<T: serde::de::DeserializeOwned>(
    data: &serde_json::Value,
    path: &[&str],
) -> Result<T, DdbClientError>;

/// The `SqlResult` GraphQL object (`ddb-server/src/schema/type_defs.rs:240-272`)
/// has four fields on the wire: `columns: [String]!`, `rows: [String]!`
/// (each row's cells are strings, matching `SqlResult::Rows.rows:
/// Vec<Vec<String>>` server-side — SQL values are NOT typed over GraphQL),
/// `affected: Int`, and `message: String`. Core's `SqlResult` enum
/// (`ddb-core/src/sql_engine/mod.rs:36-46`) has three variants mapping onto
/// these fields: `Rows{columns,rows,..}` -> populated `columns`/`rows`,
/// `affected`/`message` null; `Affected(usize)` -> `affected` set,
/// `columns`/`rows` empty, `message` null; `Ok(String)` -> `message` set,
/// everything else empty/null. `Ok` is not a minor case: INSERT returns
/// `SqlResult::Ok(created_ids.join(","))` (`ddb-core/src/sql_engine/dml/insert.rs:313,323`),
/// as does every DDL statement (CREATE/ALTER/DROP TABLE, zone/unique/
/// singleton ops, rename) and BEGIN/COMMIT/ROLLBACK — only UPDATE/DELETE
/// return `Affected`. `parse_sql_result` branches on which of the three
/// fields is populated. A malformed row records its index in the Parse
/// error location (`"rows[<i>]"`); valid empty `rows` returns
/// `Ok(SqlOutcome::Rows { columns, rows: vec![] })`.
#[derive(Debug, Clone)]
pub enum SqlOutcome {
    Rows { columns: Vec<String>, rows: Vec<Vec<String>> },
    Affected(u64),
    Message(String),
}

pub fn parse_sql_result(
    data: &serde_json::Value,
    field: &str,
) -> Result<SqlOutcome, DdbClientError>;
```

### `ddb-client/src/transport.rs`

```rust
pub struct DdbClient {
    http: reqwest::Client,
    endpoint: String,
    token: Option<String>,
}

impl DdbClient {
    /// Resolves endpoint/token per the precedence in Configuration below.
    /// Reads the token file at construction (fails loud via `NoToken` only
    /// when `typed_query`/`execute_sql` actually need auth and none was
    /// found - construction itself never fails on a missing token, matching
    /// jink's "no-token constructor for local/test use").
    pub fn new() -> Self;
    pub fn with_endpoint(endpoint: impl Into<String>) -> Self;
    pub fn with_token(self, token: impl Into<String>) -> Self;

    pub async fn typed_query<T: serde::de::DeserializeOwned>(
        &self, query: &str,
    ) -> Result<T, DdbClientError>;

    pub async fn typed_query_with_vars<T: serde::de::DeserializeOwned>(
        &self, query: &str, vars: serde_json::Value,
    ) -> Result<T, DdbClientError>;

    /// Statement execution: INSERT/DDL/transaction statements return
    /// `SqlOutcome::Message` server-side, UPDATE/DELETE return
    /// `SqlOutcome::Affected` - both are success for `execute_sql`, folded
    /// into one `SqlWriteOutcome`. Errs with `Parse` if the server instead
    /// returned a `Rows` shape (a SELECT run through `execute_sql`).
    pub async fn execute_sql(&self, sql: &str) -> Result<SqlWriteOutcome, DdbClientError>;
    /// Row query (`SELECT`); returns columns + string-cell rows. Errs with
    /// `Parse` if the server instead returned an `Affected`/`Message` shape.
    pub async fn query_sql(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>), DdbClientError>;
}

/// `execute_sql`'s result: the two `SqlOutcome` variants that represent a
/// successful write/DDL/transaction statement (never `Rows` - that is a
/// `Parse` error for `execute_sql`, handled via `query_sql` instead).
#[derive(Debug, Clone)]
pub enum SqlWriteOutcome {
    Affected(u64),
    Message(String),
}
```

`typed_query` is `typed_query_with_vars` with `vars: json!({})` — one request-building path, not two (reuse-within-the-crate; avoids the duplicate-codepath shape `jink: src/ddb_client.rs` has today). Both route the raw envelope through `parse::envelope_result`, then `serde_json::from_value::<T>`, surfacing a deserialize failure as `DdbClientError::Parse { location: "typed_query result", .. }`.

### `ddb-client/src/readiness.rs`

```rust
/// Polls `GET {endpoint_base}/health` with capped exponential backoff
/// (matches `jink: tests/common/mod.rs:411` poll_health: start 50ms, x2 each
/// retry, cap 2s, until `timeout` elapses). Treats HTTP 200 OR 503 with a
/// parseable `{status, version}` body as "answered" (503 = degraded but
/// alive) for the purpose of distinguishing "server up, index degraded" from
/// "nothing listening"; only a connection failure or timeout elapsing is an
/// Err. The base URL is derived from the GraphQL endpoint by stripping the
/// `/graphql` suffix. Targets `/health` (full index-readiness, `health_ready`
/// -> `actor.health_check()`), not the cheaper `/health/live` process-liveness
/// probe (`ddb-server/src/lib.rs:164-167`): a downstream about to issue its
/// first query needs the index ready, not just the process alive.
pub async fn wait_ready(
    base_endpoint: &str,
    timeout: std::time::Duration,
) -> Result<(), DdbClientError>;
```

### Configuration conventions (`ddb-client/src/lib.rs`)

Precedence, documented in a doc comment on `DdbClient::new`: **explicit arg > env > default**.

- Endpoint: `DdbClient::with_endpoint(url)` arg > `DDB_URL` env > default `http://127.0.0.1:2891/graphql`.
- Token: `DdbClient::with_token(tok)` arg > `DDB_TOKEN` env (reusing the e2e harness's existing name, `tests/e2e/conformance/setup.rs:49`, not minting a second spelling) > token file `~/.config/ddb/token` (via the `dirs` crate... actually avoid a new dependency: resolve `~/.config/ddb/token` with `std::env::var("HOME")` + `PathBuf`, matching how `jink: src/ddb_client.rs:41-52` does it without a `dirs` crate dependency) > none (requests needing auth fail with `NoToken`).

## Data flow

```
DdbClient::typed_query_with_vars(query, vars)
  -> transport: build POST body {query, variables}, attach Authorization header if token present
  -> reqwest send -> raw JSON body
  -> parse::envelope_result: deserialize into GraphQlResponse, branch on data/errors
       data present (even with errors) -> Ok(data: Value)
       data null -> extract first error's extensions.code via error::graphql_code -> Err(DdbClientError::GraphQl)
  -> serde_json::from_value::<T>(data) -> Ok(T) | Err(DdbClientError::Parse)

DdbClient::execute_sql / query_sql
  -> same transport+envelope path, then parse::parse_sql_result on the `executeSql`/`sql` field,
     then execute_sql requires SqlOutcome::Affected|Message (folded into SqlWriteOutcome) / query_sql requires SqlOutcome::Rows (Parse error on the other variants)

readiness::wait_ready
  -> independent reqwest GET loop against {base}/health, no GraphQL envelope involved
```

Every path returns a `Result<_, DdbClientError>` — there is one error type for the whole crate; transports never panic, matching the repo-wide "no runtime panics in library code" guardrail (AGENTS.md).

## Reuse inventory

- `reqwest = "0.12"` version and `features = ["json"]` already pinned in `ddb-server/Cargo.toml:34` — reused verbatim for the version/feature pin (plus `rustls-tls`, see Interfaces). Note: that existing entry is itself under `[dev-dependencies]` in `ddb-server` (its own test harness), not a validated production usage anywhere in the workspace today — `ddb-client` is the first crate to depend on `reqwest` as a normal, runtime dependency.
- `thiserror = "2"` already pinned in `ddb-core/Cargo.toml:30` — reused verbatim for `DdbClientError`.
- `ddb_core::error::codes` (`ddb-core/src/error.rs:103-145`, confirmed present including `SQL_ERROR` added by 00173 — PRD's open question about whether 00173 had landed is resolved: it has) is the parity-test target; included as `[dev-dependencies]` only, matching the PRD's own "test-time, never a normal dependency" requirement. Verified: `ddb-uniffi-bindgen/Cargo.toml` depends only on `uniffi`, not `ddb-core` — there is no existing dev-dependency-on-ddb-core precedent in this workspace; this is new but still the PRD's own pre-decided shape (justified directly by the build-tax-avoidance reasoning in the PRD's Risks section, not by an invented analog).
- `DDB_TOKEN` env var name (`tests/e2e/conformance/setup.rs:49`) reused rather than minting a second spelling, per the PRD's explicit instruction.
- `/health` response shape (`ddb-server/src/lib.rs:232-250`: `{status: "ok"|"degraded", version}`) reused as `wait_ready`'s contract; no server change needed, the endpoint already exists.
- `SqlResult` GraphQL object shape (`ddb-server/src/schema/type_defs.rs:240-272`: `{columns: [String]!, rows: [String]!, affected: Int, message: String}`, mirroring core `ddb_core::sql_engine::SqlResult`'s `Rows{columns,rows,column_types}` / `Affected(usize)` / `Ok(String)` enum, `ddb-core/src/sql_engine/mod.rs:36-46`) is the verified wire contract `parse_sql_result`/`execute_sql`/`query_sql` are built against — row cells are strings on the wire, not typed JSON values, and `message` (the `Ok` variant) is INSERT's and every DDL statement's success shape, not a minor case (`ddb-core/src/sql_engine/dml/insert.rs:313,323`).
- **Nothing found** for a pre-existing GraphQL client helper, HTTP retry/backoff helper, or typed-error enum in this workspace to port — greps tried: `rg -n "reqwest::Client" ddb-server ddb-core ddb-cli`, `rg -n "graphql_code|typed_query|wait_ready|poll_health" ddb-*` (all return nothing outside `ddb-server`'s own axum routes, which are server-side, not client-side). jink's `src/ddb_client.rs` (the thing this crate replaces) is not in this repository, so it cannot be read or ported mechanically — the PRD's line references are taken as given context, not verified against jink's source. This crate is new code, not an extraction.

## Alternatives considered

1. **Smallest-diff: skip the crate, document the pattern in `docs/src/guide/building-apps.md` only.** Rejected — this is explicitly what the PRD's evidence section argues against: jink and future downstreams would keep re-verifying the same 226+542 LOC. A doc page captures intent, not a compiled, testable contract.
2. **Hard `ddb-core` runtime dependency (the PRD's originally-considered, explicitly reversed choice).** Rejected per the PRD's own Risks section: taxes every downstream build with git2/rusqlite/automerge for a thin HTTP client. Kept as dev-dependency only.
3. **Chosen: standalone `ddb-client` crate, async-only, dev-dependency parity test.** The PRD's pre-decided shape. Slightly larger than option 1 (a full new crate vs. a doc page) but the extra size buys a compiled, typed, independently-testable contract that jink can `cargo add` and that a parity test keeps honest against core's error vocabulary — the thing a doc page cannot do.

## Risks & edge cases

- **`app_contract::error::codes` does not exist as a module** (verified: `ddb-core/src/app_contract/error.rs` has no `pub mod codes`; `NOT_FOUND`/`VALIDATION_ERROR`/`INTERNAL_ERROR` are private string literals inside the `ERROR_POLICY` static table). The PRD's "parity-test against ... ddb_core::app_contract::error::codes" cannot be implemented literally — there is nothing exported to pin against. The parity test instead hardcodes `"NOT_FOUND"`/`"VALIDATION_ERROR"` as the two extra typed-error variants per the docs' "Stable error codes" list, and the task's acceptance criterion is read as "pin every code that has a core constant; document the two that don't" rather than a literal two-module parity claim. Exporting `pub mod codes` from `app_contract::error` is out of scope here (it would touch the shared `ERROR_POLICY` table four other transports already depend on) — flagged as a likely follow-up, not done in this PRD.
- **Likely next change 1**: a real-server integration test (00188 ephemeral-serve), once that PRD lands — the crate's transport/parse/error modules are already isolated from the mock so swapping in a real server only adds a test file, no production code change.
- **Likely next change 2**: codegen (00189) will want to generate the `T` types `typed_query`/`parse_connection_items` are generic over — keeping those functions generic over `DeserializeOwned` rather than hand-writing per-type wrappers is what keeps that door open.
- **Likely next change 3**: a blocking variant, if a downstream needs one — explicitly out of scope (PRD "Async runtime coupling" risk), and the `DdbClient` struct holds no tokio-runtime-specific state beyond the `reqwest::Client` itself, so a `block_on` wrapper could be added later without restructuring.
- **Edge case**: `DDB_TOKEN`/token-file both absent and a call needs auth → `NoToken` surfaces at call time, not at `DdbClient::new()`, preserving the "no-token constructor for local/test use" requirement while still failing loud (not silently sending an unauthenticated request) when auth is actually needed.
- **Edge case**: GraphQL envelope has `errors` with **no** `extensions.code` at all (a malformed or non-ddb-server GraphQL endpoint) → `graphql_code` returns `None`; `envelope_result`'s error path uses `DdbErrorCode::Unknown(String::new())` rather than panicking on an `Option::unwrap`.

## Test strategy outline

Per-task local gate: `cargo build`, `cargo clippy --workspace --all-targets`, `cargo test-ci` (covers the new crate's unit tests once added to the workspace `members`). No e2e/smoke/walkthrough scenario is required — this is a library crate with no CLI/server surface, exactly as the PRD's Phase 2 acceptance criterion states.

- **Phase 0 (error + parse)**: fixture-based unit tests in `ddb-client/src/parse.rs` and `error.rs` — valid/malformed connection items, valid/malformed SQL rows, valid empty results, the `codes_parity.rs` dev-test (including the reverse-coverage loop over `codes::ALL`).
- **Phase 1 (transport)**: `wiremock`-mocked GraphQL server — happy path, partial-errors-with-data → `Ok`, error-envelope → typed `DdbErrorCode`, missing/invalid token → `NoToken`/401 handling.
- **Phase 2 (readiness + facade)**: `wiremock`-mocked `/health` returning 200/503/timeout — `wait_ready` returns `Ok` on 200, treats 503 as still-polling-worthy (per Risks), errors on elapsed timeout with no response.
- **Critical scenarios from the PRD** (happy path, partial-error edge case, typed-error-code case) are each one `wiremock` test, directly naming the mocked route and asserting the typed result — these ARE the crate's conformance surface per Transport Impact.

## Review log

- (question) execute_sql's affected-rows return type was unverified against the actual server schema when first drafted. Resolved during fix-up: confirmed against `ddb-server/src/schema/type_defs.rs:240-270` and `ddb_core::sql_engine::SqlResult` (`ddb-core/src/sql_engine/mod.rs:36-46`) — the wire shape is one `SqlResult` object with `columns`/`rows`/`affected`, not a separate per-call shape; `parse_sql_result` now returns a `SqlOutcome` enum and `execute_sql`/`query_sql` each require the matching variant.
dispatch 1 (claude): cardinal-sin 0, blocker 2, non-blocker 2, question 1
- codex (dispatch 2) could not read the design doc or PRD at all: both its shell tool and its node_repl MCP fallback failed with `sandbox-exec: sandbox_apply: Operation not permitted` before producing any findings. Treated as a codex outage; Claude fallback ran in its place per the skill's codex-outage rule.
- (blocker, fixed) `SqlOutcome`/`parse_sql_result` omitted the `SqlResult::Ok`/`message` wire state — INSERT and all DDL/transaction statements return `Ok(String)` server-side (`ddb-core/src/sql_engine/dml/insert.rs:313,323` for INSERT; DDL and BEGIN/COMMIT/ROLLBACK likewise), not `Affected`, so `execute_sql` as originally designed could not succeed on INSERT or DDL. Fixed: `SqlOutcome` gained a `Message(String)` variant; `execute_sql` now returns a new `SqlWriteOutcome{Affected|Message}` folding both non-`Rows` outcomes into one success type.
dispatch 2 (claude-fallback): cardinal-sin 0, blocker 1, non-blocker 0, question 0
- codex (dispatch 3) again failed to read either document (`sandbox-exec: sandbox_apply: Operation not permitted` on both the shell tool and the node_repl MCP fallback). Claude fallback verified the dispatch-2 fix directly against `ddb-core/src/sql_engine/mod.rs:36-46`, `ddb-core/src/sql_engine/dml/insert.rs:306-324`, `ddb-server/src/schema/base_types/to_gql.rs:301-338`, and `ddb-server/src/pgwire.rs:119-131`: the fix is correct and complete. A full fresh pass found nothing new; minor pre-existing naming drift (`parse_sql_rows`/`parse_sql_result`, `code()`/`as_str()`) noted as cosmetic, not fixed.
dispatch 3 (claude-fallback): cardinal-sin 0, blocker 0, non-blocker 0, question 0
