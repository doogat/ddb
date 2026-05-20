# Building Apps with Doogat DB

Doogat DB works as a backend for personal productivity apps. Your data lives in Git-backed Markdown files with full version history, CRDT sync across devices, and SQL/GraphQL access for frontends.

This guide covers data modeling, API access, and two worked examples.

## When to use Doogat DB

Doogat DB fits apps where:

- **You are the sole user** — single-writer, personal data
- **Data portability matters** — your data is Markdown in Git, readable by any tool
- **Multi-device sync is needed** — laptop, phone, tablet, all conflict-free
- **Write volume is moderate** — every mutation is a git commit; aim for ~100s of writes/day, not thousands

Examples: link managers, personal CRMs, reading logs, project trackers, habit trackers, recipe collections, travel planners.

## Architecture overview

```
Frontend (React, Swift, Kotlin, etc.)
    │
    ├─ GraphQL ─── ddb serve (HTTP, port 2891)        ← Mode 1: Server
    │                  │
    │                  └── Actor thread
    │                       ├── GitRepo (storage)
    │                       ├── Index (SQLite FTS5)
    │                       └── SqlEngine (DDL/DML)
    │
    ├─ FFI ─────── DoogatDriver (UniFFI, embedded)     ← Mode 2: Embedded native
    │                  ├── GitRepo (storage)
    │                  ├── Index (SQLite FTS5)
    │                  └── SqlEngine (DDL/DML)
    │
    └─ Host Shell ─ One app, multiple feature modules  ← Mode 3: Mobile host-shell
                       └── shared DoogatDriver
                            ├── GitRepo (one repo)
                            ├── Index (one index)
                            └── SqlEngine
```

