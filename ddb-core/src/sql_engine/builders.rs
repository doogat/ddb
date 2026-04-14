use std::collections::BTreeMap;

use crate::error::{DoogatError, Result};
use crate::types::{
    ColumnDef, DoogatId, DoogatMeta, InlineField, Link, ParsedDoogat, TableSchema, Value, Zone,
};

use super::helpers::{parse_title_template, re_unfilled_placeholder, to_yaml_value};

/// Convert a single `ColumnDef` into a YAML-style `Value::Map`.
fn build_column_yaml(col: &ColumnDef) -> Value {
    let mut map = BTreeMap::new();
    map.insert("name".to_string(), Value::String(col.name.clone()));
    map.insert(
        "data_type".to_string(),
        Value::String(col.data_type.clone()),
    );
    if let Some(ref zone) = col.zone {
        let zone_str = match zone {
            Zone::Frontmatter => "frontmatter",
            Zone::Body => "body",
            Zone::Reference => "reference",
        };
        map.insert("zone".to_string(), Value::String(zone_str.into()));
    }
    if col.required {
        map.insert("required".to_string(), Value::Bool(true));
    }
    if let Some(boost) = col.search_boost {
        map.insert("search_boost".to_string(), Value::Number(boost));
    }
    if let Some(ref r) = col.references {
        map.insert("references".to_string(), Value::String(r.clone()));
    }
    if let Some(ref vals) = col.allowed_values {
        map.insert(
            "allowed_values".to_string(),
            Value::List(vals.iter().map(|v| Value::String(v.clone())).collect()),
        );
    }
    if let Some(ref default) = col.default_value {
        map.insert("default_value".to_string(), Value::String(default.clone()));
    }
    Value::Map(map)
}

/// Build the `columns` YAML list from a schema's column definitions.
fn build_columns_yaml(schema: &TableSchema) -> Value {
    Value::List(schema.columns.iter().map(build_column_yaml).collect())
}

