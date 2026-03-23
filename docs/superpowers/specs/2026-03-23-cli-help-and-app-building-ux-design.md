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
| TEXT | 65535 | body |
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

- Expand `data_type_to_string` to handle new `DataType` variants: `Char`, `Character`, `TinyText`, `MediumText`, `LongText`, `Enum`, `Set`, `Blob`, `TinyBlob`, `MediumBlob`, `LongBlob`, `Binary`, `Varbinary`. Preserve size info for VARCHAR to distinguish short (≤255) vs long (>255).
- Extract ENUM values from `DataType::Enum(Vec<EnumMember>, ...)` — handle both `EnumMember::Name(String)` and `EnumMember::NamedValue(String, Expr)` variants. SET uses `DataType::Set(Vec<String>)` directly.
- Zone inference uses type length threshold: ≤255 chars → frontmatter, >255 → body
- REFERENCES → reference (unchanged)

### 2. Zone Overrides via ALTER TABLE

New DDL for changing a column's zone after creation:

```sql
ALTER TABLE bookmark SET ZONE frontmatter FOR url;
ALTER TABLE bookmark SET ZONE body FOR description;
ALTER TABLE bookmark SET ZONE reference FOR related;
```

**Parsing strategy:** These are custom DDL that `sqlparser` cannot parse. Handle via pre-parse interception: before passing SQL to `sqlparser`, match against `ALTER TABLE <table> SET ZONE <zone> FOR <column>` (and the TITLE TEMPLATE variants from Section 3) with a regex. If matched, dispatch directly to the handler without going through sqlparser's AST. Same approach for `SET TITLE TEMPLATE` and `DROP TITLE TEMPLATE`. The interception lives in `SqlEngine::execute()` before the `sqlparser::parse_sql()` call.

**Behavior:**

1. Validates column exists in the typedef
2. Updates the `zone` field in the _typedef zettel YAML
3. Warns to stderr if moving a column to frontmatter or reference: "Warning: frontmatter/reference zones are best for short values. Long text may hurt readability of the underlying Markdown file."
4. Re-materializes the SQLite table
5. Existing zettels are not migrated — their Markdown stays as-is until next update. `zdb fix --migrate` rewrites them.

**Zone migration via `zdb fix --migrate`:** For each zettel of the affected type, re-derive the Markdown from the current zone assignments. Moving body→frontmatter: parse the `## column_name` section, extract content, add as YAML field (multi-line values use YAML literal block scalars `|`), remove body section. Moving frontmatter→body: reverse. Moving to/from reference: format/parse as `- key:: value` wikilink lines. Sub-headings within a body section are preserved as-is within the section content.

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
| 3 | First body-zone column value | `description TEXT` → "Meeting notes about..." |
| 4 | First frontmatter string column value | `status VARCHAR(50)` → "todo" |
| 5 | `{type} {id}` fallback | "contact 20260301130000" |

**Direct title INSERT with a template defined:** The explicit title is used but flagged as non-compliant. `zdb fix` can auto-repair by re-deriving from the template. Health checks surface these.

**New DDL:**

```sql
ALTER TABLE contact SET TITLE TEMPLATE '{name} ({relationship})';
ALTER TABLE contact DROP TITLE TEMPLATE;
```

**Code change in sql_engine.rs:** In `build_data_zettel()`, before the column loop:
1. Check `col_values.get("title")` — if present, use as title (Priority 1)
2. If not, check `schema.title_template` — if present, interpolate column values (Priority 2)
3. Otherwise, fall through to the existing column loop which sets title from the first body-zone column (Priority 3), then first frontmatter string column (Priority 4)
4. After the loop, if `title_value` is still `None`, fall back to `"{type} {id}"` (Priority 5)

This requires `TableSchema` to carry the `title_template` field (see Section 7).

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

1. Fix the claim that "TEXT defaults to frontmatter" in the zone assignment rules — replace with the SQL type→zone inference table
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
- `TableSchema` uses manual YAML parsing via `schema_from_parsed()`, not serde derive. Both new fields use `.get("title_template").and_then(|v| v.as_str()).map(String::from)` pattern — absent fields naturally resolve to `None`

### 8. Junction Tables for Multi-Valued References

**Problem:** The zettel format naturally supports multiple reference lines for the same key:

```markdown
- category:: [[20260301120100]]
- category:: [[20260301120101]]
```

But the materialized table has a single `category TEXT` column. Only one value survives, losing data. Users are forced to query `_zdb_fields` directly — a leaky internal abstraction.

**Solution:** Every REFERENCES column auto-generates a junction table alongside the main table.

```sql
CREATE TABLE bookmark (
  url VARCHAR(2048),
  category TEXT REFERENCES category(id)
);
```

Produces:
- `bookmark` materialized table — `category` column holds **comma-separated** concatenation of all values (e.g. `"20260301120100,20260301120101"`) for display/simple queries
- `bookmark_category(bookmark_id TEXT, category_id TEXT, PRIMARY KEY(bookmark_id, category_id))` junction table for proper relational access

**Querying:**

```sql
-- Display: shows all categories (concatenated)
SELECT id, url, category FROM bookmark;

-- Relational: proper filtering and JOINs
SELECT b.id, b.url, c.name
FROM bookmark b
JOIN bookmark_category bc ON bc.bookmark_id = b.id
JOIN category c ON c.id = bc.category_id;

-- Filter by specific category
SELECT b.id, b.url
FROM bookmark b
JOIN bookmark_category bc ON bc.bookmark_id = b.id
WHERE bc.category_id = '20260301120100';
```

