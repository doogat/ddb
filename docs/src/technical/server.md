# GraphQL Server

Doogat DB exposes a GraphQL API via `ddb serve`, enabling mobile, desktop, and web clients to interact with the ddb over HTTP.

## Architecture

```
Client → HTTP (axum) → Bearer auth middleware → GraphQL POST /graphql
                                               → REST /rest/*
                                               → NoSQL /nosql/*
                     → WebSocket /ws (auth in handler: header or connection_init payload)
       → TCP (pgwire) → MD5 password auth ─────→ SQL simple query protocol

                         ┌─────────────────────────────┐
       Reads ───────────→│  ReadPool (semaphore-gated)  │
       (queries,         │  spawn_blocking per request  │
        SELECTs,         │  fresh Index + GitRepo each  │
        GETs)            └─────────────────────────────┘
                         ┌─────────────────────────────┐
       Writes ──────────→│  ActorHandle (mpsc channel)  │
       (mutations,       │  Actor thread (std::thread)  │
        INSERTs,         │  owns GitRepo + Index +      │
        DDL)             │  RedbIndex + SqlEngine        │
                         │  emits → EventBus            │
                         └─────────────────────────────┘
```

Read and write operations follow different paths. **Reads** (GraphQL queries, REST GETs, NoSQL lookups, pgwire SELECTs) go through the `ReadPool`, which dispatches each request to a `tokio::task::spawn_blocking` closure with its own freshly-opened `Index` and `GitRepo` handles. A semaphore caps concurrency (default: `min(available_parallelism, 4)`). SQLite WAL mode allows concurrent readers without blocking writes.

**Writes** (mutations, SQL DML, DDL) still serialize through the single-writer actor to maintain consistency. The actor bridges sync and async worlds: it runs on `std::thread::spawn` with `blocking_recv()`, while the HTTP layer is fully async (tokio + axum). Communication uses `tokio::sync::mpsc` for commands and `oneshot` channels for replies.

The `sql` query field uses `sqlparser` to classify queries: pure `SELECT` statements route through `ReadPool`, everything else goes to the actor.

## Interface Promises

`ddb serve` exposes four network interfaces. They are not equivalent:

| Interface | Promise level | Primary use case |
|-----------|--------------|-----------------|
| GraphQL (`/graphql`, `/ws`) | CRUD `Guaranteed` | Primary API for web and desktop apps |
| REST (`/rest/*`) | `Specialized` | Base CRUD and list/search; typed mutations not `Guaranteed` |
| PgWire (port 2892) | `Guaranteed` for SQL/reporting | BI tools, `psql`, DBeaver; SELECT, DML, and DDL |
| NoSQL HTTP (`/nosql/*`) | Read-only; writes `Intentionally absent` | O(1) document fetch and prefix scans |

