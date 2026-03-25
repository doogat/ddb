# CLI Help and App-Building UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix zone inference, title resolution, typedef workflow, add junction tables for multi-valued references, and provide in-CLI app-building guidance.

**Architecture:** Bottom-up — foundation types first, then core engine changes, then API surfaces, then CLI/docs. Junction tables are the most complex piece (parser dedup → materialization → SQL engine write-through → GraphQL/REST).

## Task Dependencies

```
Task 1 (types) ──────────────┬──→ Task 6 (title template DDL)
                              ├──→ Task 7 (title cascade) ──→ Task 11 (consistency)
                              └──→ Task 8 (origin stamping)
Task 2 (parser dedup) ───────┬──→ Task 9 (junction materialization) ──→ Task 10 (junction DML)
                              ├──→ Task 13 (GraphQL list fields)
                              └──→ Task 14 (REST structured JSON)
Task 3 (zone inference) ─────→ Task 4 (ENUM/SET)
Task 5 (ALTER SET ZONE) ─────→ standalone
Task 12 (CLI help) ──────────→ standalone (after all features land)
Task 15 (docs) ──────────────→ standalone (after all features land)
Task 16 (smoke/integration) ─→ after all code tasks
Task 17 (architecture doc) ──→ after all code tasks
Task 18 (walkthrough) ───────→ after Task 16
Task 19 (changelog) ─────────→ last
```

**Tech Stack:** Rust 2021, sqlparser, rusqlite, clap, async-graphql, axum

