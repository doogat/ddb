# Design: Server security and resource hardening (00180)

## Architecture fit

All six hardenings live inside `ddb-server` (the axum/async-graphql/pgwire
transport crate), at the exact sites the PRD names:

- `schema/mod.rs::build_schema` — the dynamic-schema builder, extended
  with `.limit_depth` / `.limit_complexity` / `.disable_introspection()`.
- `reload.rs::SchemaReloader` — rebuilds the schema on typedef change; must
  thread the same bounds so a hot reload doesn't silently drop them.
- `lib.rs::build_app` — the single axum `Router` assembly point; the
  router-wide body-limit middleware lands here, once, ahead of every route.
- `pgwire.rs::DdbBackend::do_query` / `build_rows_response` — the SELECT
  row cap and the error-redaction wrapper.
- `auth.rs::validate_bearer` — constant-time compare.
- `config.rs::ServerConfig` — the four new config keys and the bind-warning
  predicate.
- `nosql_api.rs` + `ddb-core/src/service/utility.rs` — pagination threading
  for the three NoSQL scan/backlinks reads.

Nothing here crosses into `ddb-core`'s domain layer except the NoSQL
pagination change (reuses the existing `PageRequest` contract type, which
already lives in `ddb-core::app_contract` per 00176) and the PgWire
redaction change (reuses `ddb_core::app_contract::error_rendering`, the
00173 unified policy, already used by `error.rs::classify` and
`http_error.rs::http_error_response`). No new adapter-neutral type is
needed; `PageRequest` already is one.

## Module placement

All edits to existing files; no new modules.

- `ddb-server/src/schema/mod.rs` — add `pub struct GraphQlLimits`, extend
  `build_schema`'s signature, apply the three builder calls.
- `ddb-server/src/reload.rs` — `SchemaReloader` stores `GraphQlLimits`,
  threads it into `reload_loop`'s `build_schema` call.
- `ddb-server/src/lib.rs` — `build_initial_schema` takes `GraphQlLimits`;
  `build_app` gains the two body-limit layers; `run` computes
  `GraphQlLimits` from `cfg` and calls `config::warn_if_non_loopback`.
- `ddb-server/src/pgwire.rs` — add `redact_pg_error`, a private
  `PgRedactedError` type, and `cap_unbounded_select`; replace the 3
  `DoogatError`-wrapping `PgWireError::ApiError(Box::new(e))` sites (of 7
  total `map_err` sites in the file — see Interfaces & contracts) and
  rewrite the `execute_select` call site in `do_query` to route through
  `cap_unbounded_select`.
- `ddb-server/src/auth.rs` — rewrite `validate_bearer`.
- `ddb-server/src/config.rs` — add `graphql_max_depth`,
  `graphql_max_complexity`, `graphql_introspection`, `max_body_bytes` to
  `ServerConfig` + `ServerSection`; add `is_loopback_bind` /
  `warn_if_non_loopback`.
- `ddb-server/src/nosql_api.rs` — `ScanParams` gains `limit`/`offset`;
  `scan`/`backlinks` decode a `PageRequest` and pass it through.
- `ddb-core/src/nosql.rs` — `RedbIndex::scan_by_type`/`scan_by_tag`/
  `backlinks`/`prefix_scan` take a `PageRequest` and enforce it inside the
  scan loop (skip-then-collect-then-break), so the cap bounds the scan's
  own memory, not just its post-hoc output.
- `ddb-core/src/service/utility.rs` — `nosql_scan_type` / `nosql_scan_tag`
  / `nosql_backlinks` take a `PageRequest` and forward it unchanged to the
  `RedbIndex` call.
- `ddb-server/Cargo.toml` — add `subtle = "2.6"` (direct dep, already
  transitive per the PRD) and `http-body-util = "0.1"` (direct dep,
  already transitive via `axum`/`axum-core`; needed to name
  `http_body_util::LengthLimitError` for the body-limit middleware's
  error-kind check — see Interfaces & contracts). No `tower-http` feature
  change — the body limit is enforced by a middleware function
  (`axum::body::to_bytes`), not a `tower_http::limit` layer.

## Interfaces & contracts

### GraphQL depth/complexity/introspection

```rust
// ddb-server/src/schema/mod.rs
#[derive(Debug, Clone, Copy)]
pub struct GraphQlLimits {
    pub max_depth: usize,
    pub max_complexity: usize,
    pub introspection: bool,
}

impl Default for GraphQlLimits {
    fn default() -> Self {
        Self { max_depth: 16, max_complexity: 2000, introspection: true }
    }
}

pub fn build_schema(
    actor: ActorHandle,
    read_pool: ReadPool,
    type_schemas: Vec<TableSchema>,
    reloader: Option<Arc<SchemaReloader>>,
    limits: GraphQlLimits,
) -> Result<Schema, String> {
    // ... unchanged until the final builder chain ...
    let mut builder = builder
        .extension(WarningExtensionFactory)
        .limit_depth(limits.max_depth)
        .limit_complexity(limits.max_complexity);
    if !limits.introspection {
        builder = builder.disable_introspection();
    }
    builder.finish().map_err(|e| e.to_string())
}
```

`async_graphql::dynamic::SchemaBuilder` already exposes `limit_depth`,
`limit_complexity`, and `disable_introspection` (same methods as the
non-dynamic builder; confirmed present in `async-graphql` 7.2.1's dynamic
API). All 18 existing test call sites in `schema/mod.rs` (`build_schema(actor,
pool, schemas, None)`) gain a fifth argument, `GraphQlLimits::default()`.

