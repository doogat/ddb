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

**Writes** (mutations, INSERTs, DDL) still serialize through the single-writer actor to maintain consistency. The actor bridges sync and async worlds: it runs on `std::thread::spawn` with `blocking_recv()`, while the HTTP layer is fully async (tokio + axum). Communication uses `tokio::sync::mpsc` for commands and `oneshot` channels for replies.

The `sql` query field uses `sqlparser` to classify queries: pure `SELECT` statements route through `ReadPool`, everything else goes to the actor.

## Shared Application Contract

Both the server and embedded (`DoogatDriver`) paths delegate typed SQL execution to the same `SqlEngine` in `ddb-core`. The server actor constructs `SqlEngine::new(index, repo)` per command; `DoogatDriver` does the same per `execute_sql` call. This ensures identical semantics for single statements: DDL creates typedef doogats via Git, DML reads/writes Git-backed doogats.

**Transaction difference**: The embedded path (`DoogatDriver`) supports multi-statement transactions via `begin_transaction`/`commit_transaction`/`rollback_transaction`, which suspend and resume a `TransactionBuffer` across calls. The server path creates a fresh `SqlEngine` per `executeSql` command, so BEGIN/COMMIT/ROLLBACK cannot span multiple GraphQL calls. For atomic multi-statement execution over GraphQL, use `executeBatch(statements: [...])` which joins statements and executes them through the service's batch path.

See [FFI Bindings](./ffi.md) for the embedded side of this contract.

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

`createMany` creates multiple doogats atomically in a single git commit. All records are created or none (rollback on any failure). Returns created records in input order. The optional `fields` parameter accepts a JSON string of key-value pairs for typed columns (e.g. `"{\"category\":\"books\",\"priority\":\"1\"}"`). When the record has a type with a typedef, column defaults (including `DEFAULT NEXT` auto-increment) are resolved automatically for omitted fields. Allowed-value and foreign-key constraints are validated per record.

Both `createDoogat` and `createMany` accept an optional `onConflict` argument. When set to `IGNORE`, if the new record would violate a `unique_together` constraint on the typedef, creation is skipped and the existing doogat is returned instead. When omitted or set to `ERROR` (default), a unique constraint violation returns an error. The pre-check matches field values against the `_ddb_fields` index, so the caller must pass the constrained fields via the `fields` parameter on `CreateManyItemInput`.

`sync` defaults to `remote: "origin"`, `branch: "master"` (override via arguments for repos using a different default branch). Returns an error if no remote is configured.
`compact` defaults to `force: false`. When no node is registered, returns a no-op report (zeros).

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

Every `{Type}Where` input includes `id: IDFilter` and `title: StringFilter` in addition to user-defined columns. These base fields exist in all materialized type tables, enabling single-record lookups and title searches on any typed query:

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

| DoogatError variant | GraphQL `code` extension |
|---|---|
| `NotFound` | `NOT_FOUND` |
| `Validation` | `VALIDATION_ERROR` |
| `InvalidPath` | `INVALID_PATH` |
| `Conflict` | `CONFLICT` |
| `BadRequest` | `BAD_REQUEST` |
| `SqlEngine` | `SQL_ERROR` |
| All others | `INTERNAL_ERROR` |

## PostgreSQL Wire Protocol

The server also speaks the PostgreSQL wire protocol (simple query mode), so standard tools like `psql`, DBeaver, or any Postgres client library can query Doogat DB directly.

### Usage

```bash
psql -h 127.0.0.1 -p 2892 -U ddb -d ddb
# password prompt → paste the auth token from ~/.config/ddb/token
```

Or from any Postgres client library (e.g. `tokio-postgres`, `psycopg2`, `node-postgres`) — connect to `127.0.0.1:2892`, user `ddb`, password = auth token.

### Authentication

Uses PostgreSQL MD5 password authentication. The password is the same bearer token used for HTTP/GraphQL auth (`~/.config/ddb/token`). The username can be anything (conventionally `ddb`).

### DDL propagation

`CREATE TABLE` and `DROP TABLE` statements sent over pgwire trigger the same hot schema reload as the GraphQL `executeSql` mutation. New types become immediately available via GraphQL after creation.

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

When built with the `nosql` feature (enabled by default), the server exposes key-value endpoints at `/nosql/`:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/nosql/:id` | Fetch doogat by ID (O(1) redb lookup) |
| `GET` | `/nosql?type=<type>` | Prefix scan by doogat type |
| `GET` | `/nosql?tag=<tag>` | Prefix scan by tag |
| `GET` | `/nosql/:id/backlinks` | Backlinks for a doogat |

The actor holds an `Option<RedbIndex>` alongside `Index`. Every create/update/delete that touches SQLite also writes to redb (dual-write). The redb index is rebuilt once at startup and kept in sync via dual-writes.

## REST API

In addition to GraphQL, the server exposes a REST API at `/rest/*`. Both interfaces share the same actor backend and auth middleware. See [REST API](./rest-api.md) for endpoint details.

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
│   └── discovery_queries.rs # Discovery query resolvers
├── read_pool.rs     # Semaphore-gated concurrent read dispatch (spawn_blocking)
├── filter.rs        # Filter/sort/aggregate: input types, SQL builders, Connection wrapper
├── events.rs        # DoogatEvent, EventKind, EventBus (broadcast channel)
├── ws.rs            # WebSocket upgrade handler for graphql-ws subscriptions
├── pgwire.rs        # PostgreSQL wire protocol (simple query, MD5 auth, SELECT routing)
├── reload.rs        # Hot schema reload orchestration (ArcSwap + Notify)
├── rest.rs          # REST API handlers (/rest/doogats CRUD)
├── nosql_api.rs     # NoSQL REST handlers (/nosql/ key-value queries)
├── maintenance.rs   # Background maintenance loop (compaction + stale detection)
├── auth.rs          # Token generation + Bearer middleware
├── config.rs        # ServerConfig from config.toml
└── error.rs         # DoogatError → GraphQL error mapping
```