**Spec:** `docs/superpowers/specs/2026-03-23-cli-help-and-app-building-ux-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `ddb-core/src/types.rs` | Modify | Add `title_template`, `origin` to `TableSchema` |
| `ddb-core/src/parser.rs` | Modify | Remove reference-zone dedup for same-key fields |
| `ddb-core/src/sql_engine.rs` | Modify | Zone inference, ENUM/SET, pre-parse interception, title cascade, origin, junction DDL/DML |
| `ddb-core/src/indexer/materialize.rs` | Modify | Junction table creation/population, multi-value reference extraction |
| `ddb-core/src/consistency.rs` | Modify | Title compliance, manual typedef warning, zone migration |
| `ddb-cli/src/main.rs` | Modify | Help subcommand, after_long_help, typedef warning |
| `ddb-server/src/schema/mod.rs` | Modify | GraphQL list fields for REFERENCES columns |
| `ddb-server/src/schema/base_types.rs` | Modify | Multi-value reference arrays in doogat_to_value |
| `ddb-server/src/rest.rs` | Modify | Structured multi-value JSON for reference fields |
| `docs/src/guide/building-apps.md` | Modify | Fix zone docs, add ENUM/SET, title, junction table sections |

---

### Task 1: Add `title_template` and `origin` to `TableSchema`

**Depends on:** none

**Files:**
- Modify: `ddb-core/src/types.rs:980-987` (TableSchema struct)
- Modify: `ddb-core/src/sql_engine.rs:1445+` (schema_from_parsed)
- Modify: `ddb-core/src/sql_engine.rs:1233-1317` (build_typedef_doogat)
- Test: `ddb-core/src/sql_engine.rs` (test module at bottom)

- [ ] **Step 1: Write test — schema round-trips title_template and origin**

In the sql_engine test module, add a test that creates a typedef doogat with `title_template` and `origin` fields, parses it back via `schema_from_parsed()`, and asserts both fields survive.

```rust
#[test]
fn schema_roundtrips_title_template_and_origin() {
    let typedef = "---\nid: 20260301110000\ntitle: task\ntype: _typedef\norigin: ddl\ntitle_template: \"{name} ({status})\"\ncolumns:\n  - name: name\n    data_type: TEXT\n    zone: frontmatter\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n---\n";
    let parsed = parser::parse(typedef, "ddb/_typedef/20260301110000.md").unwrap();
    let schema = schema_from_parsed(&parsed).unwrap();
    assert_eq!(schema.title_template.as_deref(), Some("{name} ({status})"));
    assert_eq!(schema.origin.as_deref(), Some("ddl"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ddb-core schema_roundtrips_title_template_and_origin`
Expected: FAIL — `title_template` and `origin` fields don't exist on TableSchema

- [ ] **Step 3: Add fields to TableSchema**

In `ddb-core/src/types.rs`, add to the `TableSchema` struct:

```rust
pub title_template: Option<String>,
pub origin: Option<String>,
```

Update all existing `TableSchema` construction sites to include `title_template: None, origin: None`.

- [ ] **Step 4: Update schema_from_parsed to read new fields**

In `ddb-core/src/sql_engine.rs` `schema_from_parsed()`, add after existing field parsing:

```rust
let title_template = map
    .get("title_template")
    .and_then(|v| v.as_str())
    .map(String::from);
let origin = map
    .get("origin")
    .and_then(|v| v.as_str())
    .map(String::from);
```

Wire both into the returned TableSchema.

- [ ] **Step 5: Update build_typedef_doogat to serialize new fields**

In `build_typedef_doogat()`, add `origin` and `title_template` to the frontmatter YAML output when present.

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p ddb-core schema_roundtrips_title_template_and_origin`
Expected: PASS

- [ ] **Step 7: Run full test suite**

Run: `cargo test -p ddb-core`
Expected: All existing tests pass (no regressions from adding Option fields with None defaults)

- [ ] **Step 8: Commit**

```bash
git add ddb-core/src/types.rs ddb-core/src/sql_engine.rs
git commit -m "feat(core): add title_template and origin to TableSchema"
```

---

### Task 2: Remove reference-zone dedup in parser

**Depends on:** none

**Files:**
- Modify: `ddb-core/src/parser.rs:260-315` (extract_inline_fields dedup logic)
- Test: `ddb-core/src/parser.rs` (test module)

- [ ] **Step 1: Write test — multiple same-key reference fields preserved**

Add to parser's test module:

```rust
#[test]
fn multi_value_reference_fields_preserved() {
    let content = "---\nid: 20260301120000\ntitle: test\ntype: bookmark\n---\n\n---\n- category:: [[20260301120100]]\n- category:: [[20260301120101]]\n- category:: [[20260301120102]]\n";
    let parsed = parse(content, "ddb/20260301120000.md").unwrap();
    let cat_fields: Vec<_> = parsed
        .inline_fields
        .iter()
        .filter(|f| f.key == "category")
        .collect();
    assert_eq!(cat_fields.len(), 3);
    assert_eq!(cat_fields[0].value, "20260301120100");
    assert_eq!(cat_fields[1].value, "20260301120101");
    assert_eq!(cat_fields[2].value, "20260301120102");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ddb-core multi_value_reference_fields_preserved`
Expected: FAIL — only 1 field (first-wins dedup)

- [ ] **Step 3: Modify extract_inline_fields to keep all reference-zone duplicates**

In `extract_inline_fields()`, change the reference-zone same-key-same-zone arm from silently discarding to pushing another entry. The `seen` HashMap check for `Some(Zone::Reference)` when current zone is also Reference should push the new InlineField instead of skipping.

Keep the body-zone first-wins behavior and cross-zone error unchanged.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ddb-core multi_value_reference_fields_preserved`
Expected: PASS

- [ ] **Step 5: Run full parser tests + core tests**

Run: `cargo test -p ddb-core`
Expected: All pass. Existing tests that relied on dedup only tested single-value references, so no regressions.

- [ ] **Step 6: Commit**

```bash
git add ddb-core/src/parser.rs
git commit -m "feat(core): preserve multi-value reference fields in parser"
```

---

### Task 3: Expand data_type_to_string and zone inference

**Depends on:** none

**Files:**
- Modify: `ddb-core/src/sql_engine.rs:1156-1168` (data_type_to_string)
- Modify: `ddb-core/src/sql_engine.rs:390-418` (extract_columns zone logic)
- Test: `ddb-core/src/sql_engine.rs` (test module)

- [ ] **Step 1: Write test — VARCHAR(100) infers frontmatter, TEXT infers body**

```rust
#[test]
fn zone_inference_by_sql_type() {
    // Setup: create engine with temp repo
    let (engine, _dir) = test_engine();
    engine
        .execute("CREATE TABLE zonetest (short_str VARCHAR(100), long_str TEXT, num INTEGER, flag BOOLEAN, med MEDIUMTEXT)")
        .unwrap();
    let schema = engine.load_schema("zonetest").unwrap();
    let col = |name: &str| schema.columns.iter().find(|c| c.name == name).unwrap();
    assert_eq!(col("short_str").zone, Some(Zone::Frontmatter));
    assert_eq!(col("long_str").zone, Some(Zone::Body));
    assert_eq!(col("num").zone, Some(Zone::Frontmatter));
    assert_eq!(col("flag").zone, Some(Zone::Frontmatter));
    assert_eq!(col("med").zone, Some(Zone::Body));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ddb-core zone_inference_by_sql_type`
Expected: FAIL — VARCHAR(100) currently maps to TEXT which defaults to Zone::Body

- [ ] **Step 3: Expand data_type_to_string**

Replace the function to handle all variants and return a richer type string that preserves size info:

```rust
fn data_type_to_string(dt: &DataType) -> String {
    match dt {
        DataType::Char(size) | DataType::Character(size) => {
            format!("CHAR({})", size.as_ref().map_or(1, |s| s.length as u32))
        }
        DataType::Varchar(size) | DataType::CharVarying(size) => {
            let n = size.as_ref().map_or(255, |s| s.length as u32);
            format!("VARCHAR({})", n)
        }
        DataType::TinyText => "TINYTEXT".into(),
        DataType::Text => "TEXT".into(),
        DataType::MediumText => "MEDIUMTEXT".into(),
        DataType::LongText => "LONGTEXT".into(),
        DataType::Integer(_) | DataType::Int(_) | DataType::BigInt(_) | DataType::SmallInt(_) => {
            "INTEGER".into()
        }
        DataType::Real | DataType::Float(_) | DataType::Double(_) | DataType::DoublePrecision => {
            "REAL".into()
        }
        DataType::Boolean => "BOOLEAN".into(),
        DataType::Blob(_) | DataType::TinyBlob | DataType::MediumBlob | DataType::LongBlob => {
            "BLOB".into()
        }
        DataType::Binary(_) | DataType::Varbinary(_) => "BINARY".into(),
        DataType::Enum(..) => "TEXT".into(),
        DataType::Set(..) => "TEXT".into(),
        _ => "TEXT".into(),
    }
}
```

- [ ] **Step 4: Add is_short_string_type helper**

```rust
fn is_short_string_type(dt: &DataType) -> bool {
    match dt {
        DataType::Char(_) | DataType::Character(_) | DataType::TinyText => true,
        DataType::Varchar(size) | DataType::CharVarying(size) => {
            size.as_ref().map_or(true, |s| s.length <= 255)
        }
        DataType::Enum(..) | DataType::Set(..) => true,
        _ => false,
    }
}
```

- [ ] **Step 5: Update extract_columns zone inference**

Replace the zone assignment in `extract_columns()`:

Note: `is_numeric_type()` takes `&str` (the stringified type from `data_type_to_string`), not `&DataType`. `is_short_string_type()` takes `&DataType` (the AST node). Both are needed because size info is in the AST but the numeric check works on the string.

```rust
let data_type = data_type_to_string(&col.data_type);
let zone = if references.is_some() {
    Some(Zone::Reference)
} else if is_numeric_type(&data_type) || is_short_string_type(&col.data_type) {
    Some(Zone::Frontmatter)
} else {
    Some(Zone::Body)
};
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p ddb-core zone_inference_by_sql_type`
Expected: PASS

- [ ] **Step 7: Run full test suite**

Run: `cargo test -p ddb-core`
Expected: Some existing tests may need zone expectation updates since TEXT now goes to body but VARCHAR(255) would go to frontmatter. Fix any broken tests to match the new behavior.

- [ ] **Step 8: Commit**

```bash
git add ddb-core/src/sql_engine.rs
git commit -m "feat(core): type-aware zone inference (short string → frontmatter, long → body)"
```

---

### Task 4: Extract ENUM/SET values into allowed_values

**Depends on:** Task 3 (uses `is_short_string_type` for zone inference)

**Files:**
- Modify: `ddb-core/src/sql_engine.rs:390-418` (extract_columns)
- Test: `ddb-core/src/sql_engine.rs` (test module)

- [ ] **Step 1: Write test — ENUM creates allowed_values in typedef**

```rust
#[test]
fn enum_creates_allowed_values() {
    let (engine, _dir) = test_engine();
    engine
        .execute("CREATE TABLE enumtest (status ENUM('todo','doing','done') DEFAULT 'todo', priority ENUM('low','medium','high'))")
        .unwrap();
    let schema = engine.load_schema("enumtest").unwrap();
    let status = schema.columns.iter().find(|c| c.name == "status").unwrap();
    assert_eq!(status.allowed_values.as_deref(), Some(&["todo", "doing", "done"][..]));
    assert_eq!(status.default_value.as_deref(), Some("todo"));
    assert_eq!(status.zone, Some(Zone::Frontmatter));
    let priority = schema.columns.iter().find(|c| c.name == "priority").unwrap();
    assert_eq!(priority.allowed_values.as_deref(), Some(&["low", "medium", "high"][..]));
}
```

Assertion syntax must match `Option<Vec<String>>`:

```rust
assert_eq!(
    status.allowed_values,
    Some(vec!["todo".to_string(), "doing".to_string(), "done".to_string()])
);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ddb-core enum_creates_allowed_values`
Expected: FAIL — ENUM values not extracted

- [ ] **Step 3: Add ENUM/SET extraction to extract_columns**

After determining the data_type string, check if the AST DataType is Enum or Set:

```rust
let allowed_values = match &col.data_type {
    DataType::Enum(members, _) => {
        let vals: Vec<String> = members
            .iter()
            .map(|m| match m {
                sqlparser::ast::EnumMember::Name(n) => n.clone(),
                sqlparser::ast::EnumMember::NamedValue(n, _) => n.clone(),
            })
            .collect();
        Some(vals)
    }
    DataType::Set(vals) => Some(vals.clone()),
    _ => None,
};
```

Wire `allowed_values` into the ColumnDef construction. Ensure ColumnDef has an `allowed_values` field (check if it already does from existing constraint support).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ddb-core enum_creates_allowed_values`
Expected: PASS

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p ddb-core`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add ddb-core/src/sql_engine.rs
git commit -m "feat(core): extract ENUM/SET values into allowed_values"
```

---

### Task 5: Pre-parse interception for ALTER TABLE SET ZONE

**Depends on:** none

**Files:**
- Modify: `ddb-core/src/sql_engine.rs:150-157` (execute method)
- Test: `ddb-core/src/sql_engine.rs` (test module)

- [ ] **Step 1: Write test — ALTER TABLE SET ZONE changes column zone**

```rust
#[test]
fn alter_table_set_zone() {
    let (engine, _dir) = test_engine();
    engine.execute("CREATE TABLE zt (url TEXT, description TEXT)").unwrap();
    // TEXT defaults to body
    let schema = engine.load_schema("zt").unwrap();
    assert_eq!(schema.columns.iter().find(|c| c.name == "url").unwrap().zone, Some(Zone::Body));

    // Move url to frontmatter
    engine.execute("ALTER TABLE zt SET ZONE frontmatter FOR url").unwrap();
    let schema = engine.load_schema("zt").unwrap();
    assert_eq!(schema.columns.iter().find(|c| c.name == "url").unwrap().zone, Some(Zone::Frontmatter));

    // Move description to reference
    engine.execute("ALTER TABLE zt SET ZONE reference FOR description").unwrap();
    let schema = engine.load_schema("zt").unwrap();
    assert_eq!(schema.columns.iter().find(|c| c.name == "description").unwrap().zone, Some(Zone::Reference));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ddb-core alter_table_set_zone`
Expected: FAIL — custom DDL not recognized

- [ ] **Step 3: Add pre-parse regex interception in execute()**

At the top of `execute()`, before passing to `sqlparser::parse_sql()`, add regex matching:

```rust
use regex::Regex;

// Pre-parse interception for custom DDL
let set_zone_re = Regex::new(
    r"(?i)^ALTER\s+TABLE\s+(\w+)\s+SET\s+ZONE\s+(frontmatter|body|reference)\s+FOR\s+(\w+)\s*;?\s*$"
).unwrap();
if let Some(caps) = set_zone_re.captures(sql.trim()) {
    let table = &caps[1];
    let zone = &caps[2];
    let column = &caps[3];
    return self.handle_set_zone(table, zone, column);
}
```

- [ ] **Step 4: Implement handle_set_zone**

New method on SqlEngine:

```rust
fn handle_set_zone(&self, table: &str, zone_str: &str, column: &str) -> Result<SqlResult> {
    let zone = match zone_str.to_lowercase().as_str() {
        "frontmatter" => Zone::Frontmatter,
        "body" => Zone::Body,
        "reference" => Zone::Reference,
        _ => return Err(DdbError::SqlEngine(format!("invalid zone: {zone_str}"))),
    };
    // Load existing schema
    let mut schema = self.load_schema(table)?;
    // Find column
    let col = schema.columns.iter_mut()
        .find(|c| c.name == column)
        .ok_or_else(|| DdbError::SqlEngine(format!("column {column} not found in {table}")))?;
    // Warn if moving to frontmatter/reference
    if matches!(zone, Zone::Frontmatter | Zone::Reference) {
        eprintln!("Warning: frontmatter/reference zones are best for short values. Long text may hurt readability of the underlying Markdown file.");
    }
    col.zone = Some(zone);
    // Update typedef doogat
    self.update_typedef(&schema)?;
    // Rematerialize
    self.index.rematerialize_type(&schema.table_name, &self.repo)?;
    Ok(SqlResult::message(format!("zone updated: {table}.{column} → {zone_str}")))
}
```

There is no existing `update_typedef` method. Create one following the pattern in `handle_alter_table` (lines 777-920) which: (1) serializes the schema back to a typedef doogat via `build_typedef_doogat`, (2) commits the updated file via `self.repo.commit_file(...)`, (3) re-indexes via `self.index.index_doogat(...)`. This method will be reused by Tasks 5, 6, and 8.

Note: `self.index.rematerialize_type()` takes two arguments: `(&self, table_name: &str, repo: &GitRepo)`. Call as `self.index.rematerialize_type(&schema.table_name, &self.repo)?`.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p ddb-core alter_table_set_zone`
Expected: PASS

- [ ] **Step 6: Write test — SET ZONE rejects invalid column**

```rust
#[test]
fn alter_table_set_zone_invalid_column() {
    let (engine, _dir) = test_engine();
    engine.execute("CREATE TABLE zt2 (url TEXT)").unwrap();
    let err = engine.execute("ALTER TABLE zt2 SET ZONE frontmatter FOR nonexistent").unwrap_err();
    assert!(err.to_string().contains("not found"));
}
```

- [ ] **Step 7: Run test, verify it passes**

Run: `cargo test -p ddb-core alter_table_set_zone_invalid_column`
Expected: PASS

- [ ] **Step 8: Run full test suite**

Run: `cargo test -p ddb-core`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add ddb-core/src/sql_engine.rs
git commit -m "feat(core): ALTER TABLE SET ZONE for column zone overrides"
```

---

### Task 6: Pre-parse interception for TITLE TEMPLATE

**Depends on:** Task 1 (`title_template` field on TableSchema)

**Files:**
- Modify: `ddb-core/src/sql_engine.rs` (execute, new handler)
- Test: `ddb-core/src/sql_engine.rs` (test module)

- [ ] **Step 1: Write test — SET/DROP TITLE TEMPLATE**

```rust
#[test]
fn alter_table_title_template() {
    let (engine, _dir) = test_engine();
    engine.execute("CREATE TABLE contact (name VARCHAR(100), relationship VARCHAR(50))").unwrap();

    engine.execute("ALTER TABLE contact SET TITLE TEMPLATE '{name} ({relationship})'").unwrap();
    let schema = engine.load_schema("contact").unwrap();
    assert_eq!(schema.title_template.as_deref(), Some("{name} ({relationship})"));

    engine.execute("ALTER TABLE contact DROP TITLE TEMPLATE").unwrap();
    let schema = engine.load_schema("contact").unwrap();
    assert!(schema.title_template.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ddb-core alter_table_title_template`
Expected: FAIL

- [ ] **Step 3: Add regex interception for TITLE TEMPLATE**

In `execute()`, after the SET ZONE regex:

```rust
let set_template_re = Regex::new(
    r"(?i)^ALTER\s+TABLE\s+(\w+)\s+SET\s+TITLE\s+TEMPLATE\s+'([^']+)'\s*;?\s*$"
).unwrap();
if let Some(caps) = set_template_re.captures(sql.trim()) {
    let table = &caps[1];
    let template = &caps[2];
    return self.handle_set_title_template(table, Some(template));
}

let drop_template_re = Regex::new(
    r"(?i)^ALTER\s+TABLE\s+(\w+)\s+DROP\s+TITLE\s+TEMPLATE\s*;?\s*$"
).unwrap();
if let Some(caps) = drop_template_re.captures(sql.trim()) {
    let table = &caps[1];
    return self.handle_set_title_template(table, None);
}
```

- [ ] **Step 4: Implement handle_set_title_template**

```rust
fn handle_set_title_template(&self, table: &str, template: Option<&str>) -> Result<SqlResult> {
    let mut schema = self.load_schema(table)?;
    schema.title_template = template.map(String::from);
    self.update_typedef(&schema)?;
    let msg = match template {
        Some(t) => format!("title template set: {table} → '{t}'"),
        None => format!("title template removed: {table}"),
    };
    Ok(SqlResult::message(msg))
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p ddb-core alter_table_title_template`
Expected: PASS

- [ ] **Step 6: Run full test suite**

Run: `cargo test -p ddb-core`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add ddb-core/src/sql_engine.rs
git commit -m "feat(core): ALTER TABLE SET/DROP TITLE TEMPLATE"
```

---

### Task 7: Title resolution cascade in build_data_doogat

**Depends on:** Task 1 (`title_template` field), Task 6 (SET TITLE TEMPLATE DDL)

**Files:**
- Modify: `ddb-core/src/sql_engine.rs:1320-1406` (build_data_doogat)
- Test: `ddb-core/src/sql_engine.rs` (test module)

- [ ] **Step 1: Write test — explicit title in INSERT wins over auto-derive**

```rust
#[test]
fn insert_explicit_title_wins() {
    let (engine, _dir) = test_engine();
    engine.execute("CREATE TABLE tittest (description TEXT)").unwrap();
    let result = engine.execute("INSERT INTO tittest (title, description) VALUES ('My Title', 'some long text')").unwrap();
    let id = &result.rows[0]["id"];
    let content = engine.repo.read_doogat(id).unwrap();
    assert!(content.contains("title: My Title"));
}
```

- [ ] **Step 2: Write test — title_template interpolation**

```rust
#[test]
fn insert_title_from_template() {
    let (engine, _dir) = test_engine();
    engine.execute("CREATE TABLE tmpltest (name VARCHAR(100), role VARCHAR(50))").unwrap();
    engine.execute("ALTER TABLE tmpltest SET TITLE TEMPLATE '{name} ({role})'").unwrap();
    let result = engine.execute("INSERT INTO tmpltest (name, role) VALUES ('Alice', 'dev')").unwrap();
    let id = &result.rows[0]["id"];
    let content = engine.repo.read_doogat(id).unwrap();
    assert!(content.contains("title: Alice (dev)"));
}
```

- [ ] **Step 3: Write test — fallback to type+id when no TEXT columns**

```rust
#[test]
fn insert_title_fallback_type_id() {
    let (engine, _dir) = test_engine();
    engine.execute("CREATE TABLE numonly (count INTEGER, active BOOLEAN)").unwrap();
    let result = engine.execute("INSERT INTO numonly (count, active) VALUES (42, true)").unwrap();
    let id = &result.rows[0]["id"];
    let content = engine.repo.read_doogat(id).unwrap();
    assert!(content.contains(&format!("title: numonly {id}")));
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p ddb-core insert_explicit_title_wins insert_title_from_template insert_title_fallback_type_id`
Expected: FAIL

- [ ] **Step 5: Implement title cascade in build_data_doogat**

In `build_data_doogat()`, before the column loop:

```rust
// Priority 1: explicit title from INSERT
let mut title_value = col_values.get("title").cloned();

// Priority 2: title_template interpolation
if title_value.is_none() {
    if let Some(ref tmpl) = schema.title_template {
        let mut rendered = tmpl.clone();
        for (key, val) in &col_values {
            rendered = rendered.replace(&format!("{{{key}}}"), val);
        }
        // Remove unfilled placeholders and clean up
        let rendered = regex::Regex::new(r"\{[^}]+\}").unwrap()
            .replace_all(&rendered, "")
            .trim()
            .to_string();
        if !rendered.is_empty() {
            title_value = Some(rendered);
        }
    }
}
```

In the column loop, change the body-zone title assignment to only set if `title_value.is_none()`:

```rust
Zone::Body => {
    if title_value.is_none() {
        title_value = Some(val.clone());
    }
    body_sections.push(format!("## {}\n\n{}", col.name, val));
}
```

Add Priority 4 (first frontmatter string column) similarly inside the Frontmatter arm.

After the loop, add Priority 5 fallback:

```rust
if title_value.is_none() {
    title_value = Some(format!("{} {}", schema.table_name, id.0));
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p ddb-core insert_explicit_title_wins insert_title_from_template insert_title_fallback_type_id`
Expected: PASS

- [ ] **Step 7: Run full test suite**

Run: `cargo test -p ddb-core`
Expected: PASS (existing INSERT tests may need title expectation updates)

- [ ] **Step 8: Commit**

```bash
git add ddb-core/src/sql_engine.rs
git commit -m "fix(core): title resolution cascade (explicit > template > body > frontmatter > fallback)"
```

---

### Task 8: Typedef origin stamping and CLI warning

**Depends on:** Task 1 (`origin` field on TableSchema)

**Files:**
- Modify: `ddb-core/src/sql_engine.rs:331-388` (handle_create_table)
- Modify: `ddb-cli/src/main.rs` (Create command handler)
- Test: `ddb-core/src/sql_engine.rs` (test module)

- [ ] **Step 1: Write test — CREATE TABLE stamps origin: ddl**

```rust
#[test]
fn create_table_stamps_origin_ddl() {
    let (engine, _dir) = test_engine();
    engine.execute("CREATE TABLE origtest (name VARCHAR(100))").unwrap();
    let schema = engine.load_schema("origtest").unwrap();
    assert_eq!(schema.origin.as_deref(), Some("ddl"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ddb-core create_table_stamps_origin_ddl`
Expected: FAIL

- [ ] **Step 3: Set origin in handle_create_table**

In `handle_create_table()`, after building the TableSchema, set `schema.origin = Some("ddl".to_string())` before passing to `build_typedef_doogat`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ddb-core create_table_stamps_origin_ddl`
Expected: PASS

- [ ] **Step 5: Add typedef warning to CLI**

In `ddb-cli/src/main.rs`, in the Create command handler, after parsing `--type`:

```rust
if r#type.as_deref() == Some("_typedef") {
    eprintln!("Warning: type definitions should be created with CREATE TABLE via 'ddb query'.");
    eprintln!("Manual typedefs are not CRDT-tracked and may be stripped by 'ddb fix'.");
    eprintln!("See: ddb help create-app");
}
```

When creating the doogat with type `_typedef`, add `origin: manual` to the frontmatter extra fields.

- [ ] **Step 6: Run full test suite**

Run: `cargo test -p ddb-core`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add ddb-core/src/sql_engine.rs ddb-cli/src/main.rs
git commit -m "feat(core): stamp origin on typedefs (ddl/manual), warn on manual creation"
```

---

### Task 9: Junction table creation and materialization

**Depends on:** Task 2 (parser multi-value reference support)

**Files:**
- Modify: `ddb-core/src/indexer/materialize.rs` (create_materialized_table, materialize_row, extract_column_value, drop_and_create_materialized_table, rematerialize_type)
- Modify: `ddb-core/src/sql_engine.rs:420-452` (create_materialized_table call)
- Test: `ddb-core/src/sql_engine.rs` (test module)

- [ ] **Step 1: Write test — CREATE TABLE with REFERENCES creates junction table**

```rust
#[test]
fn create_table_creates_junction_table() {
    let (engine, _dir) = test_engine();
    engine.execute("CREATE TABLE category (name VARCHAR(100))").unwrap();
    engine.execute("CREATE TABLE bookmark (url VARCHAR(2048), category TEXT REFERENCES category(id))").unwrap();
    // Junction table should exist in SQLite
    let result = engine.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='bookmark_category'").unwrap();
    assert_eq!(result.rows.len(), 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ddb-core create_table_creates_junction_table`
Expected: FAIL — junction table not created

- [ ] **Step 3: Add junction table creation to materialize.rs**

In `create_materialized_table()` (or the `drop_and_create_materialized_table` variant), after creating the main table, iterate columns and for each with `references.is_some()`:

```rust
for col in &schema.columns {
    if col.references.is_some() {
        let junction = format!(
            "CREATE TABLE IF NOT EXISTS {}_{} ({}_id TEXT NOT NULL, {}_id TEXT NOT NULL, PRIMARY KEY ({}_id, {}_id))",
            schema.table_name, col.name,
            schema.table_name, col.name,
            schema.table_name, col.name
        );
        conn.execute(&junction, [])?;
    }
}
```

Similarly, in `drop_and_create_materialized_table`, drop junction tables before the main table:

```rust
for col in &schema.columns {
    if col.references.is_some() {
        conn.execute(&format!("DROP TABLE IF EXISTS {}_{}", schema.table_name, col.name), [])?;
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ddb-core create_table_creates_junction_table`
Expected: PASS

- [ ] **Step 5: Write test — multi-value references populate junction table**

```rust
#[test]
fn multi_value_refs_populate_junction() {
    let (engine, _dir) = test_engine();
    engine.execute("CREATE TABLE cat (name VARCHAR(100))").unwrap();
    let r1 = engine.execute("INSERT INTO cat (name) VALUES ('work')").unwrap();
    let r2 = engine.execute("INSERT INTO cat (name) VALUES ('personal')").unwrap();
    let cat1 = &r1.rows[0]["id"];
    let cat2 = &r2.rows[0]["id"];

    engine.execute("CREATE TABLE bm (url VARCHAR(2048), category TEXT REFERENCES cat(id))").unwrap();
    let r3 = engine.execute(&format!("INSERT INTO bm (url, category) VALUES ('https://example.com', '{cat1}')")).unwrap();
    let bm_id = &r3.rows[0]["id"];

    // Manually add second reference line to the doogat and reindex
    // (This tests materialization from disk with multi-value refs)
    let content = engine.repo.read_doogat(bm_id).unwrap();
    let updated = content.replace(
        &format!("- category:: [[{cat1}]]"),
        &format!("- category:: [[{cat1}]]\n- category:: [[{cat2}]]"),
    );
    engine.repo.update_doogat(bm_id, &updated).unwrap();
    engine.reindex().unwrap();

    // Junction table should have 2 rows
    let result = engine.execute(&format!("SELECT * FROM bm_category WHERE bm_id = '{bm_id}'")).unwrap();
    assert_eq!(result.rows.len(), 2);

    // Main table should have comma-separated value
    let result = engine.execute(&format!("SELECT category FROM bm WHERE id = '{bm_id}'")).unwrap();
    let cat_val = &result.rows[0]["category"];
    assert!(cat_val.contains(cat1));
    assert!(cat_val.contains(cat2));
}
```

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p ddb-core multi_value_refs_populate_junction`
Expected: FAIL

- [ ] **Step 7: Update extract_column_value and materialize_row for multi-value references**

In `extract_column_value()` for `Zone::Reference`: collect ALL matching inline_fields into a Vec, return comma-separated concatenation.

In `materialize_row()`: after extracting values and inserting the main row, iterate REFERENCES columns and insert each individual value into the junction table.

```rust
// After main row INSERT
for col in &schema.columns {
    if col.references.is_some() {
        let vals = extract_multi_reference_values(doogat, col);
        for val in &vals {
            conn.execute(
                &format!(
                    "INSERT OR IGNORE INTO {}_{} ({}_id, {}_id) VALUES (?1, ?2)",
                    schema.table_name, col.name, schema.table_name, col.name
                ),
                rusqlite::params![id, val],
            )?;
        }
    }
}
```

Add `extract_multi_reference_values` helper that filters `inline_fields` by key and zone.

- [ ] **Step 8: Run test to verify it passes**

Run: `cargo test -p ddb-core multi_value_refs_populate_junction`
Expected: PASS

- [ ] **Step 9: Run full test suite**

Run: `cargo test -p ddb-core`
Expected: PASS

- [ ] **Step 10: Commit**

```bash
git add ddb-core/src/indexer/materialize.rs ddb-core/src/sql_engine.rs
git commit -m "feat(core): auto-generate junction tables for REFERENCES columns"
```

---

### Task 10: Junction table INSERT/DELETE write-through and DROP CASCADE

**Depends on:** Task 9 (junction table creation)

**Files:**
- Modify: `ddb-core/src/sql_engine.rs` (execute, handle_drop_table)
- Test: `ddb-core/src/sql_engine.rs` (test module)

- [ ] **Step 1: Write test — INSERT INTO junction table appends reference line**

```rust
#[test]
fn insert_into_junction_writes_through() {
    let (engine, _dir) = test_engine();
    engine.execute("CREATE TABLE cat (name VARCHAR(100))").unwrap();
    let r1 = engine.execute("INSERT INTO cat (name) VALUES ('work')").unwrap();
    let cat_id = &r1.rows[0]["id"];
    engine.execute("CREATE TABLE bm (url VARCHAR(2048), category TEXT REFERENCES cat(id))").unwrap();
    let r2 = engine.execute("INSERT INTO bm (url, category) VALUES ('https://a.com', 'placeholder')").unwrap();
    let bm_id = &r2.rows[0]["id"];

    // INSERT into junction table
    engine.execute(&format!("INSERT INTO bm_category (bm_id, category_id) VALUES ('{bm_id}', '{cat_id}')")).unwrap();

    // Verify reference line was added to doogat on disk
    let content = engine.repo.read_doogat(bm_id).unwrap();
    assert!(content.contains(&format!("- category:: [[{cat_id}]]")));
}
```

- [ ] **Step 2: Write test — DELETE FROM junction table removes reference line**

```rust
#[test]
fn delete_from_junction_writes_through() {
    let (engine, _dir) = test_engine();
    engine.execute("CREATE TABLE cat2 (name VARCHAR(100))").unwrap();
    let r1 = engine.execute("INSERT INTO cat2 (name) VALUES ('work')").unwrap();
    let cat_id = &r1.rows[0]["id"];
    engine.execute("CREATE TABLE bm2 (url VARCHAR(2048), category TEXT REFERENCES cat2(id))").unwrap();
    let r2 = engine.execute(&format!("INSERT INTO bm2 (url, category) VALUES ('https://b.com', '{cat_id}')")).unwrap();
    let bm_id = &r2.rows[0]["id"];

    // DELETE from junction table
    engine.execute(&format!("DELETE FROM bm2_category WHERE bm2_id = '{bm_id}' AND category_id = '{cat_id}'")).unwrap();

    // Verify reference line was removed from doogat
    let content = engine.repo.read_doogat(bm_id).unwrap();
    assert!(!content.contains(&format!("- category:: [[{cat_id}]]")));
}
```

- [ ] **Step 3: Write test — DROP TABLE cascades to junction tables**

```rust
#[test]
fn drop_table_cascades_junction() {
    let (engine, _dir) = test_engine();
    engine.execute("CREATE TABLE cat3 (name VARCHAR(100))").unwrap();
    engine.execute("CREATE TABLE bm3 (url VARCHAR(2048), category TEXT REFERENCES cat3(id))").unwrap();
    engine.execute("DROP TABLE bm3 CASCADE").unwrap();
    // Junction table should be gone
    let result = engine.execute("SELECT name FROM sqlite_master WHERE type='table' AND name='bm3_category'").unwrap();
    assert_eq!(result.rows.len(), 0);
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test -p ddb-core insert_into_junction_writes_through delete_from_junction_writes_through drop_table_cascades_junction`
Expected: FAIL

- [ ] **Step 5: Add pre-parse interception for junction table INSERT/DELETE**

In `execute()`, detect if the target table is a junction table (name contains `_` and matches `{table}_{column}` pattern for a known type). Route to `handle_junction_insert` or `handle_junction_delete`.

These handlers:
1. Parse the statement to get the parent doogat ID and reference target ID
2. Read the parent doogat from git
3. Append/remove the `- key:: [[target_id]]` line in the reference section
4. Commit the updated doogat
5. Re-index

- [ ] **Step 6: Add junction table drop to handle_drop_table**

In `handle_drop_table()`, before dropping the main materialized table, load the schema and drop all junction tables:

```rust
for col in &schema.columns {
    if col.references.is_some() {
        self.index.conn().execute(
            &format!("DROP TABLE IF EXISTS {}_{}", schema.table_name, col.name),
            [],
        )?;
    }
}
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p ddb-core insert_into_junction_writes_through delete_from_junction_writes_through drop_table_cascades_junction`
Expected: PASS

- [ ] **Step 8: Run full test suite**

Run: `cargo test -p ddb-core`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add ddb-core/src/sql_engine.rs
git commit -m "feat(core): junction table INSERT/DELETE write-through and DROP CASCADE"
```

---

### Task 11: Consistency checks (title compliance, manual typedef, zone migration)

**Depends on:** Task 1 (title_template, origin), Task 7 (title cascade)

Note: the spec's Code Locations table says "fix module in `ddb-core/src/indexer/`" — this is wrong. The fix logic lives in `ddb-core/src/consistency.rs`, called from `service.rs:716`.

**Files:**
- Modify: `ddb-core/src/consistency.rs` (fix_all, apply_fix)
- Modify: `ddb-core/src/types.rs:1040-1053` (Fix enum)
- Test: `ddb-core/src/consistency.rs` or integration test file

- [ ] **Step 1: Add Fix variants for new checks**

In `types.rs`, add to the `Fix` enum:

```rust
TitleNonCompliant { expected: String },
ManualTypedef,
ZoneMigrated { column: String, from: Zone, to: Zone },
```

- [ ] **Step 2: Write test — non-compliant title detected**

Test that a doogat with explicit title that differs from what the template would produce gets flagged.

- [ ] **Step 3: Implement title compliance check in fix_all**

In `consistency.rs`, during the fix scan: for each typed doogat, if the typedef has a `title_template`, interpolate the template from the doogat's fields and compare to the actual title. If different, emit `Fix::TitleNonCompliant`.

- [ ] **Step 4: Write test — manual typedef flagged**

Test that a _typedef doogat with `origin: manual` produces `Fix::ManualTypedef` in verbose mode.

- [ ] **Step 5: Implement manual typedef check**

In `fix_all`, when scanning _typedef doogats: if `origin` is `"manual"`, add `Fix::ManualTypedef` to the report.

- [ ] **Step 6: Implement zone migration in migrate_all**

In `migrate_all()`, for each doogat of a typed type: compare the column's current zone in the typedef with where the data actually lives in the doogat. If mismatched, move the data:
- body→frontmatter: parse section, add YAML field, remove section
- frontmatter→body: remove YAML field, add section
- to/from reference: format/parse wikilink lines

- [ ] **Step 7: Run full test suite**

Run: `cargo test -p ddb-core`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add ddb-core/src/types.rs ddb-core/src/consistency.rs
git commit -m "feat(core): title compliance, manual typedef flag, zone migration in fix"
```

---

### Task 12: `ddb help` subcommand and after_long_help

**Depends on:** all feature tasks (Tasks 1-11) so guide text is accurate

**Files:**
- Modify: `ddb-cli/src/main.rs` (Command enum, main fn)
- Test: `tests/e2e/`

- [ ] **Step 1: Add Help subcommand to Command enum**

```rust
/// In-depth guides
Help {
    /// Guide topic (e.g. create-app)
    topic: Option<String>,
},
```

- [ ] **Step 2: Add guide text as const**

Create a `const CREATE_APP_GUIDE: &str` in main.rs (or a separate `guides.rs` module) with the full tutorial text covering:
1. Use CREATE TABLE, not manual typedefs
2. SQL types → zone inference table
3. Three-zone mental model
4. Title resolution cascade
5. Zone overrides with ALTER TABLE SET ZONE
6. ENUM/SET for constraints
7. Junction tables for multi-valued references
8. Quick API examples
9. Common mistakes

- [ ] **Step 3: Wire Help command handler**

```rust
Command::Help { topic } => {
    match topic.as_deref() {
        Some("create-app") => outln!("{}", CREATE_APP_GUIDE)?,
        Some(other) => {
            eprintln!("Unknown guide: {other}");
            eprintln!("Available guides:");
            eprintln!("  create-app    Data modeling, zones, title resolution, and API access");
            std::process::exit(1);
        }
        None => {
            outln!("Available guides:")?;
            outln!("  create-app    Data modeling, zones, title resolution, and API access")?;
            outln!("")?;
            outln!("Usage: ddb help <topic>")?;
        }
    }
}
```

- [ ] **Step 4: Add after_long_help to existing commands**

```rust
/// Execute SQL (DDL/DML routed through SQL engine; SELECT queries index)
#[command(after_long_help = "For app data modeling, see: ddb help create-app")]
Query { ... },

/// Type definition management
#[command(after_long_help = "To define types, use CREATE TABLE via 'ddb query'. See: ddb help create-app")]
Type { ... },

/// Create a new doogat
#[command(after_long_help = "Note: for type definitions, prefer CREATE TABLE via 'ddb query'. See: ddb help create-app")]
Create { ... },
```

- [ ] **Step 5: Add GUIDES section to top-level help**

Use `after_help` (not `after_long_help`) so the GUIDES section appears on both `ddb -h` and `ddb --help`:

```rust
#[command(name = "ddb", version, about = "Decentralized Doogat DB", after_help = "GUIDES:\n  help <topic>    In-depth guides (try: ddb help create-app)")]
struct Cli { ... }
```

The per-command hints on Query/Type/Create use `after_long_help` (only visible with `--help`, not `-h`) to avoid cluttering short help.

- [ ] **Step 6: Build and manually verify**

Run: `cargo build -p ddb-cli`
Then: `./target/debug/ddb --help`, `./target/debug/ddb help`, `./target/debug/ddb help create-app`, `./target/debug/ddb query --help`

- [ ] **Step 7: Write e2e test for help subcommand**

In `tests/e2e/`, add a test that verifies `ddb help create-app` exits 0 and output contains key sections (e.g. "CREATE TABLE", "Zone", "title"). Also test `ddb help unknown-topic` exits non-zero.

```rust
#[test]
fn help_create_app_prints_guide() {
    Command::cargo_bin("ddb")
        .unwrap()
        .args(&["help", "create-app"])
        .assert()
        .success()
        .stdout(predicates::str::contains("CREATE TABLE"))
        .stdout(predicates::str::contains("zone"));
}

#[test]
fn help_unknown_topic_fails() {
    Command::cargo_bin("ddb")
        .unwrap()
        .args(&["help", "nonexistent"])
        .assert()
        .failure();
}
```

- [ ] **Step 8: Run e2e tests**

Run: `cargo build -p ddb-cli && cargo test -p ddb-e2e help_`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add ddb-cli/src/main.rs
git commit -m "feat(cli): add ddb help create-app guide and after_long_help hints"
```

---

### Task 13: GraphQL list fields for REFERENCES columns

**Depends on:** Task 2 (parser multi-value), Task 9 (junction tables)

**Files:**
- Modify: `ddb-server/src/schema/mod.rs` (typed query generation)
- Modify: `ddb-server/src/schema/base_types.rs` (doogat_to_value)
- Test: `ddb-server/` test module or integration test

- [ ] **Step 1: Write test — GraphQL type has both scalar and list field for REFERENCES**

Test that the dynamically generated GraphQL schema for a type with a REFERENCES column includes both the singular scalar field (`category: String`) and a pluralized list field (`categories: [String!]!`).

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — only scalar field exists

- [ ] **Step 3: Modify typed query generation**

In `schema/mod.rs`, where columns are mapped to GraphQL fields: for REFERENCES columns, generate an additional list field with pluralized name. The list resolver queries the junction table:

```rust
if col.references.is_some() {
    let plural_name = pluralize(&col.name);
    let junction_table = format!("{}_{}", schema.table_name, col.name);
    let col_name_owned = col.name.clone();
    let table_name = schema.table_name.clone();
    // Add list field that queries junction table
    object = object.field(
        Field::new(plural_name, TypeRef::named_nn_list(TypeRef::STRING), move |ctx| {
            let id = ctx.parent_value.try_get("id")?.string()?;
            let sql = format!(
                "SELECT {col_name_owned}_id FROM {junction_table} WHERE {table_name}_id = '{id}'"
            );
            // Execute SQL and return list of IDs
            // ...
        }),
    );
}
```

- [ ] **Step 4: Update doogat_to_value for multi-valued reference arrays**

In `base_types.rs` `doogat_to_value()`: for reference-zone fields, group by key and produce arrays instead of single values.

- [ ] **Step 5: Run test to verify it passes**

Expected: PASS

- [ ] **Step 6: Run server tests**

Run: `cargo test -p ddb-server`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add ddb-server/src/schema/mod.rs ddb-server/src/schema/base_types.rs
git commit -m "feat(server): GraphQL list fields for multi-valued REFERENCES"
```

---

### Task 14: REST structured JSON for reference fields

**Depends on:** Task 2 (parser multi-value)

**Files:**
- Modify: `ddb-server/src/rest.rs` (DoogatJson, doogat_to_json)
- Test: `ddb-server/` test module

- [ ] **Step 1: Write test — REST JSON returns structured reference arrays**

Test that `doogat_to_json()` for a doogat with multiple reference-zone fields of the same key returns them as an array.

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — currently returns raw `reference_section` string

- [ ] **Step 3: Modify DoogatJson and doogat_to_json**

Add a `references: HashMap<String, Vec<String>>` field to DoogatJson. In `doogat_to_json()`:

```rust
let mut refs: HashMap<String, Vec<String>> = HashMap::new();
for field in &parsed.inline_fields {
    if field.zone == Zone::Reference {
        refs.entry(field.key.clone()).or_default().push(field.value.clone());
    }
}
```

Keep `reference_section` for backwards compat but add the structured `references` field.

- [ ] **Step 4: Run test to verify it passes**

Expected: PASS

- [ ] **Step 5: Run server tests**

Run: `cargo test -p ddb-server`
Expected: PASS (nosql_api gets the change for free)

- [ ] **Step 6: Commit**

```bash
git add ddb-server/src/rest.rs
git commit -m "feat(server): structured multi-value reference arrays in REST JSON"
```

---

### Task 15: Documentation fixes

**Depends on:** all feature tasks (Tasks 1-14) so doc content is accurate

**Files:**
- Modify: `docs/src/guide/building-apps.md`

- [ ] **Step 1: Fix zone assignment rules**

Replace the wrong "TEXT defaults to frontmatter" claim with the SQL type→zone inference table from the spec.

- [ ] **Step 2: Add three-zone mental model**

Add the rule-of-thumb paragraph: "If it points somewhere else, it's a reference. If it describes the doogat, it's metadata. If it IS the doogat, it's body."

- [ ] **Step 3: Add ENUM/SET examples**

Replace the Constraints section's hand-edited YAML approach with SQL ENUM syntax.

- [ ] **Step 4: Add title resolution section**

Document the cascade, `title_template`, and non-compliance behavior.

- [ ] **Step 5: Add ALTER TABLE sections**

Document SET ZONE, SET/DROP TITLE TEMPLATE with examples.

- [ ] **Step 6: Add junction table section**

Document multi-valued references, junction table queries, INSERT/DELETE.

- [ ] **Step 7: Fix worked examples**

Update bookmark example: `url` → reference zone (via ALTER TABLE), `description` → body, `status` → frontmatter. Show correct on-disk Markdown.

- [ ] **Step 8: Add typedef workflow callout**

"Always use CREATE TABLE. Do not create _typedef doogats manually."

- [ ] **Step 9: Build docs and verify**

Run: `cd docs && mdbook build`
Expected: No errors

- [ ] **Step 10: Commit**

```bash
git add docs/src/guide/building-apps.md
git commit -m "docs: fix building-apps guide (zones, types, title, junction tables)"
```

---

### Task 16: Integration tests and smoke test

**Depends on:** all code tasks (Tasks 1-14)

**Files:**
- Modify: `tests/smoke.sh` (add building-apps scenario)
- Modify: `tests/smoke.ps1` (add building-apps scenario)
- Test: `tests/e2e/`

- [ ] **Step 1: Add smoke test scenario for app building**

In `tests/smoke.sh`, add a numbered section that exercises the full app-building flow:

```bash
# Section N: App building
ddb query "CREATE TABLE category (name VARCHAR(100))"
ddb query "INSERT INTO category (name) VALUES ('work')"
CAT_ID=$(ddb query "SELECT id FROM category" | tail -1 | awk '{print $1}')
ddb query "CREATE TABLE bookmark (url VARCHAR(2048), category TEXT REFERENCES category(id))"
ddb query "ALTER TABLE bookmark SET ZONE reference FOR url"
ddb query "INSERT INTO bookmark (title, url, category) VALUES ('Rust Book', 'https://doc.rust-lang.org', '$CAT_ID')"
ddb query "SELECT * FROM bookmark"
ddb query "SELECT * FROM bookmark_category"
ddb help create-app | head -5
pass "App building"
```

- [ ] **Step 2: Add equivalent to smoke.ps1**

Mirror the bash scenario in PowerShell syntax.

- [ ] **Step 3: Run smoke test**

Run: `SMOKE_PROFILE=quick ./tests/smoke.sh`
Expected: PASS

- [ ] **Step 4: Run full test suite**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 5: Run clippy**

Run: `cargo clippy --workspace`
Expected: No warnings

- [ ] **Step 6: Commit**

```bash
git add tests/smoke.sh tests/smoke.ps1
git commit -m "test: add app-building smoke test scenario"
```

---

### Task 17: Architecture doc update

**Depends on:** all code tasks (Tasks 1-14)

**Files:**
- Modify: `docs/src/technical/walkthrough.md`

- [ ] **Step 1: Update walkthrough with junction table data flow**

Add a section explaining: CREATE TABLE with REFERENCES → junction table auto-creation → multi-value materialization → GraphQL list fields.

- [ ] **Step 2: Update module descriptions**

Note the new pre-parse interception in sql_engine, the parser dedup change, and the junction table handling in materialize.rs.

- [ ] **Step 3: Commit**

```bash
git add docs/src/technical/walkthrough.md
git commit -m "docs: update architecture walkthrough with junction tables and zone inference"
```

---

### Task 18: Showboat walkthrough

**Depends on:** Task 16 (smoke test passes)

**Files:**
- Create: `.local/walkthroughs/NNNNN-app-building.md`

Per AGENTS.md Definition of Done: "if the task adds a CLI command, server endpoint, or user-facing behavior, create an executable showboat walkthrough."

- [ ] **Step 1: Initialize walkthrough**

```bash
WD=/tmp/ddb-demo-app-building
showboat init .local/walkthroughs/00002-app-building.md "Building Apps with Doogat DB"
```

(Adjust the 5-digit prefix to be the next available number.)

- [ ] **Step 2: Demo CREATE TABLE with zone inference**

```bash
showboat note .local/walkthroughs/00002-app-building.md "Initialize a repo and create a typed table."
showboat exec --workdir $WD .local/walkthroughs/00002-app-building.md bash "mkdir -p $WD && ddb init"
showboat exec --workdir $WD .local/walkthroughs/00002-app-building.md bash "ddb query \"CREATE TABLE bookmark (url VARCHAR(2048), description TEXT)\""
```

- [ ] **Step 3: Demo ENUM, title template, zone override**

```bash
showboat note .local/walkthroughs/00002-app-building.md "Use ENUM for constraints, set a title template, and override a zone."
showboat exec --workdir $WD .local/walkthroughs/00002-app-building.md bash "ddb query \"CREATE TABLE task (name VARCHAR(100), status ENUM('todo','doing','done') DEFAULT 'todo')\""
showboat exec --workdir $WD .local/walkthroughs/00002-app-building.md bash "ddb query \"ALTER TABLE task SET TITLE TEMPLATE '{name} ({status})'\""
showboat exec --workdir $WD .local/walkthroughs/00002-app-building.md bash "ddb query \"ALTER TABLE bookmark SET ZONE reference FOR url\""
```

- [ ] **Step 4: Demo INSERT with title cascade and junction tables**

```bash
showboat note .local/walkthroughs/00002-app-building.md "Insert data showing title resolution and multi-valued references."
showboat exec --workdir $WD .local/walkthroughs/00002-app-building.md bash "ddb query \"INSERT INTO task (name, status) VALUES ('Ship v1', 'doing')\""
showboat exec --workdir $WD .local/walkthroughs/00002-app-building.md bash "ddb query \"SELECT id, title, status FROM task\""
```

- [ ] **Step 5: Demo ddb help create-app**

```bash
showboat note .local/walkthroughs/00002-app-building.md "The built-in guide covers all of this."
showboat exec .local/walkthroughs/00002-app-building.md bash "ddb help create-app | head -20"
```

- [ ] **Step 6: Cleanup and verify**

```bash
showboat exec .local/walkthroughs/00002-app-building.md bash "rm -rf $WD"
showboat verify .local/walkthroughs/00002-app-building.md
```

- [ ] **Step 7: No commit needed** — `.local/` is gitignored

---

### Task 19: Changelog entry

**Depends on:** all tasks complete

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add entries to [Unreleased] section**

```markdown
### Added
- `ddb help create-app` in-CLI guide for app builders
- Type-aware zone inference: SQL types (VARCHAR, TEXT, ENUM, etc.) determine zone placement
- ENUM/SET in CREATE TABLE maps to `allowed_values` constraints
- `ALTER TABLE ... SET ZONE` for column zone overrides
- `ALTER TABLE ... SET/DROP TITLE TEMPLATE` for title derivation
- Title resolution cascade: explicit > template > body > frontmatter > fallback
- `origin` field on typedefs (ddl/manual) with warning on manual creation
- Junction tables for multi-valued REFERENCES columns
- GraphQL list fields for REFERENCES columns
- Structured multi-value JSON for reference fields in REST/NoSQL API

### Fixed
- SQL INSERT now respects explicit `title` column instead of overwriting with first body TEXT
- building-apps.md: corrected zone assignment rules (TEXT defaults to body, not frontmatter)

### Changed
- Parser preserves multiple same-key reference fields (was: first-wins dedup)
- `DROP TABLE` cascades to junction tables
```

- [ ] **Step 2: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs: update changelog with app-building UX improvements"
```