**Web/desktop apps**: talk to `ddb serve` over GraphQL.
**Single native apps**: embed `DoogatDriver` via UniFFI (Swift/Kotlin bindings) — same SQL engine, typed CRUD, transactions, and schema discovery as the server, no server process needed.
**Mobile mini-apps**: one host app embedding DoogatDriver with multiple feature modules — see [Mobile mini-apps](#mobile-mini-apps) below.
**CLI scripts**: use `ddb query` and `ddb create` directly.

## Choosing an interface

Doogat DB exposes several network and embedded interfaces. They are not equivalent. The table below tells you which one to use for each integration class.

| Integration class | Use this | Fallback | Notes |
|-------------------|----------|----------|-------|
| **Network app** (web, desktop frontend) | **GraphQL** | REST | GraphQL is the flagship network API. Every CRUD baseline capability is `Guaranteed`. Structured error codes (`extensions.code`), typed mutations, subscriptions via WebSocket. |
| **Embedded / mobile app** | **FFI (`DoogatDriver` via UniFFI)** | GraphQL over local HTTP | In-process Swift/Kotlin bindings; no server process needed. CRUD baseline `Guaranteed` within the Experimental stability envelope. Use the host-shell model for mobile (see below). |
| **CLI automation / scripting** | **CLI (`ddb` binary)** | GraphQL via `curl` + Bearer token | Shell-first: `ddb create`, `ddb query`, `ddb search`, `ddb sync`. Falls back to GraphQL when scripts need machine-readable error codes or structured warnings (CLI emits text/exit-code only). |
| **SQL / reporting** (BI tools, psql, DBeaver) | **PgWire** (port 2892) | GraphQL `executeSql` | Any PostgreSQL client works without DDB-specific code. SELECT, DML (INSERT/UPDATE/DELETE), and DDL (CREATE/ALTER/DROP TABLE) against materialized type tables. DDL triggers the hot schema reload signal — observable readiness over GraphQL may lag by up to a few seconds (poll `schemaVersion`). Errors surface as PostgreSQL messages, not `extensions.code` — use GraphQL `executeSql` when you need structured error codes alongside DML. |
| **REST CRUD/search** | **REST (`/rest/*`)** | GraphQL | Base-doogat CRUD and list/search over standard HTTP. No GraphQL library needed. Typed create/update is `Specialized` (not `Guaranteed`) until per-type REST routes land — use GraphQL when you need typed mutations. |
| **NoSQL document access** | **NoSQL HTTP (`/nosql/*`)** | REST `GET /rest/doogats/:id` | Read-only by design. O(1) document fetch and prefix scan by type or tag. All write/mutate operations are `Intentionally absent` — route writes through GraphQL or REST. |

### What "Specialized" means

`Specialized` means the capability exists but with constraints: a narrower shape, no structured error envelope, or a workflow that differs from the `Guaranteed` form on the primary interface. It is not the same as absent, but it is not a full promise either. When a cell in the table above is `Specialized`, the note explains the gap and points to the recommended alternative.

### Auth and setup

**Server-mode interfaces** (GraphQL, REST, PgWire, NoSQL HTTP) share one setup chain:

1. `ddb init` in the data directory — see [Getting started](getting-started.md).
2. `ddb serve [--port 2891] [--pg-port 2892]` — see [Server docs](../technical/server.md).
3. The server writes a UUID v4 Bearer token to `~/.config/ddb/token` on first start.
4. Pass `Authorization: Bearer <token>` on every HTTP/WebSocket request (GraphQL, REST, NoSQL HTTP).
5. For **PgWire**: connect as user `ddb` with the token as the password (MD5 password auth) — see [Server docs](../technical/server.md).

**Embedded-mode (FFI)**: standard documented setup in [FFI docs](../technical/ffi.md). Link the platform binding (XCFramework on iOS, `.aar` on Android), construct a `DoogatDriver` with the local repo path, and call `executeSql`. No auth — the host app owns the repo in-process.

**CLI**: install `ddb`, run `ddb init`. No auth required — direct repo access.

## Data modeling

> **Always use `CREATE TABLE` via `ddb query` to define types.** Do not create `_typedef` doogats manually - manual creation bypasses CRDT tracking and may cause sync conflicts across devices.

### Entities become tables

Each entity in your app maps to a SQL table, which maps to a `_typedef` doogat, which auto-generates a GraphQL type.

```
SQL table ←→ _typedef doogat ←→ GraphQL type ←→ Markdown files
```

Define schemas with SQL:

```sql
CREATE TABLE bookmark (
  title TEXT NOT NULL,
  url TEXT NOT NULL,
  category TEXT REFERENCES category(id)
);
```

This single statement:
1. Creates a `_typedef` doogat at `ddb/_typedef/{id}.md`
2. Creates a materialized SQLite table for queries
3. Generates a `Bookmark` GraphQL type with a `bookmarks()` query

### Zone mapping

Each column maps to a zone in the doogat Markdown file:

| Zone | Stored as | Best for |
|------|-----------|----------|
| `frontmatter` | YAML field | Scalars: numbers, booleans, dates, short strings |
| `body` | `## Heading` section | Long-form text, notes, descriptions |
| `reference` | `- key:: value` line | Links between entities (FK references, wikilinks) |

**Zone assignment rules** (in priority order):

1. Explicit `zone` in the typedef always wins
2. Otherwise, the SQL type determines the default:

| SQL Type | Default Zone |
|----------|-------------|
| `REFERENCES` column | reference |
| `CHAR`, `CHAR(n)`, `VARCHAR(n≤255)`, `VARCHAR` (no size), `TINYTEXT` | frontmatter |
| `INTEGER`, `REAL`, `BOOLEAN` | frontmatter |
| `ENUM(...)`, `SET(...)` | frontmatter |
| Column with `allowed_values` | frontmatter |
| `VARCHAR(n>255)`, `TEXT`, `MEDIUMTEXT`, `LONGTEXT` | body |
| Everything else | body |

Rule of thumb: if it points somewhere else, it's a reference. If it describes the doogat, it's frontmatter. If it IS the doogat, it's body.

### Relationships

Foreign keys use `REFERENCES`:

```sql
CREATE TABLE category (
  name TEXT NOT NULL,
  panel TEXT REFERENCES panel(id)
);
```

This stores the FK as a wikilink in the reference section:

```markdown
---
- panel:: [[20260301120000]]
```

The SQL engine validates FK targets on INSERT. Backlinks are automatically indexed.

### Constraints

Use SQL `ENUM` and `SET` types for value constraints:

```sql
CREATE TABLE task (
  title TEXT NOT NULL,
  status ENUM('todo', 'doing', 'done') DEFAULT 'todo',
  priority ENUM('low', 'medium', 'high')
);
```

The engine translates `ENUM`/`SET` into `allowed_values` in the typedef YAML (stored as `TEXT` with constraints):

```yaml
columns:
  - name: status
    data_type: TEXT
    zone: frontmatter
    allowed_values: [todo, doing, done]
    default_value: todo
  - name: priority
    data_type: TEXT
    zone: frontmatter
    allowed_values: [low, medium, high]
```

`allowed_values` is enforced on INSERT - invalid values are rejected. `DEFAULT` fills missing columns.

You can also add constraints to existing tables:

```sql
ALTER TABLE task ADD COLUMN tags SET('urgent', 'blocked', 'review');
```

### Changing a column's type

When a declared type becomes too narrow (for example, `VARCHAR(255)` for URLs
that sometimes exceed the cap), migrate the column with
`ALTER TABLE ... ALTER COLUMN ... TYPE`:

```sql
ALTER TABLE link ALTER COLUMN url TYPE TEXT;
ALTER TABLE link ALTER COLUMN url TYPE VARCHAR(2048);
ALTER TABLE numeric ALTER COLUMN score TYPE REAL;
```

Supported conversions:

- **Widening `VARCHAR(N)` → `VARCHAR(M)` where `M ≥ N`**: metadata-only, no
  data scan. The same applies to `CHAR(N) → CHAR(M)` widening.
- **`VARCHAR(N)` / `CHAR(N)` → `TEXT`**: metadata-only, no data scan. Use this
  when the length cap is the problem.
- **Narrowing `VARCHAR(N)` → `VARCHAR(M)` where `M < N`, or `TEXT → VARCHAR`,
  or `CHAR(N) → CHAR(M)` where `M < N`**: runs a pre-flight scan. If any
  existing row exceeds the new limit, the statement fails with
  `cannot narrow <table>.<column> to <new_type>: <n> existing rows exceed
  limit`. Widen the problem rows or DELETE them first.
- **`INTEGER` ↔ `REAL`**: scans every existing value. Fractional values fail
  when narrowing to `INTEGER`; non-numeric values are also rejected.

`CHAR` and `VARCHAR` are different families (CHAR is fixed-width with padding
semantics) and cross-family conversions are rejected. Migrate via a temporary
column when you need to change family.

`REFERENCES` columns only accept widening within the same family or to `TEXT`.
Other type changes are rejected to keep the foreign-key target stable.

The `SET DATA TYPE` form is also accepted (`ALTER COLUMN url SET DATA TYPE
TEXT`). Both forms are identical in effect.

Out of scope for v1: `BOOLEAN` conversions, cross-category conversions needing
data rewrites (e.g. `TEXT → INTEGER` where some strings are non-numeric), and
changing `NOT NULL`/`DEFAULT`/`REFERENCES` alongside the type. For those,
migrate via a temporary column + `UPDATE` + `DROP COLUMN`.

### Body sections for rich content

Use `template_sections` to define expected body headings. Note: `template_sections` must be set by editing the typedef YAML directly - there is no SQL DDL syntax for this yet.

```yaml
template_sections:
  - Description
  - Notes
```

A doogat of this type will have:

```markdown
---
id: 20260301120000
title: My Record
type: task
status: todo
---

## Description

Task description here.

## Notes

Additional notes.

---
- assignee:: [[20260101000000]]
```

Body sections are stored as `TEXT` columns in the body zone, queryable via SQL and exposed in GraphQL.

### Title resolution

By default, a doogat's title comes from the `title` frontmatter field. For typed doogats, you can set a **title template** that auto-generates titles from column values:

```sql
ALTER TABLE contact SET TITLE TEMPLATE '{name} ({relationship})';
```

Template syntax: `{column_name}` placeholders are interpolated from the row's column values. Unfilled placeholders (missing values) are stripped automatically.

**Dereferencing REFERENCES columns.** When a column is declared `REFERENCES <target_type>`, use the dotted form `{column.field}` to reach through the reference and pull `field` off the target doogat. `field` can be the target's `title` or any typed column on the target's typedef.

```sql
CREATE TABLE link (url TEXT);
CREATE TABLE category (fqn TEXT);
CREATE TABLE "category-membership" (
  link TEXT REFERENCES link,
  category TEXT REFERENCES category
);
ALTER TABLE "category-membership"
  SET TITLE TEMPLATE '{link.title} in {category.fqn}';
```

Inserting a membership composes the title from the referenced doogats:

```sql
INSERT INTO "category-membership" (link, category)
VALUES ('20260101000000', '20260102000000');
-- title becomes "My Link in Work/Jink"
```

Rules:

- Only one hop is supported. `{a.b.c}` is rejected at typedef materialization.
- Bare `{col}` on a REFERENCES column keeps its existing behavior and substitutes the raw id.
- Typedefs with a bad dotted path (column not found, column not REFERENCES, field missing on target) are rejected when the template is applied.
- At runtime, a missing target row or NULL target field substitutes the empty string. The INSERT still succeeds.
- Title is recomputed on `UPDATE` when the SET list touches any column referenced by the template. Cascading re-title when the **target** doogat's field changes is out of scope; stale junction titles must be fixed via `ddb fix` or a follow-up `UPDATE`.

Remove a template:

```sql
ALTER TABLE contact DROP TITLE TEMPLATE;
```

`ddb fix` detects doogats whose titles don't match their type's template and offers to correct them.

> **Breaking change (unreleased):** the silent title fallback (url/description) has been removed. If `title` is `NOT NULL` and no `title_template` is set, an INSERT without an explicit `title` is rejected. Choose one: provide explicit titles, declare a `title_template`, or make `title` nullable.

### Zone overrides

Override the default zone for any column:

```sql
ALTER TABLE note SET ZONE body FOR summary;
```

This moves `summary` from its inferred default zone into the body zone. Available zones: `frontmatter`, `body`, `reference`.

After changing a zone, existing doogats need migration to move data to the new zone:

```bash
ddb fix --migrate
```

### Multi-valued references

When a `CREATE TABLE` includes a `REFERENCES` column, the engine auto-creates a **junction table** named `{type}_{column}` for storing multiple references:

```sql
CREATE TABLE bookmark (
  title TEXT NOT NULL,
  url TEXT NOT NULL,
  category TEXT REFERENCES category(id)
);
-- Auto-creates junction table: bookmark_category
```

**Insert** a reference:

```sql
INSERT INTO bookmark_category (bookmark_id, category_id)
VALUES ('20260301120200', '20260301120100');
```

This appends a `- category:: [[20260301120100]]` line to the bookmark's reference section.

**Delete** a reference:

```sql
DELETE FROM bookmark_category
WHERE bookmark_id = '20260301120200' AND category_id = '20260301120100';
```

**Query** references:

```sql
-- Display view (comma-separated IDs)
SELECT id, title, category FROM bookmark;

-- Relational query (JOIN for filtering)
SELECT b.title, c.name
FROM bookmark b
JOIN bookmark_category bc ON bc.bookmark_id = b.id
JOIN category c ON c.id = bc.category_id;
```

Dropping a table cascades to its junction tables.

### Required foreign keys (RESTRICT)

A column declared `NOT NULL REFERENCES other(id)` blocks the parent's delete: any row currently pointing at the parent will keep it alive. The error names the blocking table, column, and child row id, so client code can resolve the dependency before retrying:

```sql
CREATE TABLE link (url TEXT NOT NULL);
CREATE TABLE "category-membership" (
  link_id     VARCHAR(255) NOT NULL REFERENCES link(id),
  category_id VARCHAR(255) NOT NULL REFERENCES category(id),
  UNIQUE(link_id, category_id)
);

-- Fails: "cannot delete '<link-id>': NOT NULL REFERENCES from
--         category-membership.link_id in row '<membership-id>'"
DELETE FROM link WHERE id = '<link-id>';

-- Works: remove the membership first, then the link.
DELETE FROM "category-membership" WHERE link_id = '<link-id>';
DELETE FROM link WHERE id = '<link-id>';
```

Nullable `REFERENCES` columns keep the existing cascade (the wikilink is stripped and the parent is deleted). Use `NOT NULL REFERENCES` only when a missing parent should be treated as schema corruption.

## API access

### GraphQL

Start the server:

```bash
ddb serve                    # localhost:2891
ddb serve --playground       # enables GraphQL Playground at GET /graphql
```

Authenticate with the bearer token (auto-generated at `~/.config/ddb/token`):

```bash
curl -H "Authorization: Bearer $(cat ~/.config/ddb/token)" \
     -H "Content-Type: application/json" \
     -d '{"query": "{ bookmarks { id, title, url } }"}' \
     http://localhost:2891/graphql
```

#### Auto-generated queries

For each type, the server generates a typed query:

```graphql
# From CREATE TABLE bookmark (...)
query {
  bookmarks(tag: String, limit: Int, offset: Int): [Bookmark!]!
}

type Bookmark {
  id: ID!
  title: String!
  body: String!
  tags: [String!]!
  # ... typed fields from columns
  bookmarkTitle: String    # frontmatter TEXT
  url: String              # frontmatter TEXT
  category: Category       # singular: resolved referenced object (nullable)
  categories: [Category!]! # plural: all referenced objects
}
```

#### Mutations

Use the generic mutations or SQL passthrough:

```graphql
mutation {
  # Generic doogat creation
  createDoogat(input: { title: "My Link", type: "bookmark", tags: ["dev"] }) {
    id
  }

  # SQL for typed inserts (richer column control)
  executeSql(sql: "INSERT INTO bookmark (title, url, category) VALUES ('Rust Book', 'https://doc.rust-lang.org/book/', '20260301120000')") {
    message
  }
}
```

#### Complex queries via SQL passthrough

```graphql
query {
  sql(query: "SELECT c.name, COUNT(b.id) as count FROM category c LEFT JOIN bookmark b ON b.category = c.id GROUP BY c.id ORDER BY count DESC") {
    columns
    rows
  }
}
```

`columns` returns the column names as a string array (e.g. `["name", "count"]`). `rows` returns each row as a JSON string.

#### Core fields in type tables

Materialized type tables include `title`, `date`, and `updated_at` columns automatically, so queries like `SELECT title, url FROM bookmark` work without joining the `doogats` table.

#### Boolean columns

Boolean columns are stored as `1`/`0` integers. Use `WHERE pinned = 1` (not `WHERE pinned = 'true'`). If upgrading from a previous version, run `ddb reindex` to convert existing `"true"`/`"false"` strings.

#### Distinct values

Deduplicate typed query results by a column. Useful for populating dropdowns:

```graphql
query {
  categories(distinct: "space") {
    items { space }
    totalCount          # reflects deduplicated count
  }
}
```

Combine with `where` to filter before deduplication:

```graphql
query {
  bookmarks(distinct: "category", where: { pinned: { eq: 1 } }) {
    items { category { name } }
    totalCount
  }
}
```

#### Grouped aggregates

Get per-group counts and numeric aggregates with `groupBy`:

```graphql
query {
  bookmarksAggregate(groupBy: "status") {
    groups {
      key         # the group value (e.g. "active", "archived")
      count
      minPriority
      maxPriority
    }
  }
}
```

Without `groupBy`, the aggregate query returns a single row as before:

```graphql
query {
  bookmarksAggregate { count }   # total count, no grouping
}
```

#### Batch mutations (atomic)

Execute multiple SQL statements in one call. **All DML statements run in an implicit transaction** - if any statement fails, every preceding statement is rolled back. No partial state.

```graphql
mutation {
  executeBatch(statements: [
    "INSERT INTO bookmark (title, url) VALUES ('Link 1', 'https://one.com')",
    "INSERT INTO bookmark (title, url) VALUES ('Link 2', 'https://two.com')"
  ]) {
    message
    affected
  }
}
```

Transaction rules:
- **DML** (INSERT, UPDATE, DELETE): wrapped in implicit BEGIN/COMMIT. Failure at any point rolls back all prior statements.
- **DDL** (CREATE/DROP/ALTER TABLE): commits to git immediately and is not covered by the implicit transaction. DDL triggers a schema reload.
- **Explicit transactions**: if your batch includes `BEGIN`/`COMMIT`, the implicit transaction is skipped and you manage it yourself.

The same atomicity applies to multi-statement strings passed to `ddb query` and `DoogatDriver.executeSql()` in embedded mode.

### CLI

```bash
# Define schema
ddb query "CREATE TABLE bookmark (title TEXT NOT NULL, url TEXT NOT NULL)"

# Insert data
ddb query "INSERT INTO bookmark (title, url) VALUES ('Rust Book', 'https://doc.rust-lang.org/book/')"

# Query
ddb query "SELECT id, title, url FROM bookmark"

# Full-text search across all doogats
ddb search "rust programming"
```

### UniFFI (mobile)

Embed Doogat DB directly in Swift or Kotlin. The embedded API delegates to the same `SqlEngine` as `ddb serve` — DDL creates typedef doogats via Git, DML reads/writes Git-backed doogats, and SELECT returns typed rows.

```swift
let driver = try DoogatDriver.createRepo(repoPath: "/path/to/ddb")

// Schema — same DDL as server
try driver.executeSql("CREATE TABLE contact (name TEXT, email TEXT)")

// Insert — returns created doogat IDs
let ins = try driver.executeSql(
    "INSERT INTO contact (name, email) VALUES ('Alice', 'alice@example.com')"
)

// Query — returns SqlResultRecord with columns + rows
let contacts = try driver.executeSql("SELECT name, email FROM contact")
for row in contacts.rows {
    print("\(row[0]): \(row[1])")
}

// Transactions — buffer writes, commit as single Git commit
try driver.beginTransaction()
try driver.executeSql("INSERT INTO contact (name, email) VALUES ('Bob', 'bob@example.com')")
try driver.executeSql("UPDATE contact SET email = 'alice@new.com' WHERE name = 'Alice'")
try driver.commitTransaction()

// Type discovery — bootstrap app screens from schema metadata
let schemas = try driver.listTypeSchemas()
for schema in schemas {
    print("\(schema.tableName): \(schema.columns.map { $0.name })")
}
```

No server process needed. The app owns the git repo directly. See [FFI docs](../technical/ffi.md) for the full API surface.

## Mobile mini-apps

### Why not separate apps?

Mobile platforms do not support multiple independently installed apps sharing one local backend:

- **iOS**: apps are sandboxed; no shared filesystem, no `localhost` IPC between apps, background processes are killed aggressively
- **Android**: apps have private storage; `localhost` servers are killed by Doze mode and app standby; cross-app IPC requires explicit permissions and trust

Running `ddb serve` on a phone and connecting multiple installed apps to it is not portable and not supported.

### The host-shell model

The recommended mobile architecture is one installed app containing:

- One embedded Doogat DB core (`DoogatDriver` via UniFFI)
- One shared repository and index
- Multiple feature modules that feel like mini-apps
- Optional widgets and extensions bound to the same shared data

Users get the UX of several mini-apps. The OS sees one well-behaved app.

### iOS shape

- One main app target with SwiftUI
- Feature modules as Swift packages or local frameworks
- Optional widgets and extensions (WidgetKit, Share Extension)
- App Group storage for shared repo/index when extensions need access
- UniFFI-generated Swift bindings imported by the app and extensions

### Android shape

- One main application package with Jetpack Compose
- Feature modules as Gradle modules (`:feature-bookmarks`, `:feature-contacts`, etc.)
- Optional widgets (AppWidgetProvider) and services
- App-private storage, shared across modules within the same process
- UniFFI-generated Kotlin bindings inside the app

### Mini-app contract

Each feature module contributes:

- **Schema**: table definitions via `CREATE TABLE` (applied at app startup)
- **Queries/mutations**: SQL or typed CRUD calls through the shared `DoogatDriver`
- **UI**: screens, navigation destinations, local view state
- **Optional surfaces**: dashboard widgets, share extensions, shortcuts

Each module does **not** own:

- Its own storage engine or repo copy
- Its own local backend daemon
- Its own incompatible backend semantics

### Shared schema bootstrap

On app launch, the host shell initializes `DoogatDriver` once, then each module registers its tables:

```swift
// iOS example
let driver = try DoogatDriver.createRepo(repoPath: appGroupRepoPath)

// Each module bootstraps its schema (idempotent)
try driver.executeSql(sql: "CREATE TABLE IF NOT EXISTS category (name TEXT NOT NULL)")
try driver.executeSql(sql: "CREATE TABLE IF NOT EXISTS bookmark (title TEXT NOT NULL, url TEXT NOT NULL, category TEXT REFERENCES category(id))")
try driver.executeSql(sql: "CREATE TABLE IF NOT EXISTS contact (name TEXT NOT NULL, email TEXT)")
```

```kotlin
// Android example
val driver = DoogatDriver.createRepo(repoPath = appPrivateRepoPath)

// Each module bootstraps its schema (idempotent)
driver.executeSql("CREATE TABLE IF NOT EXISTS category (name TEXT NOT NULL)")
driver.executeSql("CREATE TABLE IF NOT EXISTS bookmark (title TEXT NOT NULL, url TEXT NOT NULL, category TEXT REFERENCES category(id))")
driver.executeSql("CREATE TABLE IF NOT EXISTS contact (name TEXT NOT NULL, email TEXT)")
```

`CREATE TABLE IF NOT EXISTS` is idempotent — if the table already exists, it's a no-op.

### Relationship to embedded parity

The host-shell model depends on full embedded API parity between `DoogatDriver` and `ddb serve`.

## Worked example: link dashboard

A personal link dashboard with panels, categories, and bookmarks.

### Schema

```sql
CREATE TABLE panel (
  name TEXT NOT NULL,
  sort_order INTEGER DEFAULT 0
);

CREATE TABLE category (
  name TEXT NOT NULL,
  panel TEXT REFERENCES panel(id)
);

CREATE TABLE bookmark (
  title TEXT NOT NULL,
  url VARCHAR(255) NOT NULL,
  description TEXT,
  status ENUM('active', 'archived') DEFAULT 'active',
  category TEXT REFERENCES category(id)
);
```

Zone assignments: `url` is `VARCHAR(255)` (≤255, frontmatter). `description` is `TEXT` (body). `status` is `ENUM` (frontmatter). `category` has `REFERENCES` (reference zone).

### Sample data

```sql
INSERT INTO panel (name, sort_order) VALUES ('Development', 0);
INSERT INTO panel (name, sort_order) VALUES ('Research', 1);

-- Assume panel IDs are 20260301120000 and 20260301120001
INSERT INTO category (name, panel) VALUES ('Rust', '20260301120000');
INSERT INTO category (name, panel) VALUES ('AI/ML', '20260301120001');

-- Assume category IDs are 20260301120100 and 20260301120101
INSERT INTO bookmark (title, url, category) VALUES ('Rust Book', 'https://doc.rust-lang.org/book/', '20260301120100');
INSERT INTO bookmark (title, url, category) VALUES ('Tokio Tutorial', 'https://tokio.rs/tokio/tutorial', '20260301120100');
```

### Frontend queries

```graphql
# Load all bookmarks with resolved category objects
query {
  bookmarks {
    items {
      id, title, url
      category { id, name }       # singular: resolved object
      categories { id, name }     # plural: list of resolved objects
    }
    totalCount
  }
}

# Search across all bookmarks
query {
  search(query: "rust async") {
    totalCount
    hits { id, title, snippet, rank }
  }
}

# Search with filters: only bookmarks tagged "rust"
query {
  search(query: "async", types: ["bookmark"], tag: "rust") {
    totalCount
    hits { id, title, snippet }
  }
}

# Search with field filter: only active bookmarks
query {
  search(query: "async", where: [{ field: "status", eq: "active" }]) {
    totalCount
    hits { id, title }
  }
}

# Add a bookmark
mutation {
  executeSql(sql: "INSERT INTO bookmark (title, url, category) VALUES ('Serde Docs', 'https://serde.rs', '20260301120100')") {
    message
  }
}
```

### What each bookmark looks like on disk

```markdown
---
id: 20260301120200
title: Rust Book
type: bookmark
date: 2026-03-01
url: https://doc.rust-lang.org/book/
status: active
---

A comprehensive guide to the Rust programming language.

---
- category:: [[20260301120100]]
```

Three zones visible: frontmatter (url, status), body (description - editable in any text editor after creation), references (category wikilink). Editable in any Markdown editor or Obsidian.

## Worked example: personal CRM

Track contacts, life events, and interactions.

### Schema

```sql
CREATE TABLE contact (
  name VARCHAR(255) NOT NULL,
  relationship ENUM('family', 'friend', 'colleague', 'business', 'acquaintance'),
  email VARCHAR(255),
  phone VARCHAR(100)
);

CREATE TABLE life_event (
  event_type ENUM('birthday', 'married', 'graduated', 'moved', 'other') NOT NULL,
  event_date VARCHAR(10),
  contact TEXT REFERENCES contact(id)
);

CREATE TABLE interaction (
  interaction_date VARCHAR(10) NOT NULL,
  location VARCHAR(255),
  contact TEXT REFERENCES contact(id)
);
```

Body section headings are defined in the typedef YAML (no SQL DDL for this yet):

```yaml
template_sections:
  - Bio
  - Notes
```

### Sample data

```sql
INSERT INTO contact (name, relationship, email) VALUES ('Alice Chen', 'friend', 'alice@example.com');
INSERT INTO contact (name, relationship) VALUES ('Bob Smith', 'colleague');

-- Assume contact IDs are 20260301130000 and 20260301130001
INSERT INTO life_event (event_type, event_date, contact) VALUES ('birthday', '1990-05-15', '20260301130000');
INSERT INTO life_event (event_type, event_date, contact) VALUES ('married', '2024-06-20', '20260301130000');

INSERT INTO interaction (interaction_date, location, contact) VALUES ('2026-02-28', 'Coffee shop', '20260301130000');
```

### Frontend queries

```graphql
# All contacts
query {
  contacts(limit: 50) {
    id, name, relationship, email, phone
  }
}

# Contact's life events and interactions via SQL join
query {
  sql(query: "SELECT le.event_type, le.event_date FROM life_event le WHERE le.contact = '20260301130000' ORDER BY le.event_date") {
    rows
  }
}

# Recent interactions across all contacts
query {
  sql(query: "SELECT c.name, i.interaction_date, i.location FROM interaction i JOIN contact c ON i.contact = c.id ORDER BY i.interaction_date DESC LIMIT 20") {
    rows
  }
}

# Search across everything
query {
  search(query: "alice birthday") {
    id, title, snippet, rank
  }
}
```

### What a contact looks like on disk

```markdown
---
id: 20260301130000
title: Alice Chen
type: contact
date: 2026-03-01
relationship: friend
email: alice@example.com
---

## Bio

Met at RustConf 2024. Software engineer at Acme Corp.

## Notes

Interested in distributed systems and CRDT research.

---
- interaction:: [[20260301130100]]
```

### What an interaction looks like on disk

```markdown
---
id: 20260301130100
title: Coffee catch-up with Alice
type: interaction
date: 2026-02-28
interaction_date: 2026-02-28
location: Coffee shop
---

Talked about CRDT-based apps and the future of local-first software.
She recommended the Ink & Switch essay on local-first.

---
- contact:: [[20260301130000]]
```

Body content (Bio, Notes, free-form text) is added after creation by editing the Markdown file directly or via a frontend text editor.

## Schema design checklist

1. **One table per entity** — panels, categories, bookmarks, contacts, events
2. **Use frontmatter for filterable fields** — dates, enums, booleans, numbers
3. **Use body for rich text** — notes, descriptions, logs
4. **Use references for relationships** — FK columns with `REFERENCES`
5. **Use `allowed_values` for enums** — status, priority, relationship type
6. **Use `default_value` for sensible defaults** — status starts as "todo"
7. **Use `template_sections` for structured body** — consistent headings across records
8. **Keep types small and focused** — more small tables beats fewer bloated ones
9. **Use tags for cross-cutting labels** — tags work across all types
10. **Use search for discovery** — FTS indexes titles, bodies, and tags

## What you get for free

| Feature | How |
|---------|-----|
| Version history | Every mutation is a git commit |
| Offline-first | Works without network, syncs later |
| Multi-device | CRDT resolves conflicts automatically |
| Data portability | Markdown files in a git repo |
| Full-text search | FTS5 with porter stemming |
| Obsidian-compatible | Browse/edit data in any Markdown editor |
| Backups | `git push` to any remote |
| Audit trail | `git log` shows who changed what and when |