/// Insert optional typedef extra fields (crdt_strategy, template_sections, etc.) into `extra`.
fn build_typedef_extra_fields(schema: &TableSchema, extra: &mut BTreeMap<String, Value>) {
    if let Some(ref strategy) = schema.crdt_strategy {
        extra.insert("crdt_strategy".to_string(), Value::String(strategy.clone()));
    }
    if !schema.template_sections.is_empty() {
        extra.insert(
            "template_sections".to_string(),
            Value::List(
                schema
                    .template_sections
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
    }
    if schema.folder {
        extra.insert("folder".to_string(), Value::Bool(true));
    }
    if let Some(ref tt) = schema.title_template {
        extra.insert("title_template".to_string(), Value::String(tt.clone()));
    }
    if let Some(ref o) = schema.origin {
        extra.insert("origin".to_string(), Value::String(o.clone()));
    }
    if let Some(ref constraints) = schema.unique_together {
        if !constraints.is_empty() {
            let outer = Value::List(
                constraints
                    .iter()
                    .map(|cols| {
                        Value::List(cols.iter().map(|c| Value::String(c.clone())).collect())
                    })
                    .collect(),
            );
            extra.insert("unique_together".to_string(), outer);
        }
    }
}

/// Build a _typedef doogat from a TableSchema.
pub fn build_typedef_doogat(id: &DoogatId, schema: &TableSchema) -> ParsedDoogat {
    let mut extra = BTreeMap::new();
    extra.insert("columns".to_string(), build_columns_yaml(schema));
    build_typedef_extra_fields(schema, &mut extra);

    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(id.clone()),
            title: Some(schema.table_name.clone()),
            date: None,
            doogat_type: Some("_typedef".into()),
            tags: vec![],
            extra,
        },
        body: String::new(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: format!("ddb/_typedef/{}.md", id.0),
        updated_at: None,
    }
}

/// Accumulated output from processing schema columns into their respective zones.
struct ColumnZoneOutput {
    extra: BTreeMap<String, Value>,
    body_sections: Vec<String>,
    ref_lines: Vec<String>,
    links: Vec<Link>,
    inline_fields: Vec<InlineField>,
}

/// Process a single reference-zone column, appending to the output accumulators.
fn process_reference_column(
    col: &ColumnDef,
    val: &str,
    ref_folder_types: &std::collections::HashSet<String>,
    out: &mut ColumnZoneOutput,
) {
    let link_target = match col.references {
        Some(ref ref_table) if ref_folder_types.contains(ref_table) => {
            format!("ddb/{ref_table}/{val}.md")
        }
        _ => val.to_string(),
    };
    out.ref_lines
        .push(format!("- {}:: [[{}]]", col.name, link_target));
    out.links.push(Link {
        target: link_target.clone(),
        display: None,
        section: None,
        kind: crate::types::LinkKind::WikiLink,
        zone: Zone::Reference,
    });
    out.inline_fields.push(InlineField {
        key: col.name.clone(),
        value: link_target,
        zone: Zone::Reference,
    });
}

/// Process schema columns into frontmatter, body, and reference zones.
fn process_column_zones(
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
    ref_folder_types: &std::collections::HashSet<String>,
) -> ColumnZoneOutput {
    let mut out = ColumnZoneOutput {
        extra: BTreeMap::new(),
        body_sections: Vec::new(),
        ref_lines: Vec::new(),
        links: Vec::new(),
        inline_fields: Vec::new(),
    };

    for col in &schema.columns {
        // Core columns (id, title, type, date, updated_at) are written by the
        // materialized-row writer and meta fields, not as typed zone fields.
        // Including them here would double-write title into a frontmatter or
        // body section in addition to `meta.title`.
        if crate::indexer::materialize::is_core_column(&col.name) {
            continue;
        }
        let val = match col_values.get(&col.name) {
            Some(v) => v.clone(),
            None => continue,
        };

        match col.effective_zone() {
            Zone::Reference => {
                process_reference_column(col, &val, ref_folder_types, &mut out);
            }
            Zone::Frontmatter => {
                out.extra
                    .insert(col.name.clone(), to_yaml_value(&val, &col.data_type));
            }
            Zone::Body => {
                out.body_sections
                    .push(format!("## {}\n\n{}", col.name, val));
            }
        }
    }

    out
}

/// Resolve the title for a data doogat.
///
/// Priority chain:
/// 1. Explicit `title` column value from the INSERT.
/// 2. `title_template` interpolation declared on the typedef.
/// 3. `"{type} {id}"` last-resort fallback (only fires for tables whose
///    title is nullable and has no template).
///
/// Dotted placeholders (`{ref_col.field}`) dereference REFERENCES columns by
/// reading the target doogat's field from its materialized row. Missing
/// target or NULL field substitutes empty string (PRD 00127).
fn resolve_insert_title(
    id: &DoogatId,
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
    conn: Option<&rusqlite::Connection>,
) -> String {
    // Priority 1: explicit title column
    if let Some(t) = col_values.get("title") {
        return t.clone();
    }
    // Priority 2: title_template interpolation
    if let Some(ref tmpl) = schema.title_template {
        let rendered = render_title_template(tmpl, schema, col_values, conn);
        let stripped = re_unfilled_placeholder()
            .replace_all(&rendered, "")
            .trim()
            .to_string();
        if !stripped.is_empty() {
            return stripped;
        }
    }
    // Priority 3: "{type} {id}" fallback
    format!("{} {}", schema.table_name, id.0)
}

/// Substitute placeholders in a template. Bare `{col}` substitutes the row's
/// value for that column; `{col.field}` dereferences a REFERENCES column by
/// reading `field` off the target doogat's materialized row.
pub(super) fn render_title_template(
    tmpl: &str,
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
    conn: Option<&rusqlite::Connection>,
) -> String {
    let placeholders = match parse_title_template(tmpl) {
        Ok(p) => p,
        // Fall back to naive substitution when the template has a syntactic
        // problem (multi-hop, malformed ident). Typedef materialization
        // should reject such templates before they reach runtime; if a
        // hand-edited typedef slips through, empty substitution is safer
        // than crashing the INSERT.
        Err(_) => {
            return tmpl.to_string();
        }
    };
    let mut rendered = tmpl.to_string();
    for p in &placeholders {
        let value = match &p.field {
            None => col_values.get(&p.col).cloned().unwrap_or_default(),
            Some(field) => resolve_reference_field(schema, col_values, &p.col, field, conn),
        };
        rendered = rendered.replace(&p.raw, &value);
    }
    rendered
}

/// Resolve `{col.field}` against the referenced doogat's materialized row.
///
/// Caller contract: the target row must already be committed to the SQLite
/// index. For single-process INSERTs this is trivially true (every INSERT
/// uses one connection). For multi-process writers, the referenced target
/// doogat must have been inserted and committed before the dependent row.
fn resolve_reference_field(
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
    col: &str,
    field: &str,
    conn: Option<&rusqlite::Connection>,
) -> String {
    let target_id = match col_values.get(col) {
        Some(v) if !v.is_empty() => v.clone(),
        _ => return String::new(),
    };
    let col_def = match schema.columns.iter().find(|c| c.name == col) {
        Some(c) => c,
        None => return String::new(),
    };
    let Some(target_type) = col_def.references.as_deref() else {
        return String::new();
    };
    let Some(conn) = conn else {
        return String::new();
    };
    if !is_safe_sql_identifier(target_type) || !is_safe_sql_identifier(field) {
        return String::new();
    }
    // Guard against SQLite's legacy double-quoted-string-as-identifier
    // fallback: if `field` isn't a real column on `target_type`, `SELECT
    // "field" FROM ...` would return the literal string "field" instead
    // of erroring. Verify the column exists up front via PRAGMA.
    // (Both identifiers already validated as safe above, so interpolation
    // is safe.)
    let pragma_sql = format!("PRAGMA table_info(\"{target_type}\")");
    let col_exists = conn
        .prepare(&pragma_sql)
        .and_then(|mut stmt| {
            let mut rows = stmt.query([])?;
            let mut found = false;
            while let Some(row) = rows.next()? {
                let name: String = row.get(1)?;
                if name == field {
                    found = true;
                    break;
                }
            }
            Ok(found)
        })
        .unwrap_or(false);
    if !col_exists {
        return String::new();
    }
    let sql = format!("SELECT \"{field}\" FROM \"{target_type}\" WHERE id = ?1");
    conn.query_row(&sql, rusqlite::params![target_id], |row| {
        row.get::<_, Option<String>>(0)
    })
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// Recompute the template-derived title when an UPDATE touches any column
/// the template references. Returns `Ok(Some(new_title))` when the title
/// should change, `Ok(None)` when the template isn't engaged or no touched
/// column appears in it (or the user supplied an explicit `title` update).
pub(super) fn recompute_template_title(
    conn: &rusqlite::Connection,
    schema: &TableSchema,
    table_name: &str,
    id: &str,
    updates: &BTreeMap<String, String>,
) -> Result<Option<String>> {
    let Some(tmpl) = schema.title_template.as_ref() else {
        return Ok(None);
    };
    if updates.contains_key("title") {
        return Ok(None);
    }
    let placeholders = parse_title_template(tmpl).unwrap_or_default();
    if placeholders.is_empty() {
        return Ok(None);
    }
    let template_cols: std::collections::HashSet<String> =
        placeholders.iter().map(|p| p.col.clone()).collect();
    let touches_template = updates.keys().any(|k| template_cols.contains(k));
    if !touches_template {
        return Ok(None);
    }

    let mut col_values: BTreeMap<String, String> = BTreeMap::new();
    if !is_safe_sql_identifier(table_name) {
        return Ok(None);
    }
    for col_name in &template_cols {
        if let Some(v) = updates.get(col_name) {
            col_values.insert(col_name.clone(), v.clone());
            continue;
        }
        if !is_safe_sql_identifier(col_name) {
            continue;
        }
        let sql = format!("SELECT \"{col_name}\" FROM \"{table_name}\" WHERE id = ?1");
        if let Ok(Some(v)) = conn.query_row(&sql, rusqlite::params![id], |r| {
            r.get::<_, Option<String>>(0)
        }) {
            col_values.insert(col_name.clone(), v);
        }
    }
    let rendered = render_title_template(tmpl, schema, &col_values, Some(conn));
    let stripped = re_unfilled_placeholder()
        .replace_all(&rendered, "")
        .trim()
        .to_string();
    if stripped.is_empty() {
        Ok(None)
    } else {
        Ok(Some(stripped))
    }
}

fn is_safe_sql_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Derive the date for a data doogat: prefer explicit `date` from extra/col_values, fall back to id.
fn derive_date(
    id: &DoogatId,
    extra: &mut BTreeMap<String, Value>,
    col_values: &BTreeMap<String, String>,
) -> Option<String> {
    extra
        .remove("date")
        .and_then(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        })
        .or_else(|| col_values.get("date").cloned())
        .or_else(|| Some(format!("{}-{}-{}", &id.0[0..4], &id.0[4..6], &id.0[6..8])))
}

/// Join body sections or reference lines into their final string form.
fn join_sections(sections: &[String], prefix: &str, suffix: &str, sep: &str) -> String {
    if sections.is_empty() {
        String::new()
    } else {
        format!("{}{}{}", prefix, sections.join(sep), suffix)
    }
}

/// Build a data doogat from column values according to the schema's zone mapping.
pub(super) fn build_data_doogat(
    id: &DoogatId,
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
    ref_folder_types: &std::collections::HashSet<String>,
    conn: Option<&rusqlite::Connection>,
) -> ParsedDoogat {
    let mut zones = process_column_zones(schema, col_values, ref_folder_types);

    let title = resolve_insert_title(id, schema, col_values, conn);

    let body = join_sections(&zones.body_sections, "\n", "\n", "\n\n");
    let reference_section = join_sections(&zones.ref_lines, "", "\n", "\n");
    let date = derive_date(id, &mut zones.extra, col_values);

    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(id.clone()),
            title: Some(title),
            date,
            doogat_type: Some(schema.table_name.clone()),
            tags: vec![],
            extra: zones.extra,
        },
        body,
        sections: vec![],
        reference_section,
        inline_fields: zones.inline_fields,
        links: zones.links,
        body_tags: vec![],
        checkboxes: vec![],
        path: format!("ddb/{}.md", id.0),
        updated_at: None,
    }
}