`SchemaReloader::new` gains a `limits: GraphQlLimits` parameter, stores it
as a field, and `reload_loop` passes `this.limits` to its `build_schema`
call. `lib.rs::build_initial_schema` gains the same parameter and passes it
to both `build_schema` and `SchemaReloader::new`.

### Config additions

```rust
// ddb-server/src/config.rs — ServerConfig
pub graphql_max_depth: usize,         // default 16
pub graphql_max_complexity: usize,    // default 2000
pub graphql_introspection: bool,      // default true
pub max_body_bytes: usize,            // default 8 * 1024 * 1024

// ServerSection (TOML [server])
graphql_max_depth: Option<usize>,
graphql_max_complexity: Option<usize>,
graphql_introspection: Option<bool>,
max_body_bytes: Option<usize>,
```

Loaded the same way `read_pool_size` is today (`file_cfg.server...or default`);
no CLI override flags — the PRD does not ask for `--` flags, only config
keys.

```rust
/// True when `bind` is a loopback address: `"localhost"`, or any numeric
/// IPv4/IPv6 address `std::net::IpAddr::is_loopback()` accepts — which
/// covers the whole `127.0.0.0/8` range (not just `127.0.0.1`) and every
/// IPv6 loopback spelling (`::1`, `0:0:0:0:0:0:0:1`), not a fixed string
/// whitelist that would misclassify a real loopback address as exposed.
pub fn is_loopback_bind(bind: &str) -> bool {
    bind == "localhost" || bind.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Log a startup WARNING when `bind` is not loopback. Does not block.
pub fn warn_if_non_loopback(bind: &str) {
    if !is_loopback_bind(bind) {
        tracing::warn!(
            %bind,
            "server is binding a non-loopback address with no TLS; \
             put it behind a TLS-terminating proxy"
        );
    }
}
```

Called once from `lib.rs::run`, right after `ServerConfig::load`.

### Router-wide body limit

**A plain `tower_http::limit::RequestBodyLimitLayer` + `DefaultBodyLimit::max`
pair is correct for REST and NoSQL but wrong for GraphQL, and must not be
used alone.** Verified against the vendored crate sources: axum-core's
`Bytes`/`Json` rejection path explicitly downcasts the boxed body error to
`http_body_util::LengthLimitError` and maps it to `413`
(`axum-core-0.5.6/src/extract/rejection.rs:33-41`) — REST and NoSQL, which
extract via `Json`, get a correct `413` from `RequestBodyLimitLayer`
alone. GraphQL does not: `async-graphql-axum` 7.2.1's
`GraphQLBatchRequest::from_request` reads the body via
`req.into_body().into_data_stream().map_err(|err|
std::io::Error::other(err.to_string()))`
(`async-graphql-axum-7.2.1/src/extract.rs:116-119`) — this discards the
error's concrete type before `async_graphql` ever sees it, so a
length-limit error becomes a generic `ParseRequestError::Io`, and
`GraphQLRejection::into_response` maps anything other than
`ParseRequestError::PayloadTooLarge` to `400`, not `413`
(`async-graphql-axum-7.2.1/src/extract.rs:38-50`). A `RequestBodyLimitLayer`
would still stop an oversized GraphQL body from being fully buffered (the
resource-exhaustion bound the PRD cares about most), but it would answer
`400` where the PRD's own acceptance text says `413`, failing that
criterion for exactly the route the PRD calls out by name ("async-graphql-axum
... bypasses DefaultBodyLimit").

**Design: one `axum::middleware::from_fn` that owns the whole body-limit
decision**, applied once at the top-level router merge (same position the
two-layer approach occupied), so GraphQL, REST, and NoSQL all get the SAME
413 response from the SAME code, rather than depending on each
destination extractor's own (inconsistent) error mapping:

```rust
// ddb-server/src/lib.rs
use axum::body::{to_bytes, Body};
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::error::Error as StdError;

/// Enforce `max_body_bytes` uniformly across every route (including
/// GraphQL, which bypasses `DefaultBodyLimit` — see Interfaces &
/// contracts). `to_bytes` rejects once the cumulative byte count would
/// cross `max_body_bytes`, independent of `Content-Length` or
/// chunked/streamed framing — but a single oversized frame can still be
/// read into memory before that check rejects it (`http_body_util::Limited`
/// checks frame-by-frame, not byte-by-byte), so this bounds *accepted*
/// payload size exactly, not peak transient memory during rejection.
/// `to_bytes`'s error can be either `LengthLimitError` (genuinely over the
/// cap — 413, matching axum's own `Bytes`/`Json` convention, see
/// `axum-core-0.5.6/src/extract/rejection.rs:11-55`) or some other body
/// read failure (client disconnect, malformed framing — 400, not 413, so
/// a transport error is never mislabeled as "too large").
async fn enforce_body_limit(
    Extension(max_body_bytes): Extension<usize>,
    request: Request,
    next: Next,
) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, max_body_bytes).await {
        Ok(b) => b,
        Err(e) if e
            .source()
            .is_some_and(|s| s.downcast_ref::<http_body_util::LengthLimitError>().is_some()) =>
        {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    next.run(Request::from_parts(parts, Body::from(bytes))).await
}

// final layer chain, build_app:
//   .layer(DefaultBodyLimit::max(max_body_bytes))        // raises Json/Bytes' own 2 MiB default (REST/NoSQL)
//   .layer(middleware::from_fn(enforce_body_limit))      // uniform 413 for every route incl. GraphQL
```

The `.source().downcast_ref::<LengthLimitError>()` check mirrors axum-core's
own `FailedToBufferBody::from_err` (same file/lines above) — the canonical
way this codebase's own dependency distinguishes "really too large" from
"some other buffering failure" after `Limited`/`to_bytes` wraps the error.
`http_body_util` is already a transitive dependency (pulled in by `axum`
and `tower-http`); this uses its existing `LengthLimitError` type without
adding a new direct dependency.

`axum::body::to_bytes(body, limit)` is axum's own documented mechanism for
a hard body-size cap with a guaranteed caller-controlled response: it
reads the stream and returns `Err` as soon as the cumulative byte count
would exceed `limit`, working identically for `Content-Length` and
chunked/streamed bodies (it never trusts `Content-Length`; it counts
actual bytes read). Because this is middleware wrapping the *whole*
request, not an extractor's own fallible read, the 413 decision is made
and returned **before** GraphQL's (or any other) extractor ever runs —
closing the exact gap `RequestBodyLimitLayer` left open. `build_app` gains
a `max_body_bytes: usize` parameter, threaded from `cfg` in `run`, passed
in via `Extension(max_body_bytes)` and the `middleware::from_fn` layer
added once at the final merge.

**`DefaultBodyLimit::max(max_body_bytes)` is still needed, as an
additional layer, for REST/NoSQL — the middleware alone does not raise
their extractor's own internal cap.** `Bytes::from_request` (which `Json`
is built on, and which REST/NoSQL use) looks up a `DefaultBodyLimitKind`
request extension and **defaults to 2 MiB when that extension is absent**
(`axum-core-0.5.6/src/extract/default_body_limit.rs:7-8`, `:82-86`) — this
check runs on EVERY call to `Bytes::from_request`, including one reading
an already-fully-buffered `Body::from(bytes)` the `enforce_body_limit`
middleware reconstructed, because the check is extension-driven, not
body-state-driven. Without `DefaultBodyLimit::max(max_body_bytes)` also
layered, a 5 MiB REST/NoSQL body would pass `enforce_body_limit`'s 8 MiB
check, then still be rejected by `Json`'s own un-overridden 2 MiB default
— silently failing the PRD's own "Align REST/NoSQL Json's extractor
bound" acceptance text even though `enforce_body_limit` reports success.
So the final layer set is **both**: `.layer(DefaultBodyLimit::max(max_body_bytes))`
(raises `Json`'s default so REST/NoSQL don't double-reject under the new
8 MiB ceiling) **and** `.layer(middleware::from_fn(enforce_body_limit))`
(the uniform 413 for every route, GraphQL included). No `tower-http`
`"limit"` feature and no `tower_http::limit::RequestBodyLimitLayer` are
needed — `DefaultBodyLimit` alone cannot give GraphQL a 413 (see above),
and `enforce_body_limit` alone cannot raise `Json`'s own default, so both
are required together, each closing a gap the other cannot.

