# CLI Help and App-Building UX

Improve the app-building experience by fixing zone defaults, title resolution, typedef workflow, and adding in-CLI guidance.

## Problem

Users building apps with zdb hit five traps:

1. **No CLI bridge** — nothing in `zdb --help` points to app-building guidance
2. **Zone confusion** — building-apps.md claims TEXT defaults to frontmatter; code defaults TEXT to body; neither is universally correct
3. **Title overwrite** — SQL INSERT ignores explicit `title` column, auto-derives from first body-zone TEXT column
4. **Typedef trap** — users create _typedef zettels via `zdb create --type _typedef` then hand-edit; CRDT doesn't track manual edits, `zdb fix` strips them
5. **No zone control** — once a column's zone is set via CREATE TABLE, there's no way to change it

## Design

### 1. Zone Inference via SQL Types

Replace the current blanket TEXT→body default with type-aware zone inference based on expected content length.

**Zone semantics:**

| Zone | Semantics | Examples |
|------|-----------|---------|
| frontmatter | Metadata about the content | status, priority, date, name, boolean flags |
| body | The content itself | description, notes, log, bio |
| reference | Pointers outside the zettel | FK to other zettels, URLs, emails, external links |

Rule of thumb: "If it points somewhere else, it's a reference. If it describes the zettel, it's metadata. If it IS the zettel, it's body."

**SQL type to zone mapping:**

| SQL Type | Max Length | Default Zone |
|----------|-----------|--------------|
| CHAR(n), VARCHAR(n≤255), TINYTEXT | ≤255 | frontmatter |
| VARCHAR(n>255) | >255 | body |
| TEXT, TEXT(n>255) | 65535 | body |
| MEDIUMTEXT, LONGTEXT | >64K | body |
| ENUM('a','b','c') | short | frontmatter |
| SET('a','b','c') | short | frontmatter |
| BLOB, TINYBLOB, MEDIUMBLOB, LONGBLOB | binary | body |
| BINARY(n), VARBINARY(n) | binary | body |
| INTEGER, REAL, BOOLEAN | n/a | frontmatter |
| Any column with REFERENCES | n/a | reference |

**ENUM and SET map to _typedef constraints:**

```sql
CREATE TABLE task (
  status ENUM('todo','doing','done') DEFAULT 'todo',
  priority ENUM('low','medium','high')
);
```

Produces:

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

**Code changes in sql_engine.rs:**

- Expand `data_type_to_string` to preserve size info and distinguish short/long string types
- Extract ENUM/SET values into `allowed_values` in the _typedef column definition
- Zone inference uses type length threshold: ≤255 chars → frontmatter, >255 → body
- REFERENCES → reference (unchanged)

### 2. Zone Overrides via ALTER TABLE

New DDL for changing a column's zone after creation:

```sql
ALTER TABLE bookmark SET ZONE frontmatter FOR url;
ALTER TABLE bookmark SET ZONE body FOR description;
ALTER TABLE bookmark SET ZONE reference FOR related;
```

**Behavior:**

1. Validates column exists in the typedef
2. Updates the `zone` field in the _typedef zettel YAML
3. Warns to stderr if moving a column to frontmatter or reference: "Warning: frontmatter/reference zones are best for short values. Long text may hurt readability of the underlying Markdown file."
4. Re-materializes the SQLite table
5. Existing zettels are not migrated — their Markdown stays as-is until next update. `zdb fix --migrate` rewrites them.

### 3. Title Resolution

**_typedef YAML gains optional `title_template`:**

```yaml
title_template: "{name} ({relationship})"
```

Templates interpolate column values by name. Missing values produce empty strings (trimmed).

**Resolution cascade on INSERT:**

| Priority | Source | Example |
|----------|--------|---------|
| 1 | Explicit `title` in INSERT | `INSERT INTO contact (title, name) VALUES ('Dr. Alice', 'Alice')` → "Dr. Alice" |
| 2 | `title_template` in _typedef | `"{name} ({relationship})"` → "Alice Chen (friend)" |
| 3 | First body-zone TEXT column value | `description TEXT` → "Meeting notes about..." |
| 4 | First frontmatter TEXT column value | `url TEXT` → "https://..." |
| 5 | `{type} {id}` fallback | "contact 20260301130000" |

**Direct title INSERT with a template defined:** The explicit title is used but flagged as non-compliant. `zdb fix` can auto-repair by re-deriving from the template. Health checks surface these.

**New DDL:**

```sql
ALTER TABLE contact SET TITLE TEMPLATE '{name} ({relationship})';
ALTER TABLE contact DROP TITLE TEMPLATE;
```