**Inserting:**

```sql
-- Single reference (populates both main table and junction)
INSERT INTO bookmark (url, category) VALUES ('https://...', '20260301120100');

-- Additional references via junction table
INSERT INTO bookmark_category (bookmark_id, category_id)
VALUES ('20260301120200', '20260301120101');
```

**Indexing from disk:** When a zettel has multiple `- category::` lines, all values go into the junction table. The main table column gets the comma-separated concatenation.

**Impact across all API surfaces:**

| Surface | Change |
|---------|--------|
| **SQL/materialized** | Main table column: comma-separated. Junction table for filtering/JOINs. |
| **GraphQL typed queries** | Keep scalar field (concatenated string). Add list field via junction: `categories: [Category!]!` |
| **GraphQL mutations** | `executeSql` supports INSERT INTO junction table for additional refs |
| **REST JSON** | Replace raw `reference_section` string with structured multi-value fields: `"category": ["20260301120100", "20260301120101"]` |
| **NoSQL JSON** | Same structured field as REST |
| **FFI** | Junction tables queryable via same `execute_sql` SQL interface — no FFI API change |
| **CLI read/get** | No change — raw Markdown naturally shows all `- category::` lines |

**Sync is unaffected** — junction tables are derived index data, rebuilt on reindex like all materialized tables.

**Parser already supports it** — `extract_inline_fields()` in parser.rs already extracts multiple `- key:: value` lines with the same key as separate `InlineField` entries.

**`_zdb_fields` stays internal** — junction tables are the public API for multi-valued references. Documentation and `zdb help create-app` never reference `_zdb_fields`.

**Code changes:**

| File | Change |
|------|--------|
| `zdb-core/src/sql_engine.rs` | On CREATE TABLE with REFERENCES column, also create junction table. On INSERT, populate both main column (concatenated) and junction table. On INSERT INTO junction table directly, validate both IDs exist. |
| `zdb-core/src/indexer/materialize.rs` | `extract_column_value()` for Zone::Reference: collect ALL matching inline_fields, concatenate for main table, insert each into junction table. `create_materialized_table()`: also create junction table DDL. |
| `zdb-server/src/schema/mod.rs` | For REFERENCES columns, generate both scalar field (concatenated) and list field (junction query). |
| `zdb-server/src/schema/base_types.rs` | `zettel_to_value()`: map multi-valued reference fields to arrays. |
| `zdb-server/src/rest.rs` | `zettel_to_json()`: structured multi-value arrays for reference fields instead of raw Markdown. |
| `zdb-server/src/nosql_api.rs` | Same structured output as REST. |
| `zdb-core/src/types.rs` | No struct changes needed — `InlineField` already supports multiple entries per key. |

## Code Locations

| Change | File |
|--------|------|
| Zone inference, ALTER TABLE SET ZONE, ALTER TABLE SET/DROP TITLE TEMPLATE, title cascade, ENUM/SET extraction, origin stamping, junction table DDL, junction INSERT | `zdb-core/src/sql_engine.rs` |
| Schema struct fields (title_template, origin) | `zdb-core/src/types.rs` |
| Help subcommand, after_long_help, typedef warning | `zdb-cli/src/main.rs` |
| Title compliance check, manual typedef flag | `zdb-core/src/indexer/` (fix module) |
| Zone migration on fix --migrate | `zdb-core/src/indexer/` |
| Multi-value reference materialization, junction table population, concatenated main column | `zdb-core/src/indexer/materialize.rs` |
| GraphQL list fields for REFERENCES columns, junction query resolution | `zdb-server/src/schema/mod.rs`, `zdb-server/src/schema/base_types.rs` |
| Structured multi-value JSON for reference fields | `zdb-server/src/rest.rs`, `zdb-server/src/nosql_api.rs` |
| Doc corrections | `docs/src/guide/building-apps.md` |

## Tests

- Zone inference for each SQL type (CHAR, VARCHAR with sizes, TEXT, TINYTEXT, MEDIUMTEXT, LONGTEXT, ENUM, SET, BLOB variants)
- ENUM/SET → allowed_values extraction
- Title cascade: explicit title in INSERT column list produces that title (not auto-derived), template interpolation, first body column, first frontmatter string column, type+id fallback
- Title non-compliance detection when template exists but explicit title used
- ALTER TABLE SET ZONE: valid zone, invalid column, warning on long TEXT to frontmatter
- ALTER TABLE SET/DROP TITLE TEMPLATE
- Typedef origin: ddl via CREATE TABLE, manual via zdb create --type _typedef
- `zdb fix` title compliance repair
- `zdb fix` manual typedef warning
- `zdb fix --migrate` zone rewrites
- Junction table auto-creation on CREATE TABLE with REFERENCES
- Junction table population from multiple `- key::` lines during indexing
- Main table REFERENCES column contains comma-separated concatenation
- INSERT into main table populates junction; INSERT into junction table directly works
- GraphQL list field resolves via junction table query
- REST/NoSQL JSON returns structured arrays for reference fields
- Reindex rebuilds junction tables from zettel Markdown