### PgWire SELECT row cap

```rust
// ddb-server/src/pgwire.rs
const DEFAULT_SELECT_ROW_CAP: usize = ddb_core::app_contract::PageRequest::MAX_LIMIT; // 10_000

/// Sets `LIMIT <cap>` on a single-statement SELECT that carries no
/// explicit LIMIT clause, by mutating the parsed AST and re-serializing it
/// — NOT by string-appending `" LIMIT <cap>"` to the raw SQL text (a
/// trailing `--`/`/* */` comment, or any other case sqlparser tolerates as
/// trailing-but-unparsed-into-the-statement text, would otherwise put the
/// cap inside dead text and leave the query genuinely unbounded; mutating
/// the AST and calling its `Display` makes this impossible by
/// construction rather than by argument). `sql` is already known
/// `is_select_only`. Returns `sql` unchanged if a LIMIT is already present
/// or parsing fails (parsing already succeeded once in `is_select_only`,
/// so failure here is unreachable in practice; the fallback is "leave it
/// unbounded" rather than panic).
fn cap_unbounded_select(sql: &str) -> String {
    let dialect = GenericDialect {};
    match Parser::parse_sql(&dialect, sql) {
        Ok(mut stmts) if stmts.len() == 1 => {
            if let Statement::Query(q) = &mut stmts[0] {
                if q.limit.is_none() {
                    q.limit = Some(Expr::Value(
                        Value::Number(DEFAULT_SELECT_ROW_CAP.to_string(), false).into(),
                    ));
                    return stmts[0].to_string();
                }
            }
            sql.to_string()
        }
        _ => sql.to_string(),
    }
}
```

`sqlparser::ast::Query::limit` is `Option<Expr>` and both `Statement` and
`Query` implement `Display` (re-serializing to valid SQL text) — confirmed
in the vendored `sqlparser` 0.55.0 source
(`~/.cargo/registry/.../sqlparser-0.55.0/src/ast/query.rs:47`, `:73`;
`.../src/ast/mod.rs:3622`). `Expr::Value` takes a `ValueWithSpan`, and
`Value: Into<ValueWithSpan>` (`.../src/ast/value.rs:99`), so
`Value::Number(..).into()` is the correct, compiling construction. The
re-serialized text need not byte-match the original (whitespace/casing may
differ); that is irrelevant here since the output is only ever executed,
never shown to a client.

`do_query`'s `is_select_only(query)` branch calls
`self.read_pool.execute_select(cap_unbounded_select(query)).await` instead
of `execute_select(query.to_string())`.

### PgWire error redaction

