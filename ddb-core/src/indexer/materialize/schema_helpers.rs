use super::{is_core_column, normalize_bool_str};

/// Check if a key appears in multiple zones across a doogat, emitting warnings for duplicates.
pub(super) fn check_cross_zone_duplicates(
    parsed: &crate::types::ParsedDoogat,
    path: &str,
    warnings: &mut Vec<crate::types::ConsistencyWarning>,
) {
    use crate::types::ConsistencyWarning;

    let mut seen_keys: std::collections::HashMap<String, &str> = std::collections::HashMap::new();

    for key in parsed.meta.extra.keys() {
        seen_keys.insert(key.clone(), "frontmatter");
    }

    for field in &parsed.inline_fields {
        if field.zone == crate::types::Zone::Reference {
            if let Some(&other_zone) = seen_keys.get(&field.key) {
                if other_zone != "reference" {
                    warnings.push(ConsistencyWarning::CrossZoneDuplicate {
                        path: path.to_string(),
                        key: field.key.clone(),
                    });
                }
            }
        }
    }
}

/// Check if required fields from a typedef are present, emitting warnings for missing ones.
pub(super) fn check_required_fields(
    parsed: &crate::types::ParsedDoogat,
    path: &str,
    typedef_schemas: &std::collections::HashMap<String, crate::types::TableSchema>,
    warnings: &mut Vec<crate::types::ConsistencyWarning>,
) {
    use crate::types::ConsistencyWarning;

    let type_name = match &parsed.meta.doogat_type {
        Some(t) => t,
        None => return,
    };
    let schema = match typedef_schemas.get(type_name.as_str()) {
        Some(s) => s,
        None => return,
    };

    for col in &schema.columns {
        if !col.required {
            continue;
        }
        let has_value = match col
            .zone
            .as_ref()
            .unwrap_or(&crate::types::Zone::Frontmatter)
        {
            crate::types::Zone::Frontmatter => parsed.meta.extra.contains_key(&col.name),
            crate::types::Zone::Reference => parsed.inline_fields.iter().any(|f| f.key == col.name),
            crate::types::Zone::Body => parsed.body.contains(&format!("## {}", col.name)),
        };
        if !has_value {
            warnings.push(ConsistencyWarning::MissingRequired {
                path: path.to_string(),
                type_name: type_name.clone(),
                field: col.name.clone(),
            });
        }
    }
}

/// Build column names, placeholders, and values for an INSERT from a parsed doogat.
pub(super) fn extract_column_values(
    schema: &crate::types::TableSchema,
    id: &str,
    doogat: &crate::types::ParsedDoogat,
    updated_at: Option<String>,
) -> (Vec<String>, Vec<String>, Vec<Option<String>>) {
    let mut col_names = vec![
        "id".to_string(),
        "title".to_string(),
        "date".to_string(),
        "updated_at".to_string(),
    ];
    let mut placeholders = vec![
        "?1".to_string(),
        "?2".to_string(),
        "?3".to_string(),
        "?4".to_string(),
    ];
    let mut vals: Vec<Option<String>> = vec![
        Some(id.to_string()),
        doogat.meta.title.clone(),
        doogat.meta.date.clone(),
        updated_at,
    ];

    let mut param_idx = 5;
    for col in &schema.columns {
        if is_core_column(&col.name) {
            continue;
        }
        col_names.push(format!("\"{}\"", col.name));
        placeholders.push(format!("?{}", param_idx));
        param_idx += 1;
        let val = extract_column_value(doogat, col);
        vals.push(if val.is_empty() { None } else { Some(val) });
    }

    (col_names, placeholders, vals)
}

/// Collect frontmatter, body, and reference columns from a parsed doogat into the accumulator.
pub(super) fn collect_zone_columns(
    parsed: &crate::types::ParsedDoogat,
    columns: &mut std::collections::HashMap<String, (crate::types::Zone, Vec<String>)>,
) {
    use crate::types::Zone;

    for (key, value) in &parsed.meta.extra {
        let inferred_type = infer_yaml_type(value);
        columns
            .entry(key.to_lowercase())
            .or_insert_with(|| (Zone::Frontmatter, Vec::new()))
            .1
            .push(inferred_type);
    }

    for section in &parsed.sections {
        if section.level > 0 {
            columns
                .entry(section.heading.to_lowercase())
                .or_insert_with(|| (Zone::Body, vec!["TEXT".to_string()]));
        }
    }

    for field in &parsed.inline_fields {
        if field.zone == Zone::Reference {
            let entry = columns
                .entry(field.key.to_lowercase())
                .or_insert_with(|| (Zone::Reference, Vec::new()));
            entry.1.push("TEXT".to_string());
        }
    }
}