See [Choosing an interface](../guide/building-apps.md#choosing-an-interface) for the full promise matrix and auth/setup requirements.

## Shared Application Contract

Both the server and embedded (`DoogatDriver`) paths delegate typed SQL execution to the same `SqlEngine` in `ddb-core`. The server actor constructs `SqlEngine::new(index, repo)` per command; `DoogatDriver` does the same per `execute_sql` call. This ensures identical semantics for single statements: DDL creates typedef doogats via Git, DML reads/writes Git-backed doogats.

**Transaction difference**: The embedded path (`DoogatDriver`) supports multi-statement transactions via `begin_transaction`/`commit_transaction`/`rollback_transaction`, which suspend and resume a `TransactionBuffer` across calls. The server path creates a fresh `SqlEngine` per `executeSql` command, so BEGIN/COMMIT/ROLLBACK cannot span multiple GraphQL calls. For atomic multi-statement execution over GraphQL, use `executeBatch(statements: [...])` which joins statements and executes them through the service's batch path.

See [FFI Bindings](./ffi.md) for the embedded side of this contract.

## Shared transport glue

Adapter policy that more than one transport needs lives in one place. Each
helper is consumed by every transport that needs it, so the same input produces
the same behavior across interfaces unless a transport documents a deliberate
exception.

| Helper | Location | Signature | Consumers |
|--------|----------|-----------|-----------|
| HTTP error mapping | `ddb-server/src/http_error.rs` | `http_error_response(DoogatError) -> (StatusCode, Json<ErrorBody>)` | REST (`rest.rs`), NoSQL HTTP (`nosql_api.rs`) |
| SQL schema-mutation classifier | `ddb-core/src/sql_engine/classify.rs` | `requires_schema_reload(&str) -> bool` | GraphQL `executeSql`/`executeBatch` (`schema/mutations/operations.rs`), PgWire (`pgwire.rs`) |
| GraphQL input decoding | `ddb-server/src/schema/input.rs` | `opt_string`, `opt_string_list`, `string_list`, `fields_map`, `opt_fields_map`, `conflict_action` | mutation resolvers (`schema/mutations/operations.rs`) |

- **HTTP error mapping** turns a `DoogatError` into a status code plus a
  `{ error, message }` body. The `error` field carries the unified code
  vocabulary (`NOT_FOUND`, `VALIDATION_ERROR`, `UNIQUE_VIOLATION`, ...), the same
  codes GraphQL exposes under `extensions.code`. REST and NoSQL HTTP route every
  error through this one helper, so a given `DoogatError` yields the same shape
  on both surfaces.
- **SQL schema-mutation classifier** parses a statement (or batch) with
  `sqlparser` and returns whether a schema reload is required. Only real
  `CREATE TABLE`, `ALTER TABLE`, and `DROP TABLE` trigger a reload; the same DDL
  text inside a string literal or comment (`SELECT 'CREATE TABLE x'`) does not.
  Parse failures are conservative: unparseable custom DDL returns `true`.
  GraphQL and PgWire share this one classifier instead of each re-detecting DDL.
- **GraphQL input decoding** pulls `title`, `content`, `tags`, the `fields` JSON
  blob, `unsetFields`, and `onConflict` out of dynamic resolver argument maps and
  returns domain types (`BTreeMap<String, Value>`, `ConflictAction`).
  `createDoogat`, `createMany`, `updateDoogat`, and `batchUpdate` all decode
  through these helpers, so tag/field/conflict parsing stays consistent across
  the four mutations. `opt_fields_map` preserves batch-update semantics:
  an absent `fields` argument means "leave unchanged", distinct from an empty map.

### Thin adapters

Transport code handles auth, serialization, request parsing, and response
formatting; business policy lives in the service and actor layers. The mutation
resolvers, REST handlers, NoSQL handlers, and PgWire executor decode input,
delegate to the actor or read pool, and serialize the result. The inline logic
that remains in transports is transport-scoped only: pagination clamping and
sort-field validation in REST, mutually-exclusive `?type=`/`?tag=` checks in
NoSQL, and `pg_catalog`/`pg_class` introspection interception in PgWire. None of
it carries domain rules. Validation, conflict resolution, and schema decisions
are all delegated.

## Running

```bash
ddb serve                           # default: HTTP 2891, pgwire 2892
ddb serve --port 8080               # custom HTTP port
ddb serve --pg-port 5432            # custom pgwire port
ddb serve --bind 0.0.0.0            # all interfaces
ddb serve --playground              # enable GraphQL Playground at GET /graphql
```

## Configuration

Server config lives at `~/.config/ddb/config.toml`:

```toml
[server]
port = 2891
pg_port = 2892
bind = "127.0.0.1"
token_file = "/path/to/custom/token"  # optional
read_pool_size = 4                    # default: min(available_parallelism, 4)
```

CLI flags (`--port`, `--pg-port`, `--bind`) override config file values.

## Logging

The server uses `tracing` for structured logging. Default level: `info` for ddb crates, `warn` for dependencies.

```bash
ddb serve                             # default: info level
ddb --log-level debug serve           # debug output (includes HTTP requests)
RUST_LOG=ddb_server=trace ddb serve   # trace a specific crate
ddb --log-dir /var/log/ddb serve      # NDJSON file logging
```

`RUST_LOG` takes precedence over `--log-level`. HTTP request/response tracing (method, path, status, latency) is logged at `debug` level via `tower-http`.

## Health Check

Unauthenticated endpoints for monitoring and orchestration:

| Endpoint | Purpose | Response |
|----------|---------|----------|
| `GET /health` | Readiness (alias for `/health/ready`) | 200 or 503 |
| `GET /health/ready` | Actor alive + index reachable | 200 `{"status":"ok"}` or 503 `{"status":"degraded"}` |
| `GET /health/live` | Process alive | Always 200 `{"status":"ok"}` |

Ready response includes `version`, `uptime_seconds`, and `index_reachable` fields.

## Authentication

On first start, the server generates a UUID v4 token at `~/.config/ddb/token` (chmod 0600 on Unix). All requests must include:

```
Authorization: Bearer <token>
```

Missing or invalid tokens return HTTP 401.

## Schema

The schema has two components: base types (always present) and dynamic types (generated from `_typedef` doogats at startup).

### Base Types

```graphql
type Doogat {
  id: ID!
  title: String
  date: String
  type: String
  tags: [String!]!       # merged frontmatter + body hashtags, deduplicated
  body: String!
  path: String!
  fields: [InlineField!]!
  links: [Link!]!
  attachments: [Attachment!]!
  updated_at: String     # last-indexed timestamp (RFC 3339, set at index time)
  created_at: String     # alias for date (frontmatter creation timestamp)
}

type Attachment { name: String!, mime: String!, size: Int!, url: String! }

type InlineField { key: String!, value: String!, zone: String! }
type Link { target: String!, display: String, zone: String! }
type SearchHit { id: ID!, title: String!, path: String!, snippet: String!, rank: Float!, updated_at: String, tags: [String!]!, type: String, fields: JSON, created_at: String }
type SearchConnection { hits: [SearchHit!]!, totalCount: Int!, queryNormalized: String! }
type TypeDef { name: String!, columns: [ColumnInfo!]!, crdtStrategy: String, templateSections: [String!]! }
type ColumnInfo { name: String!, dataType: String!, zone: String, required: Boolean!, references: String }
type SqlResult { columns: [String!], rows: [String!], affected: Int, message: String }
type TagInfo { name: String!, count: Int! }
type CheckboxItem {
  doogatId: ID!
  doogatTitle: String
  state: String!        # "open", "done", "info"
  content: String!
  date: String
  dueDate: String
  lineNumber: Int
  indentLevel: Int
}
```

Note: `SqlResult.rows` encodes each row as a JSON string to avoid nested list limitations. `SqlResult.columns` returns column names matching the SELECT clause order.

DDL statements (CREATE TABLE, ALTER TABLE, DROP TABLE) return empty `columns` and `rows` arrays with a `message` field containing the created/affected doogat ID or confirmation. DML SELECT returns populated `columns` and `rows`; INSERT/UPDATE/DELETE returns `affected` count and the doogat ID in `message`.

#### Row format option

The `sql`, `executeSql`, and `executeBatch` fields accept an optional `format` argument:

- `format: "array"` (default) - rows are JSON arrays: `["val1", "val2"]`
- `format: "objects"` - rows are JSON objects keyed by column name: `{"id": "val1", "title": "val2"}`

Object format eliminates positional coupling between client code and SELECT column order.

### Queries

```graphql
type Query {
  doogat(id: ID!): Doogat
  doogats(type: String, tag: String, backlinksOf: ID, limit: Int, offset: Int): [Doogat!]!
  search(query: String!, types: [String], tag: String, where: [SearchFieldFilter], limit: Int, offset: Int): SearchConnection!
  normalizeSearchQuery(query: String!): String!
  typeDefs: [TypeDef!]!
  sql(query: String!, format: String): SqlResult!
  schemaVersion: Int!
  checkboxItems(state: String, doogatId: ID, limit: Int, offset: Int): [CheckboxItem!]!
  openActions(limit: Int): [CheckboxItem!]!
  tags: [TagInfo!]!
  tagEntries(where: TagEntriesWhere): TagEntryConnection!
}

type TagEntry {
  doogatId: ID!
  tag: String!
  source: String!
}

type TagEntryConnection {
  items: [TagEntry!]!
  totalCount: Int!
}

input TagEntriesWhere {
  doogatId: StringFilter
  tag: StringFilter
}

input SearchFieldFilter {
  field: String!
  eq: String
  contains: String
  in: [String]
}
```

### Mutations

```graphql
type Mutation {
  createDoogat(input: CreateDoogatInput!, onConflict: ConflictAction): Doogat!
  updateDoogat(input: UpdateDoogatInput!): Doogat!
  batchUpdate(updates: [UpdateDoogatInput!]!): [Doogat!]!
  createMany(inputs: [CreateManyItemInput!]!, onConflict: ConflictAction): [Doogat!]!
  deleteDoogat(id: ID!): Boolean!
  executeSql(sql: String!, format: String): SqlResult!
  executeBatch(statements: [String!]!, format: String): [SqlResult!]!
  attachFile(input: AttachFileInput!): Attachment!
  detachFile(doogatId: ID!, filename: String!): Boolean!
  sync(remote: String, branch: String): SyncResult!
  compact(force: Boolean): CompactResult!
}

input CreateDoogatInput { title: String!, content: String, tags: [String!], type: String }
input CreateManyItemInput { title: String!, content: String, tags: [String!], type: String, fields: String }
input UpdateDoogatInput { id: ID!, title: String, content: String, tags: [String!], type: String, fields: String, unsetFields: [String!] }
input AttachFileInput { doogatId: ID!, filename: String!, dataBase64: String!, mime: String }

enum ConflictAction { ERROR, IGNORE }

type SyncResult {
  direction: String!
  commitsTransferred: Int!
  conflictsResolved: Int!
  resurrected: Int!
  collisionsReassigned: Int!
}

type CompactResult {
  filesRemoved: Int!
  crdtDocsCompacted: Int!
  gcSuccess: Boolean!
}
```

`updateDoogat` accepts optional `fields` (a JSON string of key-value pairs, e.g. `"{\"url\":\"https://example.com\"}"`) to set type-specific frontmatter fields, and `unsetFields` (a list of field names to remove). When the doogat has a type with a typedef, `allowed_values` and foreign-key constraints are validated before the write. The materialized type table row is updated in place after the change.

`batchUpdate` applies multiple updates atomically in a single git commit. All updates must succeed or none are applied. If any ID is not found, the entire batch fails with no changes committed. An empty updates array returns an empty array with no git commit. Reuses `UpdateDoogatInput` so `fields` and `unsetFields` work per-item in a batch.

`deleteDoogat` removes the doogat file, its index entries, and its materialized type table row (if typed). Junction table rows referencing the deleted doogat are cascade-cleaned.

`createDoogat` and `createMany` populate the type-specific materialized table whenever `type` is set. Pre-PRD 00129 only the base `doogats` row was written. Now: an `input.type` referencing an unregistered typedef rejects with `TYPE_NOT_REGISTERED`, an `input.fields` key not in the typedef rejects with `UNKNOWN_FIELD`, and a missing required column with no default rejects with `NOT_NULL_VIOLATION`. PRD 00133 unifies the typed-create pipeline: `createDoogat`, `createMany`, and the CLI / FFI `ddb create` (when targeting a registered typedef) all route through the shared `sql_engine::typed_insert::prepare_typed_insert_validate` helper that the SQL `INSERT` path already used. As a result, REFERENCES column values land in the doogat's reference zone (e.g. `- target:: [[id]]`) rather than frontmatter, FK existence is validated against the *referenced* typedef's materialized table (not the generic `doogats` index), and `allowed_values` constraints fire uniformly across every entry point. The CLI `ddb create` path on *unregistered* types is unchanged — it still permits silent base-only creation, preserving PRD 00129 §T3.

`CreateDoogatInput.title` and `CreateManyItemInput.title` are nullable (PRD 00130 / issue #13). When `title` is omitted on a typedef that declares a `title_template`, the engine renders the title server-side from the template — the same path the SQL `INSERT` already used. Without a template (or for an untyped create), an omitted title rejects with `NOT_NULL_VIOLATION` on the `title` column.

`createMany` creates multiple doogats atomically in a single git commit. All records are created or none (rollback on any failure). Returns created records in input order. The optional `fields` parameter accepts a JSON string of key-value pairs for typed columns (e.g. `"{\"category\":\"books\",\"priority\":\"1\"}"`). When the record has a type with a typedef, column defaults (including `DEFAULT NEXT` auto-increment) are resolved automatically for omitted fields. Allowed-value and foreign-key constraints are validated per record.

Both `createDoogat` and `createMany` accept an optional `onConflict` argument. When set to `IGNORE`, if the new record would violate a `unique_together` constraint on the typedef, creation is skipped and the existing doogat is returned instead. When omitted or set to `ERROR` (default), a unique constraint violation returns an error. The pre-check queries the materialized typedef table directly (e.g. `SELECT id FROM "<type>" WHERE col1 = ? AND col2 = ?`) so it sees the same source of truth the SQL path's `UNIQUE` index uses, regardless of which zone the columns route to. (PRD 00133: pre-PRD-00133 the pre-check joined `_ddb_fields` which only indexes frontmatter, so unique_together silently stopped catching duplicates once typed creates routed TEXT columns to body.) The caller must pass the constrained columns via `fields`.

PRD 00130 / issue #12: `createMany(onConflict: IGNORE)` returns the surviving row's ID (and full payload) at the array index of every skipped duplicate — both for cross-batch conflicts (an existing row in the index) and for intra-batch conflicts (two inputs in the same batch carrying the same unique tuple). Earlier the bulk path returned the rejected/rolled-back ID for intra-batch duplicates, leaving callers with an ID that did not exist anywhere.

`sync` defaults to `remote: "origin"`, `branch: "master"` (override via arguments for repos using a different default branch). Returns an error if no remote is configured.
`compact` defaults to `force: false`. When no node is registered, returns a no-op report (zeros).

### Response warnings (PRD 00154)

Every GraphQL response carries an `extensions.warnings` array alongside the existing `data` / `errors` keys. The array is always present (`[]` when no warnings were collected) so clients can read it unconditionally. Each entry has `code` (stable SCREAMING_SNAKE string) and `message` (human-readable). The structure parallels the REST `warnings` array surfaced by PRD 00147 — same vocabulary across transports. `createDoogat` drains `AppOutput::warnings` from `DoogatService::create` into the response (e.g. omitting `title` on a typedef with a `title_template` surfaces `TITLE_FROM_TEMPLATE`). Other mutations do not yet forward warnings; the infrastructure is in place for incremental rollout. Client handling is advisory.

### SINGLETON typedefs (PRD 00139)

For every typedef declared `singleton: true`, the dynamic schema additionally emits:

```graphql
type Query {
  <type_name>: <TypeName>           # singular field, no args; null when typedef is empty
  # plus the existing plural <type_name>s(where:, orderBy:, limit:, ...)
}

type Mutation {
  update_<type_name>(input: String!): <TypeName>!         # no id arg; rejects with SINGLETON_NOT_FOUND when empty
  upsert_<type_name>(input: String!): UpsertResult!       # returns { id, created }
  # plus the existing createDoogat / updateDoogat (the latter still requires id and works on the singleton row)
}

type UpsertResult { id: ID!, created: Boolean! }
```

Field-name shape mirrors the typedef's snake-case `table_name` (e.g. `app_config` -> `app_config: App_config`, `update_app_config(input:)`, `upsert_app_config(input:)`). When the singular field name collides with another `Query` field, it falls back to `<table_name>_singleton` and emits a `tracing::warn!` at schema-build time. If that fallback also collides, schema build fails with a clear error naming the typedef and both colliding field names rather than silently dropping the singular field. Hyphenated typedef names (e.g. `meeting-minutes`) sanitize to `meeting_minutes` before singleton field-name generation.

Constraint violations on `createDoogat` / typed `INSERT` against a populated SINGLETON typedef carry `extensions.code = "SINGLETON_VIOLATION"` plus structured `table` and `existing_id` context (mirroring the `UNIQUE_VIOLATION` envelope from PRD 00129 §6). `update_<type>` against an empty typedef returns `SINGLETON_NOT_FOUND` with `table` context.

`upsert_<type>` input is a JSON object of typed field values only; it carries no `title`. When the upsert creates the first row, the resolver defaults that row's title to the type name (e.g. `app_config`), since the service typed-create path requires a title.

The plural query field (`<type_name>s`) and `createDoogat` are still generated for SINGLETON typedefs (backward compat); `createDoogat` rejects with `SINGLETON_VIOLATION` once a row exists. ALTER TABLE `SET/DROP SINGLETON` triggers the existing schema reload (any DDL through `executeSql` does so per `mutations.rs::executeSql`); the singular field appears or disappears on the next `schemaVersion` poll.

### Search query syntax

The `search` query passes the `query` string directly to SQLite FTS5 MATCH. This means FTS5's full query syntax is available:

- **AND**: `"rust AND crdt"` - both terms must appear
- **OR**: `"rust OR golang"` - either term matches
- **NOT**: `"rust NOT draft"` - exclude doogats containing a term
- **Quoted phrases**: `"\"conflict resolution\""` - exact phrase match
- **Implicit AND**: `"rust crdt"` (space-separated terms default to AND)

Combine with the `types`, `tag`, and `where` parameters for structured filtering on top of full-text search.

Malformed FTS5 queries (e.g., `"AND AND"`) return a `BAD_REQUEST` error with the message `"invalid search query: ..."`.

### Search where filter resolution

The `where` parameter accepts `[SearchFieldFilter]` with `field`, `eq`, `contains`, and `in` operators. Filters resolve in this order:

1. **Tag**: if `field` is `"tag"`, resolves against the `_ddb_tags` table. Works with both `eq` (exact match) and `contains` (substring match).
2. **Materialized type columns**: if the field matches a column in any materialized type table, resolves against that table. When `types` is also set, only those type tables are checked. When a field exists in multiple type tables, results are UNIONed across all matching tables.
3. **Fallback to `_ddb_fields`**: if the field is not found in any type table, resolves against the generic `_ddb_fields` key-value store (frontmatter extras and inline fields).

Examples:

```graphql
# Filter by materialized column (url on link type table)
{ search(query: "example", where: [{field: "url", eq: "https://example.com"}]) { hits { id } totalCount } }

# Filter by tag via where
{ search(query: "rust", where: [{field: "tag", eq: "svelte"}]) { hits { id } totalCount } }

# Combined: type restriction + materialized column filter
{ search(query: "docs", types: ["link"], where: [{field: "url", contains: "github"}]) { hits { id } totalCount } }

# Set membership: match doogats with any of the listed tags
{ search(query: "rust", where: [{field: "tag", in: ["systems", "performance", "concurrency"]}]) { hits { id } totalCount } }
```

An empty `in: []` produces no matches (the clause evaluates to false).

The dedicated `tag` argument on `search` and the `where` tag filter work independently. Both can be used in the same query.

### Query normalization

The server can normalize search queries to a canonical form so that semantically equivalent queries always produce the same string. This is useful for saved searches, deduplication, and matching.

**`queryNormalized` on SearchConnection**: Every `search()` response includes the normalized form of the input query.

```graphql
{ search(query: "Tag=svelte AND category=work.portals") { queryNormalized totalCount } }
# Returns: queryNormalized = "category=work.portals and tag=svelte"
```

**`normalizeSearchQuery` standalone query**: Normalize a query without executing a search.

```graphql
{ normalizeSearchQuery(query: "B AND A") }
# Returns: "a and b"
```

Normalization rules:

1. Lowercase all terms, field names, values, and operators
2. Collapse whitespace (trim, internal runs to single space)
3. Make implicit AND explicit (`meeting minutes` becomes `meeting and minutes`)
4. Sort AND operands alphabetically by serialized form
5. Preserve OR operand order
6. Normalize NOT and parenthesized subexpressions recursively
7. Preserve internal spaces in quoted field values
8. Invalid queries (unparseable) fall back to lowercase + whitespace collapse

Note: normalization is for canonical comparison, not query rewriting. The normalized form may not be valid FTS5 syntax (e.g., field filters like `tag=svelte` are a normalization-layer concept, not FTS5).

### Enriched search results

Each `SearchHit` includes enriched fields beyond the basic FTS5 result:

- **tags** `[String!]!` - all tags from both frontmatter and body hashtags (always present, may be empty)
- **type** `String` - the doogat's type name, or null for untyped doogats
- **fields** `JSON` - type-specific column values as a JSON object (null if untyped or no columns). Fields come from both frontmatter extras and materialized type tables. Access keys directly: `fields.url`, `fields.description`.
- **created_at** `String` - the date from the doogats table (derived from frontmatter `date:` or the doogat ID)

Example query:

```graphql
{ search(query: "rust") { hits { id title tags type fields created_at } } }
```

For a typed link doogat, `fields` returns `{"url":"https://example.com","description":"Example"}` as a native JSON object - no `JSON.parse()` needed.

**Breaking change:** Clients previously calling `JSON.parse(hit.fields)` should now use `hit.fields` directly.

### Tag entries

The `tagEntries` query returns individual tag-doogat associations from the `_ddb_tags` table. Unlike `tags` (which returns aggregate name+count), `tagEntries` returns each row with `doogatId`, `tag`, and `source` (frontmatter or body).

Filter with `TagEntriesWhere` using `StringFilter` operators (`eq`, `contains`, `in`):

```graphql
# Tags for specific doogats
{ tagEntries(where: { doogatId: { in: ["20260401120000", "20260401120001"] } }) {
    items { doogatId tag source } totalCount
} }

# Tags matching a pattern
{ tagEntries(where: { tag: { contains: "client" } }) {
    items { doogatId tag source } totalCount
} }

# Combined: specific doogat + tag filter
{ tagEntries(where: { doogatId: { eq: "20260401120000" }, tag: { eq: "rust" } }) {
    items { doogatId tag source } totalCount
} }
```

The existing `tags` query (returns `[TagInfo!]!` with `name` and `count`) is unchanged.

### Subscriptions

Real-time push notifications over WebSocket using the `graphql-transport-ws` protocol.

```graphql
type Subscription {
  doogatChanged: DoogatChangeEvent!
  doogatCreated: Doogat!
  doogatUpdated: Doogat!
  doogatDeleted: ID!
  # per-type fields (e.g. contactChanged, bookmarkChanged)
}

type DoogatChangeEvent {
  action: String!    # "created", "updated", "deleted"
  doogat: Doogat     # null for deletions
  doogatId: ID!
}
```

**WebSocket endpoint**: `ws://host:port/ws`

**Authentication**: The `/ws` route is NOT behind the bearer auth middleware. Instead, `ws_handler` supports two auth paths:

1. **Header auth** (native clients): include `Authorization: Bearer <token>` on the HTTP upgrade request. If valid, the session is pre-authenticated and `connection_init` payload is ignored. If invalid, the server returns 401 before upgrade.
2. **Payload auth** (browser clients): omit the `Authorization` header. The server accepts the upgrade, then validates the token from the `connection_init` payload:

```json
{
  "type": "connection_init",
  "payload": {
    "Authorization": "Bearer <token>"
  }
}
```

If the payload token is valid, the server responds with `connection_ack`. If missing or invalid, the server sends an error and closes the connection.

**JavaScript browser example** (using [graphql-ws](https://github.com/enisdenjo/graphql-ws)):

```js
import { createClient } from 'graphql-ws';

const client = createClient({
  url: 'ws://127.0.0.1:2891/ws',
  connectionParams: {
    Authorization: `Bearer ${token}`,
  },
});
```

**Protocol**: Clients connect using the `graphql-transport-ws` subprotocol. Flow:

1. Client sends `connection_init` (with optional `payload` for auth)
2. Server validates auth (header or payload) and responds with `connection_ack`
3. Client sends `subscribe` with the subscription query
4. Server pushes `next` messages as mutations occur
5. Client sends `complete` to unsubscribe

**Event bus**: The actor emits events to a `tokio::sync::broadcast` channel (capacity 256) after successful mutations. Each subscription stream receives events from this bus and filters by kind/type. When no subscribers exist, events are dropped with zero overhead. Slow clients that lag behind the buffer lose events (acceptable for MVP — clients can refetch on reconnect).

**Per-type subscriptions**: For each `_typedef`, a `{typeName}Changed` subscription field is generated (e.g. `contactChanged`). These filter events server-side by `doogat_type`, so clients only receive events for the types they care about.

**Keepalive**: The server sends periodic pings per the `graphql-ws` protocol. If a client doesn't respond to a ping within 30 seconds, the connection is closed. Idle connections survive indefinitely as long as the client responds to pings.

### Dynamic Types

#### Type Name Conventions

Type names containing hyphens (kebab-case) are converted to valid GraphQL identifiers:

| Original name | GraphQL type | Query field | Subscription |
|--------------|-------------|-------------|-------------|
| `contact` | `Contact` | `contacts` | `contactChanged` |
| `category-membership` | `CategoryMembership` | `categoryMemberships` | `categoryMembershipChanged` |
| `saved-search` | `SavedSearch` | `savedSearches` | `savedSearchChanged` |

The original table name is preserved for SQL queries. Column names with hyphens follow the same conversion for GraphQL field names, with the original name used for data extraction.

If two types produce the same GraphQL name after conversion (e.g. `my-type` and `myType` both become `MyType`), the second type is skipped with a warning.

For each `_typedef` doogat (e.g. "project"), the server generates:
- A typed GraphQL object (e.g. `Project`) with native fields from the typedef columns
- A `{Type}Connection` wrapper with `items` and `totalCount`
- A `{Type}Where` input for field-level filtering
- A `{Type}OrderBy` input for sorting
- A `{Type}Aggregate` type for aggregate queries
- A `{Type}AggregateGroup` type for grouped aggregate results
- A per-type query: `projects(where: ProjectWhere, orderBy: ProjectOrderBy, tag: String, limit: Int, offset: Int, distinct: String): ProjectConnection!`
- A per-type aggregate query: `projectsAggregate(where: ProjectWhere, groupBy: String): ProjectAggregate!`

Column type mapping:

| `_typedef` data_type | Zone | GraphQL type |
|---------------------|------|-------------|
| BOOLEAN | frontmatter | `Boolean` |
| INTEGER | frontmatter | `Int` |
| REAL | frontmatter | `Float` |
| TEXT | frontmatter | `String` |
| TEXT | body | `String` (section content) |
| TEXT | reference | `String` (wikilink target) |

#### Relation Resolution

Columns with `REFERENCES` resolve as nested typed objects instead of raw ID strings. For a `category TEXT REFERENCES category` column:

- **Singular field** (`category`): Returns the referenced typed object (e.g., `Category`) with all its fields, or `null` if no reference exists or the target doogat is missing.
- **Plural field** (`categories`): Returns `[Category!]!` - a list of all referenced typed objects from the junction table. Returns an empty list if no references exist.
- **Raw ID scalar** (`category_id`): Returns the raw reference ID as a `String`, or `null` if no reference exists. Useful when you need only the ID without resolving the full object.

For columns with a `_id` suffix (e.g., `link_id TEXT REFERENCES link`), the naming adjusts: `link_id` returns the raw scalar, `link` returns the resolved object, and `links` returns the plural list.

```graphql
# Example: raw scalar + resolved object in one query
{
  bookmarks {
    items {
      category_id          # raw ID string
      category { id label } # resolved Category object
      categories(orderBy: "label", limit: 5) { id label }
    }
  }
}
```

Plural fields accept optional sorting and limiting arguments:

- `orderBy: String` - field name to sort by (e.g., `"label"`, `"title"`)
- `orderDir: String` - `"ASC"` (default) or `"DESC"`
- `limit: Int` - max items to return (applied after sorting)

Sorting and limiting happen in-memory after batch-fetching, which is efficient at personal scale since SQLite is in-process with no network round-trips.

Resolution is single-level only (no recursive nesting). Plural fields batch-fetch all referenced IDs for each parent item in a single call. Singular fields resolve individually. If the target type schema is unknown, the resolver falls back to the base `Doogat` type.

#### Typed accessor on `Doogat` (PRD 00129 §4)

Every registered typedef adds a matching nested accessor to the base `Doogat` GraphQL type. `Doogat.link` resolves to the `Link` typed object when the row's `type` is `link`, and `null` otherwise. The accessor is available on every mutation response (`createDoogat`, `updateDoogat`, `createMany`, `batchUpdate`) and every read path (`doogat(id:)`, `doogats`, nested references) — letting clients pull typed fields in a single round trip:

```graphql
mutation {
  createDoogat(input: {type: "link", title: "x", fields: "{\"url\":\"https://x\"}"}) {
    id title
    link { url description }
  }
}
```

The field name is the camelCased table name (`category-membership` -> `categoryMembership`). Collisions with reserved Doogat fields (`id`, `title`, `type`, `tags`, `body`, etc.) are skipped silently.

Tags are always available on typed connection queries via the `tags` field, populated from the parsed doogat's frontmatter and body hashtags.

The REST API exposes multi-value references via a `references` JSON object on each doogat. Each key maps to an array of referenced IDs:

```json
{
  "id": "20260310120000",
  "url": "https://example.com",
  "references": {
    "category": ["20260310120001", "20260310120002"]
  }
}
```

### Filtering

Each per-type query accepts a `where` argument with field-level filters. Filter types match column data types:

```graphql
input StringFilter { eq: String, neq: String, contains: String, startsWith: String, in: [String] }
input IntFilter    { eq: Int, neq: Int, gt: Int, gte: Int, lt: Int, lte: Int, in: [Int] }
input FloatFilter  { eq: Float, neq: Float, gt: Float, gte: Float, lt: Float, lte: Float, in: [Float] }
input BoolFilter   { eq: Boolean }
input IDFilter     { eq: ID, in: [ID] }
```

Every `{Type}Where` input includes `id: IDFilter`, `title: StringFilter`, and `tags: TagsFilter` in addition to user-defined columns. PRD 00129 §5 added the `tags` filter:

```graphql
input TagsFilter { contains: String, containsAll: [String!], containsAny: [String!] }
```

- `contains: "rust"` — row carries the named tag.
- `containsAll: ["a", "b"]` — row carries every listed tag.
- `containsAny: ["a", "b"]` — row carries at least one listed tag.

```graphql
{ links(where: { tags: { contains: "rust" } }) { items { id title url } } }
```

All three operators are nullable (PRD 00130 / issue #11) — a contains-only call no longer requires the caller to also pass empty `containsAll` / `containsAny` arrays. The resolver rejects two user-input mistakes that earlier silently matched no rows: an empty filter (`tags: {}`) returns `tags filter requires at least one of: contains, containsAll, containsAny`, and an empty array (`containsAll: []` or `containsAny: []`) returns `containsAll cannot be empty` / `containsAny cannot be empty`.

Composes with column filters via the existing AND conjunction. Backed by `EXISTS` against the `_ddb_tags` index, no new storage.

These base fields exist in all materialized type tables, enabling single-record lookups and title searches on any typed query:

```graphql
{ links(where: { id: { eq: "20260401074007" } }) { items { id title url } } }
{ links(where: { title: { contains: "example" } }) { items { id title } } }
```

Where inputs support compound logic with `_and` and `_or`:

```graphql
{ projects(where: {
    status: { eq: "active" },
    _or: [{ priority: { gte: 3 } }, { tags: { contains: "urgent" } }]
  }) { items { id title } totalCount } }
```

All filter values are parameterized (never interpolated into SQL), preventing injection.

### Sorting

Per-type queries accept `orderBy` with column names mapped to `SortOrder` (`ASC`/`DESC`):

```graphql
{ projects(orderBy: { priority: DESC, title: ASC }) {
    items { id title priority } totalCount
  } }
```

### Aggregation

Per-type aggregate queries return `count` plus per-numeric-column `min`/`max`/`avg`/`sum`:

```graphql
{ projectsAggregate(where: { status: { eq: "active" } }) {
    count
    minPriority maxPriority avgPriority sumPriority
  } }
```

Use `groupBy` for per-group aggregates:

```graphql
{ projectsAggregate(groupBy: "status") {
    groups {
      key       # the distinct group value
      count
      minPriority maxPriority
    }
  } }
```

Without `groupBy`, the top-level `count` and numeric fields return a single aggregate row. With `groupBy`, use the `groups` field. The column name must exist in the type schema.

### Distinct

Deduplicate typed query results by a column. Useful for dropdown population:

```graphql
{ projects(distinct: "status") {
    items { status }
    totalCount          # reflects deduplicated count
  } }
```

When `distinct` is set, results are grouped by the specified column and one representative row per unique value is returned. `totalCount` uses `COUNT(DISTINCT col)`. The column name must exist in the type schema (unknown columns are silently ignored).

### Batch Mutations

Execute multiple SQL statements atomically:

```graphql
mutation {
  executeBatch(statements: [
    "INSERT INTO project (title) VALUES ('P1')",
    "INSERT INTO project (title) VALUES ('P2')"
  ]) {
    message
    affected
  }
}
```

Returns one `SqlResult` per statement. Multi-statement batches run in an implicit transaction: if any DML statement fails, all are rolled back. DDL statements (CREATE/DROP/ALTER TABLE) commit to git immediately and are not covered by the implicit transaction. DDL statements trigger schema reload.

### Connection Wrapper

Per-type queries return a Connection type instead of a bare list:

```graphql
type ProjectConnection {
  items: [Project!]!
  totalCount: Int!
}
```

`totalCount` reflects the total matching rows (respecting `where` and `distinct` filters but ignoring `limit`/`offset`), enabling pagination UI.

### Introspection

All schema fields include descriptions visible via standard GraphQL introspection. Clients can discover capabilities, filter behavior, and cascade semantics without external documentation:

```graphql
{ __type(name: "Query") { fields { name description } } }
{ __type(name: "SearchFieldFilter") { inputFields { name description } } }
```

### Hot Schema Reload

The schema updates automatically when types change at runtime. After an `executeSql` mutation containing `CREATE TABLE` or `DROP TABLE`:

1. The mutation triggers a reload signal
2. A background task fetches current type schemas from the actor
3. A new GraphQL schema is built and atomically swapped in via `ArcSwap`
4. In-flight requests finish against the old schema; new requests use the updated one

No server restart is needed. Clients can poll the `schemaVersion` query field to detect when the schema has changed:

```graphql
{ schemaVersion }  # monotonic Int!, starts at 1, increments on each reload
```

## Attachment Downloads

`GET /attachments/{doogat_id}/{filename}` serves raw attachment bytes from the `reference/` directory with the correct `Content-Type` header (detected via `AttachmentInfo::mime_from_filename`). Protected by the same bearer auth middleware. Returns 404 if the file does not exist, 400 if the path contains traversal characters.

## Error Mapping

| DoogatError variant | GraphQL `code` extension | Message |
|---|---|---|
| `NotFound` | `NOT_FOUND` | Original message |
| `Validation` | `VALIDATION_ERROR` | Original message |
| `InvalidPath` | `INVALID_PATH` | Original message |
| `Conflict` | `CONFLICT` | Original message |
| `BadRequest` | `BAD_REQUEST` | Original message |
| `SqlEngine` | `SQL_ERROR` | Original message (user-actionable: syntax errors, unsupported DDL, constraint violations) |
| All others | `INTERNAL_ERROR` | Redacted to `"internal error"` (details logged server-side) |

### Structured error codes (PRD 00129 §6)

A vocabulary of stable machine-readable codes is attached to specific error classes via `extensions.code` and per-code structured fields. The `message` text remains stable for each code so callers still string-matching it keep working — the `code` is additive.

| `code` | When | Extensions |
|---|---|---|
| `UNIQUE_VIOLATION` | typedef-declared `UNIQUE(...)` violated at INSERT/UPDATE | `{ table, columns, values }` |
| `REFERENCES_VIOLATION` | `NOT NULL REFERENCES` parent delete blocked by RESTRICT | `{ table, column, referencing_table, referencing_id }` |
| `NOT_NULL_VIOLATION` | required column missing or set to NULL on INSERT/UPDATE | `{ table, column }` |
| `UNKNOWN_FIELD` | `fields` JSON has a key not declared in the typedef | `{ table, unknown_field }` |
| `TYPE_NOT_REGISTERED` | `createDoogat`/`createMany` `type` references a typedef that doesn't exist | `{ type }` |
| `CASCADE_CYCLE` | `ON DELETE CASCADE` walk would form a cycle | `{ tables }` |

## PostgreSQL Wire Protocol

PgWire is the `Guaranteed` interface for SQL/reporting workflows. It exposes SELECT queries, DDL (CREATE/DROP/ALTER TABLE), and DML (INSERT/UPDATE/DELETE) to any PostgreSQL client — `psql`, DBeaver, BI tools, or Postgres client libraries — without requiring DDB-specific code.

DML over pgwire produces standard PostgreSQL command tags (`INSERT 0 1`, `UPDATE`, `DELETE`) on success. Errors surface as PostgreSQL error messages, **not** the GraphQL `extensions.code` envelope — structured error codes over pgwire are tracked separately (PRD 00139 §T22). For consumers that need machine-readable error codes alongside SQL mutations, prefer GraphQL `executeSql`, which uses the same `SqlEngine` and emits `extensions.code` per the GraphQL error mapping.

### Usage

```bash
psql -h 127.0.0.1 -p 2892 -U ddb -d ddb
# password prompt → paste the auth token from ~/.config/ddb/token
```

Or from any Postgres client library (e.g. `tokio-postgres`, `psycopg2`, `node-postgres`) — connect to `127.0.0.1:2892`, user `ddb`, password = auth token.

### Authentication

Uses PostgreSQL MD5 password authentication. The password is the same bearer token used for HTTP/GraphQL auth (`~/.config/ddb/token`). The username can be anything (conventionally `ddb`).

### DDL propagation

`CREATE TABLE`, `ALTER TABLE`, and `DROP TABLE` statements sent over pgwire fire the same hot schema reload signal as the GraphQL `executeSql` mutation. Both paths return success as soon as the typedef commit lands in Git, **before** the schema reload has been observed by the GraphQL layer.

Reload is asynchronous: the new type becomes addressable over GraphQL on the next `schemaVersion` increment, which typically lands within a few hundred milliseconds but can take up to several seconds under contention. Consumers that need to issue a GraphQL request against a freshly-created type should poll `query { schemaVersion }` until it advances past the value observed before the DDL, instead of assuming immediate availability. See the schema-reload polling pattern below for the canonical wait shape.

### Type encoding

When a SELECT targets a materialized type table (one with a typedef), the pgwire response uses proper PostgreSQL types:

- `BOOLEAN` columns use `BOOL` type (psql displays `t`/`f`)
- `INTEGER` columns use `INT8` type
- `REAL` columns use `FLOAT8` type
- Other columns use `VARCHAR`

Queries against untyped tables (no typedef) return all columns as `VARCHAR`.

### Limitations

- **Text encoding for untyped tables**: columns from tables without typedefs are returned as `VARCHAR`.
- **Simple query protocol only**: no prepared statements or extended query protocol. Most clients default to simple mode for ad-hoc queries.
- **No TLS**: bind to localhost or use an SSH tunnel for remote access.

## Background Maintenance

The server runs periodic maintenance (compaction + stale node detection) in a background tokio task.

### Configuration

```toml
[maintenance]
enabled = true         # default: true
interval_secs = 3600   # default: 3600 (1 hour)
```

Set `enabled = false` to disable. CLI flags don't override maintenance config — edit `~/.config/ddb/config.toml`.

### Behavior

- Spawns on startup if `maintenance_enabled` is true
- Skips the first tick (waits one full interval before first run)
- Calls `compact()` + `detect_stale_nodes()` via `ActorCommand::RunMaintenance`, returns `CompactionReport`
- Also available on demand via the `compact` GraphQL mutation
- Git maintenance available via the `maintenance(task: String)` GraphQL mutation (`ActorCommand::GitMaintenance`), returns `MaintenanceReport`
- Logs at `info` on success, `warn` on failure — maintenance errors are non-fatal
- `run_maintenance` propagates errors (returns `Err` on compaction failure); callers handle this gracefully

## NoSQL REST API

The NoSQL HTTP interface is read-only. All write and mutate operations are `Intentionally absent` — route writes through GraphQL or REST.

When built with the `nosql` feature (enabled by default), the server exposes key-value endpoints at `/nosql/`:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/nosql/:id` | Fetch doogat by ID (O(1) redb lookup) |
| `GET` | `/nosql?type=<type>` | Prefix scan by doogat type |
| `GET` | `/nosql?tag=<tag>` | Prefix scan by tag |
| `GET` | `/nosql/:id/backlinks` | Backlinks for a doogat |

The actor holds an `Option<RedbIndex>` alongside `Index`. Every create/update/delete that touches SQLite also writes to redb (dual-write). The redb index is rebuilt once at startup and kept in sync via dual-writes.

## REST API

The REST interface is `Specialized` — it covers base-doogat CRUD and list/search, but typed mutations (column writes with `allowed_values` enforcement, FK validation, per-type routes) are not `Guaranteed` until per-type REST routes land. Use GraphQL when you need typed mutations or structured error codes. The REST API is backed by the same actor and auth middleware as GraphQL. See [REST API](./rest-api.md) for endpoint details.

## Compatibility and Deprecation

Promise labels (`Guaranteed`, `Specialized`, `Intentionally absent`, `Deprecated`) are defined in [Compatibility and Deprecation](../guide/building-apps.md#compatibility-and-deprecation) in the building-apps guide. Every deprecated behavior below names a replacement; entries flagged "Status: planned, not yet implemented" reference candidate follow-up PRD slugs that have not shipped yet.

### GraphQL

No deprecated behavior. `Specialized` capabilities for GraphQL: bundle export/import (CLI is the canonical workflow; GraphQL exposes the engine for orchestration).

### REST

Deprecated behavior on `/rest/*`:

- **Search error envelope** (`GET /rest/doogats?q=...`): HTTP 4xx + `{ error, message }` JSON. Replacement: AppError envelope shipped by PRD 00147 — REST adapters now map service-layer `AppError` into the unified code vocabulary. The `{ error, message }` envelope shape is unchanged; the `error` field carries the same codes GraphQL exposes under `extensions.code` (e.g. `NOT_FOUND`, `VALIDATION_ERROR`). REST has no `extensions` object — branch on the value of the `error` field using the unified vocabulary.
- **CRUD mutation error envelope** (`POST/PUT /rest/doogats[/:id]`): HTTP 4xx + `{ error, message }`. Replacement: AppError envelope shipped by PRD 00147 — the same service-layer error type now feeds both GraphQL `extensions.code` and the REST envelope.
- **Validation error envelope** (`POST /rest/doogats` invalid): HTTP 400/422 with `error` carrying a short REST-local code string (500 on server errors). Replacement: AppError envelope shipped by PRD 00147 — REST validation errors flow through the same AppError mapping; the unified code lands in REST's `error` field, carrying the same vocabulary GraphQL exposes under `extensions.code`.
- **Typed create/update writes base shape only** (`POST/PUT /rest/doogats` typed payload): request accepted but only base-doogat shape is written; type-specific tables not populated atomically. Replacement: typed-write paths shipped by PRD 00147 — REST adapters now route typed create/update through the unified AppCommand, populating typed tables atomically.
- **No per-type REST route** (`POST/PUT /rest/doogats` typed): typed payloads route through the base endpoint without atomic typed-column population. Replacement: typed-write paths shipped by PRD 00147 — typed REST create/update now lands typed columns atomically through the unified AppCommand.
- **Warnings in HTTP response body**: warnings surfaced as text embedded in the response body, no structured channel. Replacement: REST `warnings` array shipped by PRD 00147 — REST responses now carry a top-level `warnings` array of structured `AppWarning` entries alongside `data`.

`Specialized` capabilities for REST (not deprecated, still binding promise): `Create (typed)`, `Update (typed)`, `Validation errors`, and `Warnings` remain `Specialized` per the promise matrix until PRD 00149 transport thinning removes the legacy code paths and conformance fixtures assert only the AppError shape.

### PgWire

Deprecated behavior on the PostgreSQL wire protocol:

- **No structured warning channel**: warnings are not surfaced over the PgWire protocol; no notice-emission path today. Replacement: candidate `pgwire-structured-errors-v1` follow-up PRD will extend the deferred PRD 00139 §T22 work to map AppError codes and AppWarning entries onto PostgreSQL `ErrorResponse`/`NoticeResponse` messages. Status: planned, not yet implemented.

`Specialized` capabilities for PgWire: `Validation errors` (PostgreSQL error message strings, not `extensions.code`; structured-code envelope deferred per PRD 00139 §T22), `FTS5 search` and `Backlinks` (reachable only via raw SELECT against the FTS5 virtual table / `_ddb_*` tables, not a curated workflow shape).

### NoSQL HTTP

No deprecated behavior. `Specialized` capabilities for NoSQL HTTP: `List / search basics` and `FTS5 search` are limited to prefix scan by `type=` / `tag=`; free-text search is not supported. All write/mutate capabilities are `Intentionally absent` by design (read-only interface). All NoSQL HTTP errors use the shared `{ error, message }` envelope and the same unified code vocabulary REST returns (see "Shared transport glue" above). Request-validation and not-found responses are constructed inline with that shape — `GET /nosql/:id` not-found is HTTP 404 with `error: "NOT_FOUND"` / `message: "doogat not found"` (G-13 closed), and a malformed scan query is HTTP 400 with `error: "BAD_REQUEST"`. Service-layer failures (a `DoogatError` from the actor) route through the shared `http_error_response` helper, so they yield the same status and code REST produces for the same error.

## Crate Structure

```
ddb-server/src/
├── lib.rs           # pub async fn run() entrypoint
├── actor/           # Thread-safe GitRepo+Index bridge
│   ├── mod.rs       # RepoActor, event bus, command dispatch
│   └── handlers.rs  # Command handler implementations
├── schema/          # Dynamic GraphQL schema
│   ├── mod.rs       # Schema builder (query, mutation, subscription)
│   ├── base_types.rs # Value converters, type builders, helpers
│   ├── queries.rs   # Query field resolvers
│   ├── mutations.rs # Mutation field resolvers
│   ├── subscriptions.rs # Subscription field resolvers
│   ├── type_defs.rs # GraphQL type/input/enum definitions
│   ├── input.rs     # Shared GraphQL input decoding helpers (create/update/batch)
│   └── discovery_queries.rs # Discovery query resolvers
├── read_pool.rs     # Semaphore-gated concurrent read dispatch (spawn_blocking)
├── filter.rs        # Filter/sort/aggregate: input types, SQL builders, Connection wrapper
├── events.rs        # DoogatEvent, EventKind, EventBus (broadcast channel)
├── ws.rs            # WebSocket upgrade handler for graphql-ws subscriptions
├── pgwire.rs        # PostgreSQL wire protocol (simple query, MD5 auth, SELECT + DML/DDL routing)
├── reload.rs        # Hot schema reload orchestration (ArcSwap + Notify)
├── rest.rs          # REST API handlers (/rest/doogats CRUD)
├── nosql_api.rs     # NoSQL REST handlers (/nosql/ key-value queries)
├── http_error.rs    # Shared DoogatError → HTTP status + JSON body (REST + NoSQL)
├── maintenance.rs   # Background maintenance loop (compaction + stale detection)
├── auth.rs          # Token generation + Bearer middleware
├── config.rs        # ServerConfig from config.toml
└── error.rs         # DoogatError → GraphQL error mapping
```

The SQL schema-mutation classifier (`requires_schema_reload`) lives in
`ddb-core/src/sql_engine/classify.rs`, shared by the GraphQL and PgWire
transports rather than duplicated per adapter.