```rust
// ddb-server/src/pgwire.rs
#[derive(Debug)]
struct PgRedactedError(String);

impl std::fmt::Display for PgRedactedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for PgRedactedError {}

/// Route a DoogatError through the 00173 unified policy before boxing it
/// into `PgWireError::ApiError`, so the wire never sees `git:`/`io:` detail —
/// the same redaction GraphQL (`error.rs::classify`) and REST/NoSQL
/// (`http_error.rs::http_error_response`) already apply.
fn redact_pg_error(e: ddb_core::error::DoogatError) -> PgWireError {
    let rendering = ddb_core::app_contract::error_rendering(
        ddb_core::app_contract::AppError::from(e),
    );
    PgWireError::ApiError(Box::new(PgRedactedError(rendering.message)))
}
```

Of the 7 `.map_err(|e| PgWireError::ApiError(Box::new(e)))` sites in
`pgwire.rs`, **3 wrap a real `DoogatError` and are in scope**:
`do_query`'s `execute_select` call, `do_query`'s `actor.execute_sql` call,
and `handle_pg_catalog_query`'s `read_pool.execute_select` call (the
`\dt`/table-listing path — also a real leak path, e.g. a locked or
corrupted index surfacing `git:`/`io:` detail through `psql \dt`). All
three become `.map_err(redact_pg_error)`. The remaining 4 sites, inside
`handle_pg_catalog_query`'s row-building loop (`encode_field` ×3 +
`finish`), wrap `DataRowEncoder` errors, not `DoogatError` — those stay
`Box::new(e)` as today; they are not `git:`/`io:` detail and the PRD's
redaction scope is the SQL execution path, not protocol encoding
failures.

### Constant-time token compare

```rust
// ddb-server/src/auth.rs
use subtle::ConstantTimeEq;

pub fn validate_bearer(header_value: &str, expected: &str) -> bool {
    match header_value.strip_prefix("Bearer ") {
        Some(t) if t.len() == expected.len() => {
            t.as_bytes().ct_eq(expected.as_bytes()).into()
        }
        _ => false,
    }
}
```

Unequal-length and missing/malformed-prefix cases both return `false`
through ordinary (non-constant-time) short-circuit logic — the PRD is
explicit that unequal lengths "reject without promising length secrecy",
so this is in contract, not a gap.

### NoSQL scan/backlinks cap

**The cap is enforced inside the redb scan itself, not by collecting the
full match set and slicing afterward.** A first draft of this section
page-sliced a fully-collected `Vec<String>` after the fact; two
independent cross-model review dispatches flagged that this still
allocates memory proportional to the *full* matching set even for
`limit=1` — the Capability description's own intent ("so one authed
client cannot materialize an entire large type's id vector") is about
that in-process materialization, not only the wire response, so this is
fixed at the source rather than documented as a residual:

```rust
// ddb-core/src/nosql.rs — RedbIndex
use crate::app_contract::PageRequest;

pub fn scan_by_type(&self, type_name: &str, page: PageRequest) -> Result<Vec<String>> {
    self.prefix_scan(BY_TYPE, &format!("{type_name}/"), page)
}
pub fn scan_by_tag(&self, tag: &str, page: PageRequest) -> Result<Vec<String>> {
    self.prefix_scan(BY_TAG, &format!("{tag}/"), page)
}
pub fn backlinks(&self, target_id: &str, page: PageRequest) -> Result<Vec<String>> {
    self.prefix_scan(LINKS, &format!("{target_id}/"), page)
}

fn prefix_scan(
    &self,
    table_def: TableDefinition<&str, &[u8]>,
    prefix: &str,
    page: PageRequest,
) -> Result<Vec<String>> {
    let txn = self.db.begin_read().map_err(redb_err)?;
    let t = match txn.open_table(table_def) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
        Err(e) => return Err(redb_err(e)),
    };

    let mut ids = Vec::with_capacity(page.limit.min(1024));
    let range = t.range(prefix..).map_err(redb_err)?;
    let mut skipped = 0usize;

    for entry in range {
        let (key, _val) = entry.map_err(redb_err)?;
        let k: &str = key.value();
        if !k.starts_with(prefix) {
            break;
        }
        if skipped < page.offset {
            skipped += 1;
            continue;
        }
        if ids.len() >= page.limit {
            break;
        }
        if let Some(id) = k.strip_prefix(prefix) {
            ids.push(id.to_string());
        }
    }
    Ok(ids)
}
```

(`Vec::with_capacity(page.limit.min(1024))` avoids over-allocating for a
large `limit` the matching set never reaches, while still pre-sizing for
the common case — `PageRequest::MAX_LIMIT` is 10,000, so an unclamped
`with_capacity(page.limit)` on every call would itself allocate up to
10,000 slots even for a type with 3 matches; capping the pre-allocation
at 1024 and letting `Vec` grow from there is the smaller change.) The
redb `Range` cursor still advances through `offset` entries one at a time
(no redb seek-by-count API), but no `String` is ever allocated for a
skipped or past-the-page entry, and the loop `break`s as soon as `page.limit`
ids are collected — regardless of how many more matching keys follow. This
bounds the method's own memory to `O(limit)`, not `O(total matches)`,
closing the gap the first draft left as a documented residual.

`ddb-core/src/service/utility.rs`'s `nosql_scan_type`/`nosql_scan_tag`/
`nosql_backlinks` now just forward `page: PageRequest` straight through to
the `RedbIndex` call — no `paginate` helper, no post-hoc slice:

```rust
// ddb-core/src/service/utility.rs
pub fn nosql_scan_type(&self, type_name: &str, page: PageRequest) -> Result<Vec<String>> {
    self.open_nosql_fresh()?.scan_by_type(type_name, page)
}
// nosql_scan_tag, nosql_backlinks: identical shape.
```