struct OptionalColumnFields {
    references: Option<String>,
    zone: Option<Zone>,
    required: bool,
    search_boost: Option<f64>,
    allowed_values: Option<Vec<String>>,
    default_value: Option<String>,
}

fn parse_optional_column_fields(map: &BTreeMap<String, Value>) -> OptionalColumnFields {
    let references = map
        .get("references")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let zone = map.get("zone").and_then(|v| v.as_str()).map(|s| match s {
        "frontmatter" => Zone::Frontmatter,
        "body" => Zone::Body,
        "reference" => Zone::Reference,
        _ => Zone::Body,
    });
    let required = map
        .get("required")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let search_boost = map.get("search_boost").and_then(|v| v.as_f64());
    let allowed_values = map
        .get("allowed_values")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });
    let default_value = map
        .get("default_value")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    OptionalColumnFields {
        references,
        zone,
        required,
        search_boost,
        allowed_values,
        default_value,
    }
}

fn parse_single_column(item: &Value) -> Result<ColumnDef> {
    let map = item
        .as_mapping()
        .ok_or_else(|| DoogatError::SqlEngine("column must be a mapping".into()))?;
    let name = map
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DoogatError::SqlEngine("column missing name".into()))?
        .to_string();
    let data_type = map
        .get("data_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DoogatError::SqlEngine("column missing data_type".into()))?
        .to_string();
    let OptionalColumnFields {
        references,
        zone,
        required,
        search_boost,
        allowed_values,
        default_value,
    } = parse_optional_column_fields(map);
    Ok(ColumnDef {
        name,
        data_type,
        references,
        zone,
        required,
        search_boost,
        allowed_values,
        default_value,
    })
}