/// Build final sorted ColumnDef list and TableSchema from collected column data.
pub(super) fn finalize_schema_columns(
    type_name: &str,
    columns: std::collections::HashMap<String, (crate::types::Zone, Vec<String>)>,
) -> crate::types::TableSchema {
    use crate::types::{ColumnDef, TableSchema};

    let mut cols: Vec<ColumnDef> = columns
        .into_iter()
        .map(|(name, (zone, types))| {
            let data_type = widen_types(&types);
            ColumnDef {
                name,
                data_type,
                references: None,
                zone: Some(zone),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
                on_delete: crate::types::OnDeleteAction::Restrict,
            }
        })
        .collect();

    cols.sort_by(|a, b| a.name.cmp(&b.name));

    TableSchema {
        table_name: type_name.to_string(),
        columns: cols,
        crdt_strategy: None,
        template_sections: vec![],
        folder: false,
        stale_after_days: None,
        title_template: None,
        origin: None,
        unique_together: None,
        search_key: None,
        singleton: false,
    }
}

/// Extract a column value from a parsed doogat according to zone mapping.
fn extract_column_value(
    doogat: &crate::types::ParsedDoogat,
    col: &crate::types::ColumnDef,
) -> String {
    use crate::types::Zone;

    let zone = col.zone.clone().unwrap_or_else(|| {
        if col.references.is_some() {
            Zone::Reference
        } else if matches!(
            col.data_type.to_uppercase().as_str(),
            "INTEGER" | "REAL" | "BOOLEAN"
        ) {
            Zone::Frontmatter
        } else {
            Zone::Body
        }
    });

    match zone {
        Zone::Reference => {
            let values = extract_multi_reference_values(doogat, &col.name);
            values.join(",")
        }
        Zone::Frontmatter => {
            // Use path navigation for dot/bracket names, flat lookup otherwise
            let val = if col.name.contains('.') || col.name.contains('[') {
                crate::types::get_path_in_map(&doogat.meta.extra, &col.name).ok()
            } else {
                doogat.meta.extra.get(&col.name)
            };
            let is_bool_col = col.data_type.eq_ignore_ascii_case("BOOLEAN");
            val.map(|v| match v {
                crate::types::Value::Number(n) => n.to_string(),
                crate::types::Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
                crate::types::Value::String(s) => {
                    if is_bool_col {
                        normalize_bool_str(s)
                    } else {
                        s.clone()
                    }
                }
                _ => format!("{v:?}"),
            })
            .unwrap_or_default()
        }
        Zone::Body => doogat
            .sections
            .iter()
            .find(|s| s.level > 0 && s.heading.eq_ignore_ascii_case(&col.name))
            .map(|s| s.content.trim().to_string())
            .unwrap_or_default(),
    }
}

/// Extract all reference values for a given column name from a parsed doogat.
///
/// Empty / whitespace-only values are filtered out. PRD 00134 cycle-1 review
/// C1 task #3: `UPDATE … SET col = NULL` collapses to `- col:: [[]]` in the
/// reference zone (`expr_to_string` turns NULL into ""), and without this
/// filter the helper would `INSERT (parent_id, '')` into the junction
/// table, leaving an empty-string ghost row instead of clearing the FK.
/// Filtering here gives a single uniform fix for every caller
/// (`populate_junction_tables`, `sync_junction_tables_for_columns`,
/// `extract_column_values`).
pub(super) fn extract_multi_reference_values(
    doogat: &crate::types::ParsedDoogat,
    col_name: &str,
) -> Vec<String> {
    doogat
        .inline_fields
        .iter()
        .filter(|f| f.key == col_name && f.zone == crate::types::Zone::Reference)
        .map(|f| f.value.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// Infer a SQL data type from a domain Value.
fn infer_yaml_type(value: &crate::types::Value) -> String {
    match value {
        crate::types::Value::Bool(_) => "BOOLEAN".to_string(),
        crate::types::Value::Number(n) => {
            if n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 {
                "INTEGER".to_string()
            } else {
                "REAL".to_string()
            }
        }
        crate::types::Value::String(s) => {
            if s.parse::<i64>().is_ok() {
                "INTEGER".to_string()
            } else if s.parse::<f64>().is_ok() {
                "REAL".to_string()
            } else if s == "true" || s == "false" {
                "BOOLEAN".to_string()
            } else {
                "TEXT".to_string()
            }
        }
        _ => "TEXT".to_string(),
    }
}

/// Widen types: if all values agree, use that type; otherwise widen to TEXT.
fn widen_types(types: &[String]) -> String {
    if types.is_empty() {
        return "TEXT".to_string();
    }
    let first = &types[0];
    if types.iter().all(|t| t == first) {
        return first.clone();
    }
    // INTEGER + REAL → REAL
    if types.iter().all(|t| t == "INTEGER" || t == "REAL") {
        return "REAL".to_string();
    }
    "TEXT".to_string()
}