`#[cfg(not(feature = "nosql"))]` variants gain the same `_page: PageRequest`
parameter (unused, matching the existing `_type_name`/`_tag`/`_id`
convention) so both cfg arms keep one signature.

```rust
// ddb-server/src/nosql_api.rs
#[derive(Deserialize)]
pub struct ScanParams {
    #[serde(rename = "type")]
    pub doogat_type: Option<String>,
    pub tag: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
```

`scan` decodes `PageRequest::decode(params.limit, params.offset)` once
(mapping its `Err` the same way other `DoogatError`s on this route map —
through `nosql_error`, since `PageRequest::decode` already returns
`DoogatError::Validation` on an out-of-range limit/offset, which
`nosql_error` already renders as `400 VALIDATION_ERROR` — no new error
path), then passes the decoded `PageRequest` to `nosql_scan_type`/
`nosql_scan_tag`. `backlinks` adds the same two query params (via
`Query<ScanParams>` reused, or a smaller dedicated params struct — reuse
`ScanParams`'s `limit`/`offset` fields by giving `backlinks` its own
two-field params struct to avoid an unused `doogat_type`/`tag` pair on that
route; either is acceptable, the planner should pick the smaller one) and
the same `PageRequest::decode` → `nosql_backlinks` call.

## Data flow

- **GraphQL query** → `graphql_handler` → `schema.execute(...)` → the
  depth/complexity extension rejects before any resolver runs (this is
  async-graphql's own validation pass, before execution) → unaffected
  queries resolve as today.
- **Any HTTP request body** → `enforce_body_limit` middleware (applied
  once, at the top-level router merge) → `axum::body::to_bytes(body,
  max_body_bytes)` reads the stream and errors once the cumulative byte
  count would cross the bound → the middleware returns `413` directly →
  **no handler or extractor for any route (GraphQL, REST, or NoSQL) ever
  sees an oversized body**, so none of them gets a chance to mis-map the
  error the way GraphQL's own extractor would (see Interfaces &
  contracts).
- **PgWire SELECT** → `do_query` → `is_select_only` → `cap_unbounded_select`
  mutates the parsed AST's `Query.limit` and re-serializes (setting a
  `LIMIT` on a `LIMIT`-less single SELECT cannot change its meaning for
  rows within the cap) → `read_pool.execute_select` → `build_rows_response`
  (unchanged) → a LIMIT-less SELECT never buffers more than
  `DEFAULT_SELECT_ROW_CAP` rows regardless of result size. `DEFAULT_SELECT_ROW_CAP`
  is a default for queries that don't specify their own bound, matching
  the PRD's explicit Behavior text ("an explicit LIMIT is honored as-is");
  it is not a hard security ceiling overriding a client-specified larger
  `LIMIT` — a client that writes `LIMIT 1000000` still gets up to that
  many rows, same as today.
- **PgWire error** → `DoogatError` from either `execute_select` or
  `execute_sql` → `redact_pg_error` → `error_rendering` (00173's one
  rendering table) → a `PgRedactedError` whose `Display` is the redacted
  message → `PgWireError::ApiError` → wire. Same rendering table GraphQL
  and REST/NoSQL already consume, so DML and SELECT errors alike are
  redacted identically across all three transports.
- **Non-loopback bind** → `ServerConfig::load` resolves `bind` → `run`
  calls `warn_if_non_loopback(&cfg.bind)` once at startup, before the
  listener binds → a log line, never a block.
- **NoSQL scan/backlinks** → `nosql_api::scan`/`backlinks` decode
  `PageRequest` from query params → `ddb-core` service forwards it to
  `RedbIndex::prefix_scan`, which skips `offset` matching keys, collects at
  most `limit` ids, and stops scanning as soon as the page is full → only
  `O(limit)` ids are ever allocated, and only the page crosses the wire as
  JSON.

## Reuse inventory

- `ddb_core::app_contract::PageRequest` (`ddb-core/src/app_contract/commands.rs:91`) —
  the existing validated-page type from 00176 (`DEFAULT_LIMIT = 50`,
  `MAX_LIMIT = 10_000`, `decode(limit, offset)` rejecting out-of-range
  input as `DoogatError::Validation`). Reused as-is for the NoSQL cap and
  as the PgWire row-cap constant's source (`PageRequest::MAX_LIMIT`), so
  the "10,000 row" bound the PRD names is the SAME constant everywhere,
  not a second hardcoded `10_000`.
- `ddb_core::app_contract::error_rendering` / `AppError::from` (00173,
  already used by `error.rs::classify` and
  `http_error.rs::http_error_response`) — reused unchanged for the PgWire
  redaction; no new rendering rule, no new error code.
- `nosql_api.rs::nosql_error` / `http_error.rs::http_error_response` —
  reused unchanged; `PageRequest::decode`'s `Validation` error flows
  through the existing NoSQL error path with no new branch.
- `async_graphql::dynamic::SchemaBuilder::limit_depth` /
  `limit_complexity` / `disable_introspection` — the dynamic-schema
  builder (`async-graphql` 7.2.1, already a direct dependency) exposes the
  same bound-setting methods as the static builder; confirmed present,
  no new dependency.
- `axum::body::to_bytes` / `axum::middleware::from_fn` /
  `axum::extract::DefaultBodyLimit` — all already used/available in
  `axum` 0.8 (already a direct dependency); no new dependency, no new
  `tower-http` feature. (An earlier draft of this section proposed
  `tower_http::limit::RequestBodyLimitLayer` in place of the
  `middleware::from_fn`; a cross-model review dispatch, backed by reading
  the vendored `async-graphql-axum` 7.2.1 and `axum-core` 0.5.6 sources,
  found that substitution produces the wrong HTTP status for GraphQL
  specifically, and a second verification pass against the same sources
  found that dropping `DefaultBodyLimit` entirely — as that draft also
  did — would have left REST/NoSQL's own extractor silently capped at
  axum's 2 MiB default regardless of the new 8 MiB bound. Both gaps are
  closed in Interfaces & contracts, Router-wide body limit.)

