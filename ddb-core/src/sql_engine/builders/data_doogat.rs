use std::collections::BTreeMap;

use crate::error::Result;
use crate::types::{
    ColumnDef, DoogatId, DoogatMeta, InlineField, Link, ParsedDoogat, TableSchema, Value, Zone,
};

use super::super::helpers::{
    is_safe_sql_identifier, parse_title_template, re_unfilled_placeholder, to_yaml_value,
};

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
pub(crate) fn resolve_insert_title(
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
pub(in crate::sql_engine) fn recompute_template_title(
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
pub(crate) fn build_data_doogat(
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