**Code change in sql_engine.rs:** Fix `build_data_zettel()` to check for explicit `title` in INSERT column values before the column loop, instead of always overwriting with the first body TEXT value. Implement the full cascade.

### 4. Typedef Origin Tracking

**_typedef frontmatter gains `origin` field:**

- `origin: ddl` — created via `CREATE TABLE`
- `origin: manual` — created via `zdb create --type _typedef`

```yaml
---
id: 20260301120000
title: bookmark
type: _typedef
origin: ddl
columns:
  ...
---
```

**Runtime warning on `zdb create --type _typedef`:** Emit to stderr:

```
Warning: type definitions should be created with CREATE TABLE via 'zdb query'.
Manual typedefs are not CRDT-tracked and may be stripped by 'zdb fix'.
See: zdb help create-app
```

The create still proceeds — not blocked.

**`zdb fix --verbose` flags manual typedefs:**

```
Warning: _typedef/20260301120000.md (bookmark) has origin: manual.
Consider recreating with: zdb query "CREATE TABLE bookmark (...)"
```

### 5. `zdb help` Subcommand

**Top-level `zdb --help` output gains:**

```
GUIDES:
  help <topic>    In-depth guides (try: zdb help create-app)
```

**`zdb help` with no topic** lists available guides:

```
Available guides:
  create-app    Data modeling, zones, title resolution, and API access
```

**`zdb help create-app`** prints a focused tutorial to stdout:

1. Use `CREATE TABLE`, not manual typedefs
2. Choose SQL types for zone inference (type→zone table, ENUM/SET for constraints)
3. Three-zone mental model (metadata / content / pointers)
4. Title resolution cascade and `title_template`
5. Zone overrides with `ALTER TABLE ... SET ZONE`
6. Quick API examples (CLI, GraphQL, UniFFI)
7. Common mistakes (manual typedef, zone surprise, title overwrite)

**`after_long_help` on existing commands:**

- `zdb query --help`: "For app data modeling, see: zdb help create-app"
- `zdb type --help`: "To define types, use CREATE TABLE via 'zdb query'. See: zdb help create-app"
- `zdb create --help`: "Note: for type definitions, prefer CREATE TABLE via 'zdb query'. See: zdb help create-app"

**Implementation:** Guide text is a `const &str` in the CLI crate, printed directly. No pager, no external dependency.

### 6. Documentation Fixes

**docs/src/guide/building-apps.md:**

1. Fix line 86: replace wrong "TEXT defaults to frontmatter" with the SQL type→zone inference table
2. Replace zone mapping table with the full SQL type → zone table
3. Add ENUM/SET examples to the Constraints section, replacing hand-edited YAML approach
4. Add title resolution section documenting the cascade, `title_template`, and non-compliance
5. Fix worked examples: url → reference zone (via ALTER TABLE), description → body, status → frontmatter
6. Add typedef workflow callout: "Always use CREATE TABLE. Do not create _typedef zettels manually."
7. Add ALTER TABLE SET ZONE and SET TITLE TEMPLATE to zone mapping section
8. Add three-zone mental model explanation (metadata / content / pointers)

### 7. types.rs Changes

- Add `title_template: Option<String>` to typedef schema struct
- Add `origin: Option<String>` to typedef schema struct

## Code Locations

| Change | File |
|--------|------|
| Zone inference, ALTER TABLE SET ZONE, ALTER TABLE SET/DROP TITLE TEMPLATE, title cascade, ENUM/SET extraction, origin stamping | `zdb-core/src/sql_engine.rs` |
| Schema struct fields (title_template, origin) | `zdb-core/src/types.rs` |
| Help subcommand, after_long_help, typedef warning | `zdb-cli/src/main.rs` |
| Title compliance check, manual typedef flag | `zdb-core/src/indexer/` (fix module) |
| Zone migration on fix --migrate | `zdb-core/src/indexer/` |
| Doc corrections | `docs/src/guide/building-apps.md` |

## Tests

- Zone inference for each SQL type (CHAR, VARCHAR with sizes, TEXT, TINYTEXT, MEDIUMTEXT, LONGTEXT, ENUM, SET, BLOB variants)
- ENUM/SET → allowed_values extraction
- Title cascade: explicit title, template, first body TEXT, first frontmatter TEXT, type+id fallback
- Title non-compliance detection when template exists but explicit title used
- ALTER TABLE SET ZONE: valid zone, invalid column, warning on long TEXT to frontmatter
- ALTER TABLE SET/DROP TITLE TEMPLATE
- Typedef origin: ddl via CREATE TABLE, manual via zdb create --type _typedef
- `zdb fix` title compliance repair
- `zdb fix` manual typedef warning
- `zdb fix --migrate` zone rewrites