Greps run before concluding nothing else exists: `rg -n "limit_depth|limit_complexity|disable_introspection"` (repo-wide, zero hits outside this design); `rg -n "ConstantTimeEq|subtle::"` (zero hits — confirms `subtle` is transitive-only today, per the PRD's own claim); `rg -n "RequestBodyLimitLayer|DefaultBodyLimit"` (zero hits); `rg -n "redact|classify|error_rendering"` (hits only in `error.rs`/`http_error.rs`, confirming one existing redaction chokepoint to extend, not replace); `rg -n "paginate|PageRequest"` (hits only in the REST/GraphQL/FFI search path from 00176, confirming the NoSQL reads are the one remaining unbounded consumer the PRD's own provenance note already named).

## Alternatives considered

1. **Chosen: thread `GraphQlLimits`/`max_body_bytes` as explicit parameters
   through `build_schema`/`build_app`.** Slightly larger diff (touches ~16
   test call sites with one added `GraphQlLimits::default()` argument) but
   keeps every bound config-overridable end-to-end, matching the PRD's
   "config keys ... overridable" requirement and the existing pattern
   (`ServerConfig` already flows this way for `read_pool_size`,
   `remote_timeout_secs`).
2. **Smallest-diff alternative: hardcode the bounds as `const`s inside
   `build_schema`/`build_app`, no config plumbing.** Rejected — the PRD
   explicitly names `graphql_max_depth`/`graphql_max_complexity` and
   `graphql_introspection` as config keys in the Feature behavior text;
   hardcoding would fail the PRD's own acceptance criteria, not just be a
   smaller engineering choice.
3. **Streaming PgWire rows instead of a row cap.** The PRD's own Risk
   section prefers streaming "if cheap" but the Behavior/acceptance text
   commits to a cap; `pgwire` 0.25's `QueryResponse` already streams via
   `stream::iter`, but the bound problem is that `do_query` first collects
   the *full* `Vec<Vec<String>>` from `SqlResult::Rows` before that stream
   is built — truly fixing the memory bound end-to-end would mean
   streaming `SqlResult` itself out of `ddb-core`, a far larger change
   touching the SQL engine's row-production path. The row cap is the
   PRD-specified, smallest correct fix; true streaming is a `Risks & edge
   cases` follow-up, not in scope here.

## Risks & edge cases

- **`cap_unbounded_select` double-LIMIT risk**: if sqlparser ever parses a
  SELECT whose `q.limit` is `Some` but the raw SQL text's LIMIT clause sits
  inside a subquery/CTE at the *outer* level (e.g. `WITH x AS (SELECT ...)
  SELECT * FROM x` with no outer LIMIT) — `q.limit` is a property of the
  outermost `Query` AST node regardless of nesting, so this is already
  correctly detected; no special-case needed, but worth a test fixture
  (`WITH` + no outer LIMIT must still get capped).
- **`UNION`/`UNION ALL` queries** parse as a single `Statement::Query`
  whose `body` is a `SetExpr::SetOperation`; setting `limit` on that
  `Query` node applies to the whole union's output (standard SQL
  semantics, not per-arm), so capping a LIMIT-less union caps the combined
  result exactly as intended — add a `UNION` fixture to the unit tests
  (Test strategy outline) to pin this rather than leave it implicit.
  (Earlier draft of this section proposed string-appending `" LIMIT
  <cap>"` to the raw SQL text instead of mutating the AST; a cross-model
  review dispatch correctly flagged that a trailing `--`/`/* */` comment
  in the query text — e.g. `SELECT * FROM notes -- tail` — would put the
  appended `LIMIT` *inside* the comment, leaving the query genuinely
  unbounded. The Interfaces & contracts section above now mutates
  `Query.limit` on the parsed AST and re-serializes via `Display`, which
  cannot have this failure mode: the cap becomes a real AST node, not text
  sqlparser never re-parses.)
- **GraphQL introspection toggle breaks tooling in prod** (PRD's own named
  risk) — default `true` (introspection ON) keeps today's behavior; only a
  future `--production`/explicit config flip changes it, so this PRD ships
  with zero behavior change for existing deployments until an operator
  opts in.
- **`RedbIndex::prefix_scan` collecting-then-early-stopping, not a
  seek-by-offset**: redb's `Range` cursor has no "skip N" API, so an
  `offset` still costs one cursor advance per skipped key (cheap: no
  `String` allocation, no value deserialization for a skipped or
  past-the-page entry). A first draft of this section instead collected
  the full matching `Vec<String>` in `ddb-core/src/service/utility.rs` and
  sliced it afterward; two independent cross-model review dispatches
  flagged that as leaving the Capability description's own intent ("so
  one authed client cannot materialize an entire large type's id vector")
  unmet for any `limit`, since `limit=1` still allocated `O(total
  matches)` strings before slicing to one. The scan-level fix above
  bounds allocation to `O(limit)` regardless of how large the matching set
  is; the remaining `O(offset)` cursor-advance cost is accepted as cheap
  and inherent to offset-based pagination (the same cost REST's own
  `page`/`per_page` pagination already has over SQLite).
