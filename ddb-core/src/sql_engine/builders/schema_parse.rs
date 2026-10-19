use std::collections::BTreeMap;

use crate::error::{DoogatError, Result};
use crate::types::{ColumnDef, ParsedDoogat, TableSchema, Value, Zone};

use super::super::helpers::parse_title_template;

struct OptionalColumnFields {
    references: Option<String>,
    zone: Option<Zone>,
    required: bool,
    search_boost: Option<f64>,
    allowed_values: Option<Vec<String>>,
    default_value: Option<String>,
    /// PRD 00129 §2: per-column `ON DELETE` action. Default RESTRICT.
    on_delete: crate::types::OnDeleteAction,
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
    let on_delete = map
        .get("on_delete")
        .and_then(|v| v.as_str())
        .map(crate::types::OnDeleteAction::parse_typedef_str)
        .unwrap_or_default();
    OptionalColumnFields {
        references,
        zone,
        required,
        search_boost,
        allowed_values,
        default_value,
        on_delete,
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
        on_delete,
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
        on_delete,
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

    let unique_together = extra.get("unique_together").and_then(parse_unique_together);

    let search_key = extra
        .get("search_key")
        .and_then(|v| v.as_str())
        .map(String::from);

    // PRD 00139 §1: typedef YAMLs without `singleton:` parse as `false`
    // so legacy typedefs keep their non-singleton semantics.
    let singleton = extra
        .get("singleton")
        .map(|v| matches!(v, Value::Bool(true)) || v.as_str() == Some("true"))
        .unwrap_or(false);

    OptionalSchemaFields {
        crdt_strategy,
        template_sections,
        folder,
        stale_after_days,
        title_template,
        origin,
        unique_together,
        search_key,
        singleton,
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
    search_key: Option<String>,
    singleton: bool,
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
    // load time, not silently at runtime.
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
        search_key: opt.search_key,
        singleton: opt.singleton,
    })
}