/// Parse column definitions from the typedef's `columns` YAML sequence.
fn parse_column_definitions(columns_seq: &[Value]) -> Result<Vec<ColumnDef>> {
    columns_seq.iter().map(parse_single_column).collect()
}

/// Parse the unique_together constraint from a typedef's YAML value.
/// Supports both flat (`["a", "b"]`) and nested (`[["a", "b"], ["c"]]`) forms.
fn parse_unique_together(val: &Value) -> Option<Vec<Vec<String>>> {
    let outer = val.as_sequence()?;
    if outer.is_empty() {
        return None;
    }
    let is_flat = outer.iter().all(|item| item.as_str().is_some());
    let constraints = if is_flat {
        let cols: Vec<String> = outer
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        if cols.is_empty() {
            return None;
        }
        vec![cols]
    } else {
        outer
            .iter()
            .filter_map(|item| item.as_sequence())
            .map(|inner| {
                inner
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .filter(|cols| !cols.is_empty())
            .collect::<Vec<_>>()
    };
    if constraints.is_empty() {
        None
    } else {
        Some(constraints)
    }
}

/// Extract optional schema fields from a typedef doogat's extra metadata.
fn extract_optional_schema_fields(extra: &BTreeMap<String, Value>) -> OptionalSchemaFields {
    let crdt_strategy = extra
        .get("crdt_strategy")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let template_sections = extra
        .get("template_sections")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let folder = extra
        .get("folder")
        .map(|v| matches!(v, Value::Bool(true)) || v.as_str() == Some("true"))
        .unwrap_or(false);

    let stale_after_days = extra
        .get("stale_after_days")
        .and_then(|v| v.as_f64())
        .map(|n| n as u32);

    let title_template = extra
        .get("title_template")
        .and_then(|v| v.as_str())
        .map(String::from);

    let origin = extra
        .get("origin")
        .and_then(|v| v.as_str())
        .map(String::from);

    let unique_together = extra
        .get("unique_together")
        .and_then(parse_unique_together);

    OptionalSchemaFields {
        crdt_strategy,
        template_sections,
        folder,
        stale_after_days,
        title_template,
        origin,
        unique_together,
    }
}

/// Bag of optional fields parsed from a typedef doogat.
struct OptionalSchemaFields {
    crdt_strategy: Option<String>,
    template_sections: Vec<String>,
    folder: bool,
    stale_after_days: Option<u32>,
    title_template: Option<String>,
    origin: Option<String>,
    unique_together: Option<Vec<Vec<String>>>,
}

/// Extract a TableSchema from a parsed _typedef doogat.
pub fn schema_from_parsed(doogat: &ParsedDoogat) -> Result<TableSchema> {
    let table_name = doogat
        .meta
        .title
        .as_deref()
        .ok_or_else(|| DoogatError::SqlEngine("typedef doogat missing title".into()))?
        .to_string();

    let columns_val = doogat
        .meta
        .extra
        .get("columns")
        .ok_or_else(|| DoogatError::SqlEngine("typedef doogat missing columns".into()))?;

    let columns_seq = columns_val
        .as_sequence()
        .ok_or_else(|| DoogatError::SqlEngine("columns must be a sequence".into()))?;

    let columns = parse_column_definitions(columns_seq)?;
    let opt = extract_optional_schema_fields(&doogat.meta.extra);

    // Validate title_template against this typedef's own columns. Cross-
    // typedef field validation (e.g. `field` exists on `target_type`) runs
    // at ALTER TABLE time in `handle_title_template` because it needs the
    // target's schema. Syntactic + same-typedef checks run here so that
    // hand-edited or imported typedefs with bad templates are rejected at
    // load time, not silently at runtime (PRD 00127 blind-review gap).
    if let Some(tmpl) = opt.title_template.as_deref() {
        let placeholders = parse_title_template(tmpl)?;
        for p in &placeholders {
            let Some(_) = &p.field else { continue };
            let col = columns.iter().find(|c| c.name == p.col).ok_or_else(|| {
                DoogatError::SqlEngine(format!(
                    "title_template references {raw}: column '{col}' not found on {table}",
                    raw = p.raw,
                    col = p.col,
                    table = table_name
                ))
            })?;
            if col.references.is_none() {
                return Err(DoogatError::SqlEngine(format!(
                    "title_template references {raw}: column '{col}' is not a REFERENCES column on {table}",
                    raw = p.raw,
                    col = p.col,
                    table = table_name
                )));
            }
        }
    }

    Ok(TableSchema {
        table_name,
        columns,
        crdt_strategy: opt.crdt_strategy,
        template_sections: opt.template_sections,
        folder: opt.folder,
        stale_after_days: opt.stale_after_days,
        title_template: opt.title_template,
        origin: opt.origin,
        unique_together: opt.unique_together,
    })
}

/// Apply UPDATE SET assignments to a ParsedDoogat according to schema zone mapping.
pub(super) fn apply_updates_to_doogat(
    doogat: &mut ParsedDoogat,
    schema: &TableSchema,
    updates: &BTreeMap<String, String>,
) {
    for (col_name, new_val) in updates {
        // Handle implicit `title` column (not in schema.columns).
        if col_name == "title" {
            doogat.meta.title = Some(new_val.clone());
            continue;
        }

        let col_def = schema.columns.iter().find(|c| c.name == *col_name);
        let col_def = match col_def {
            Some(c) => c,
            None => continue,
        };

        match col_def.effective_zone() {
            Zone::Reference => {
                update_reference_line(&mut doogat.reference_section, col_name, new_val);
            }
            Zone::Frontmatter => {
                doogat
                    .meta
                    .extra
                    .insert(col_name.clone(), to_yaml_value(new_val, &col_def.data_type));
            }
            Zone::Body => {
                update_body_section(&mut doogat.body, col_name, new_val);
                // Legacy behavior: if the first body-zone column is updated
                // and the table has no `title_template`, mirror the new value
                // into `title`. When a `title_template` is declared, the
                // template owns the title and UPDATE runs
                // `recompute_template_title` to derive a fresh value.
                if schema.title_template.is_none() {
                    if let Some(first_body) = schema
                        .columns
                        .iter()
                        .find(|c| c.effective_zone() == Zone::Body)
                    {
                        if first_body.name == *col_name {
                            doogat.meta.title = Some(new_val.clone());
                        }
                    }
                }
            }
        }
    }
}

fn update_body_section(body: &mut String, section_name: &str, new_val: &str) {
    let heading = format!("## {section_name}");
    let lines: Vec<&str> = body.lines().collect();
    let mut result = Vec::new();
    let mut i = 0;
    let mut found = false;

    while i < lines.len() {
        if lines[i].trim() == heading {
            found = true;
            result.push(lines[i]);
            // Skip blank line after heading
            i += 1;
            if i < lines.len() && lines[i].trim().is_empty() {
                result.push("");
            }
            i += 1;
            // Skip old content until next heading or end
            while i < lines.len() && !lines[i].starts_with("## ") {
                i += 1;
            }
            // Insert new value
            result.push(new_val);
            // Add blank line before next section if there is one
            if i < lines.len() {
                result.push("");
            }
        } else {
            result.push(lines[i]);
            i += 1;
        }
    }

    if !found {
        // Append new section
        if !result.is_empty() && !result.last().is_none_or(|l| l.trim().is_empty()) {
            result.push("");
        }
        result.push(&heading);
        result.push("");
        result.push(new_val);
    }

    *body = result.join("\n");
}

fn update_reference_line(reference: &mut String, key: &str, new_val: &str) {
    let prefix = format!("- {key}::");
    let new_line = format!("- {key}:: [[{new_val}]]");
    let lines: Vec<&str> = reference.lines().collect();
    let mut result = Vec::new();
    let mut found = false;

    for line in &lines {
        if line.starts_with(&prefix) {
            result.push(new_line.as_str());
            found = true;
        } else {
            result.push(line);
        }
    }

    if !found {
        result.push(&new_line);
    }

    *reference = format!("{}\n", result.join("\n"));
}

/// Rename a key in a parsed doogat within the appropriate zone.
pub(super) fn rename_key_in_doogat(
    doogat: &mut ParsedDoogat,
    old_name: &str,
    new_name: &str,
    zone: &Zone,
) {
    match zone {
        Zone::Frontmatter => {
            if let Some(val) = doogat.meta.extra.remove(old_name) {
                doogat.meta.extra.insert(new_name.to_string(), val);
            }
        }
        Zone::Body => {
            let old_heading = format!("## {old_name}");
            let new_heading = format!("## {new_name}");
            doogat.body = doogat.body.replace(&old_heading, &new_heading);
        }
        Zone::Reference => {
            let old_prefix = format!("- {old_name}::");
            let new_prefix = format!("- {new_name}::");
            doogat.reference_section = doogat.reference_section.replace(&old_prefix, &new_prefix);
        }
    }
}