- **NoSQL pagination default/max mismatch with REST's existing `per_page`
  clamp (200)**: the PRD explicitly picks the 00176 `PageRequest` policy
  (default 50 / max 10,000) for NoSQL, not REST's narrower 200 — this is a
  deliberate divergence already decided by the PRD text ("the 00176
  `PageRequest` default and max"), not a new inconsistency to fix here.
- **Likely next changes after this PRD**: (1) the `cargo-deny` RUSTSEC
  advisory on `unsafe-libyaml`/yanked `chacha20`/`spin` flagged by nightly
  CI is a separate dependency-hygiene item, explicitly out of this PRD's
  scope — noted so it is not accidentally folded in; (2) true PgWire row
  streaming (see Alternative 3) if the 10,000-row cap proves too tight for
  a real workload; (3) a per-route (not just global) body-limit override,
  if attachments or bulk-import ever need a larger bound than 8 MiB — this
  design's global `enforce_body_limit` middleware would need to become
  per-router if that lands, which is why it is layered at the top-level
  merge in `build_app` rather than inside each sub-router, keeping that
  future split a one-layer move rather than a rewrite.

## Test strategy outline

Per-task Tier 1 gate (`cargo build`, `cargo clippy --workspace
--all-targets`, `cargo test-ci`) plus colocated unit tests for each
behavior, matching the PRD's acceptance criteria line for line:

- `schema/mod.rs`: depth-17 query rejected, depth-16 passes, complexity
  over 2000 rejected, a normal query unaffected, introspection answered
  when `introspection: true` / refused when `false` (5 cases → the PRD's
  5 acceptance clauses).
- `lib.rs`/`build_app`: exact-limit body passes, limit+1 body returns 413,
  for REST/GraphQL/NoSQL alike, with and without `Content-Length`
  (streamed) — a `tower::ServiceExt::oneshot` test harness (already used
  elsewhere in this crate's test modules per the `tower = { features =
  ["util"] }` dev-dependency) sending a body via `Body::from_stream` with
  no `Content-Length` header for the no-length case. The GraphQL case is
  the one this design's middleware exists for (`enforce_body_limit` runs
  before `GraphQLRequest::from_request`, so GraphQL gets the same `413`
  REST/NoSQL get, not GraphQL's own `400`-mapped `ParseRequestError::Io`)
  — test it explicitly, not just REST/NoSQL, since that is the regression
  this design's middleware choice specifically prevents. Also test a
  body between the old 2 MiB default and the new 8 MiB bound (e.g. 3 MiB)
  passes for REST/NoSQL — the regression `DefaultBodyLimit::max` exists to
  prevent — and that a genuinely malformed/truncated body (not merely
  oversized) gets `400`, not `413`, pinning the `LengthLimitError`
  downcast in `enforce_body_limit`.
- `pgwire.rs`: `cap_unbounded_select` unit tests (LIMIT-less SELECT gets
  capped via the AST mutation, explicit LIMIT preserved, non-SELECT
  untouched, `WITH`-CTE fixture, `UNION`/`UNION ALL` fixture, and a
  trailing-SQL-comment fixture — `SELECT * FROM notes -- tail` must still
  be capped, proving the AST-based fix closes the string-append bypass an
  earlier draft had); `redact_pg_error` unit test asserting a
  `DoogatError::Git(...)` input never appears in the output `Display`; an
  e2e/integration test driving a real `git:`/`io:`-triggering SELECT and a
  failing DML (e.g. duplicate-key `INSERT`) through a real PgWire
  connection, asserting no internal substring crosses the wire for
  either.
- `auth.rs`: the 5 existing `validate_bearer` unit tests stay (correct,
  wrong, empty, missing-prefix, lowercase-prefix) plus equal-prefix/
  different-length and empty-vs-nonempty cases added per the PRD's
  acceptance line; no timing assertion.
- `config.rs`: `is_loopback_bind` unit tests for `"127.0.0.1"`, `"127.0.0.2"`,
  `"::1"`, `"0:0:0:0:0:0:0:1"`, `"localhost"` (all true) and `"0.0.0.0"`,
  `"::"`, a garbage string (all false); a `tracing-test`-style
  capture (or a manual `tracing::Subscriber` test sink, matching whatever
  pattern this crate already uses for log-assertion tests, if any — if
  none exists yet, a minimal `tracing_subscriber::fmt` test writer is the
  smallest addition) asserting `warn_if_non_loopback("0.0.0.0")` logs and
  `warn_if_non_loopback("127.0.0.1")` does not.
- `nosql.rs`: `prefix_scan` unit tests — a type/tag/backlink set larger
  than `limit` returns exactly `limit` ids starting at `offset`; `limit`
  larger than the matching set returns the whole (smaller) set; `offset`
  past the end of the matching set returns empty; a test asserting the
  loop actually stops early (e.g. via a counted/instrumented fixture, or
  simply a large matching set + a tiny `limit` run under a wall-clock
  bound incompatible with full materialization) rather than only
  asserting the returned `Vec`'s length.
- `nosql_api.rs` / `service/utility.rs`: a scan over a type with more ids
  than the default page returns exactly `PageRequest::DEFAULT_LIMIT` ids
  at offset 0; `offset` honored; `limit` above `MAX_LIMIT` rejected
  (`DoogatError::Validation` → `400 VALIDATION_ERROR` through the existing
  `nosql_error` path, pinned the same way
  `nosql_error_delegates_to_shared_http_helper` already pins other codes).

e2e scenarios (`tests/e2e/`, per Phase 2's task): one Rust e2e scenario per
Critical Scenario in the PRD's Test Strategy (too-deep query, oversized
body, non-loopback bind warning, PgWire error redacted) plus the
NoSQL-scan-cap assertion folded into whichever scenario module already
covers NoSQL reads (00176's), rather than a fifth new file, since it is
the same capability class (bounded reads) as the existing NoSQL scenarios.

## Review log

- non-blocker: internally inconsistent count of PgWire `map_err` sites across sections — fixed (3 of 7 sites are in scope; both sections now agree).
- non-blocker: `build_schema` test call-site count was wrong (16 vs actual 18) — fixed.
- question: `cap_unbounded_select`'s string-append approach vs. comment tails and `UNION` — addressed in Risks & edge cases (safe because `is_select_only` already guarantees a single clean statement; `UNION` applies LIMIT to the combined result as intended, test fixture added to the outline).

dispatch 1 (claude): cardinal-sin 0, blocker 1, non-blocker 2, question 1

- blocker (fixed): `cap_unbounded_select`'s string-append approach left a trailing-SQL-comment bypass (`SELECT ... -- tail` puts the appended LIMIT inside the comment) — rewritten to mutate the parsed AST's `Query.limit` and re-serialize, verified against the vendored `sqlparser` 0.55.0 source (`Query.limit: Option<Expr>`, `Value: Into<ValueWithSpan>`, both `Statement`/`Query` implement `Display`).
- blocker (fixed): the `RequestBodyLimitLayer` + `DefaultBodyLimit::max` pair produces `400`, not `413`, for an oversized GraphQL body — verified against the vendored `async-graphql-axum` 7.2.1 and `axum-core` 0.5.6 sources (GraphQL's extractor discards the error's concrete type before `async_graphql` can map it to `PayloadTooLarge`). Replaced with one `axum::middleware::from_fn` using `axum::body::to_bytes` that owns the 413 decision uniformly across GraphQL/REST/NoSQL, before any extractor runs.
- non-blocker (fixed): `is_loopback_bind`'s fixed-string whitelist misclassified `127.0.0.2` and the expanded IPv6 loopback spelling as non-loopback — rewritten to parse via `std::net::IpAddr::is_loopback()`.
- non-blocker (recorded, not fixed): `paginate` still reads the full matching id set from the redb index before slicing, so memory use scales with the full set, not the page — real, but outside this PRD's own (wire-response) acceptance criterion; recorded as a follow-up candidate in Risks & edge cases.
- question (dispatch itself, not the design): codex's sandbox could not read this checkout (`sandbox_apply: Operation not permitted`) — it verified its findings against upstream crate docs/sources instead of this repo's pinned versions. All four findings above were independently re-verified against the actual vendored `~/.cargo/registry` sources for the exact pinned versions (`sqlparser` 0.55.0, `async-graphql-axum`/`axum-core` 7.2.1/0.5.6) before being accepted, so the sandbox gap did not propagate into this doc.

dispatch 2 (codex): cardinal-sin 2, blocker 1, non-blocker 1, question 1

- cardinal-sin (resolved, confirmed): the AST-mutation fix for the trailing-SQL-comment bypass holds; verified against the tagged `sqlparser` 0.55.0 source.
- blocker (fixed this pass): `DefaultBodyLimit::max(max_body_bytes)` must stay layered alongside `enforce_body_limit` — `Bytes::from_request` (which `Json` is built on) defaults to axum's own 2 MiB cap when no `DefaultBodyLimitKind` extension is present, regardless of what the middleware already buffered; a 3-8 MiB REST/NoSQL body would otherwise still 413 under the old default even though it's under the new configured bound. Both layers are now required together.
- blocker (resolved, confirmed): the `enforce_body_limit` middleware returning 413 before any extractor runs does close the GraphQL 400-vs-413 gap; confirmed against axum's real `to_bytes` signature and `Limited`-based implementation.
- blocker (fixed this pass, upgraded from dispatch 2's non-blocker): `paginate`'s collect-then-slice left `O(total matches)` allocation for any `limit`, including `limit=1` — a second independent cross-model pass pushed back harder on treating this as a documented residual, since it leaves the PRD's own named Capability intent (no client can materialize an entire type's id vector) unmet regardless of wire-response size. Fixed at the source: `RedbIndex::prefix_scan` now skips `offset` entries and stops scanning after `limit` ids are collected, bounding its own allocation to `O(limit)`.
- non-blocker (resolved, confirmed): `is_loopback_bind`'s `IpAddr`-based rewrite is correct; `is_loopback()` and `is_ok_and` both verified as the right APIs.
- non-blocker (fixed): the middleware's doc comment overclaimed "never buffers more than one byte past the limit" — `http_body_util::Limited` checks per-frame, not per-byte, so a single oversized frame can be read into memory before the check rejects it. Reworded to describe the accepted-payload bound, not a transient-memory guarantee.
- non-blocker (fixed): the middleware's `Err(_) => 413` conflated a real length-limit rejection with any other body-read failure (client disconnect, malformed framing) — now downcasts to `http_body_util::LengthLimitError` (the same idiom axum-core's own `Bytes` rejection uses) and returns 400 for anything else, so a transport error is never mislabeled as "too large".
- question (resolved): the PgWire row-cap doc text could read as an unconditional ceiling even for an explicit client `LIMIT` — clarified it is a default for LIMIT-less queries only, matching the PRD's own "explicit LIMIT honored as-is" text.

dispatch 3 (codex, conditional verification): cardinal-sin 1 resolved, blocker 3 (2 resolved, 1 newly found and fixed), non-blocker 3 (1 upgraded-and-fixed, 2 fixed), question 1 resolved — 0 open cardinal sins/blockers remain.
