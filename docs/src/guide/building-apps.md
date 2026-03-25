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

Template syntax: `{column_name}` placeholders are interpolated from frontmatter values. Unfilled placeholders (missing values) are stripped automatically.

Remove a template:

```sql
ALTER TABLE contact DROP TITLE TEMPLATE;
```

`ddb fix` detects doogats whose titles don't match their type's template and offers to correct them.

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
  category: String         # reference FK
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
    rows
  }
}
```

`rows` returns each row as a JSON string.

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
# Load all panels with their categories and bookmarks
query {
  panels {
    id, name, sortOrder
  }
  categories {
    id, name, panel
  }
  bookmarks {
    id, title, url, category
  }
}

# Search across all bookmarks
query {
  search(query: "rust async") {
    id, title, snippet, rank
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
