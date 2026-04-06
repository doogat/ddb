use std::collections::BTreeMap;

use crate::error::{DoogatError, Result};
use crate::types::{
    ColumnDef, DoogatId, DoogatMeta, InlineField, Link, ParsedDoogat, TableSchema, Value, Zone,
};

use super::helpers::{is_numeric_type, re_unfilled_placeholder, to_yaml_value};

/// Build a _typedef doogat from a TableSchema.
pub fn build_typedef_doogat(id: &DoogatId, schema: &TableSchema) -> ParsedDoogat {
    let mut extra = BTreeMap::new();

    let columns_yaml: Vec<Value> = schema
        .columns
        .iter()
        .map(|col| {
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
        })
        .collect();

    extra.insert("columns".to_string(), Value::List(columns_yaml));

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

/// Build a data doogat from column values according to the schema's zone mapping.
pub(super) fn build_data_doogat(
    id: &DoogatId,
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
    ref_folder_types: &std::collections::HashSet<String>,
) -> ParsedDoogat {
    let mut extra = BTreeMap::new();
    let mut body_sections: Vec<String> = Vec::new();
    let mut ref_lines: Vec<String> = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let mut inline_fields: Vec<InlineField> = Vec::new();

    // Priority 1: explicit title from INSERT column list
    let mut title_value: Option<String> = col_values.get("title").cloned();

    // Priority 2: title_template interpolation
    if title_value.is_none() {
        if let Some(ref tmpl) = schema.title_template {
            let mut rendered = tmpl.clone();
            for (key, val) in col_values {
                rendered = rendered.replace(&format!("{{{key}}}"), val);
            }
            let rendered = re_unfilled_placeholder()
                .replace_all(&rendered, "")
                .trim()
                .to_string();
            if !rendered.is_empty() {
                title_value = Some(rendered);
            }
        }
    }

    // Track first frontmatter string column for Priority 4 fallback
    let mut first_fm_string: Option<String> = None;

    for col in &schema.columns {
        let val = match col_values.get(&col.name) {
            Some(v) => v.clone(),
            None => continue,
        };

        match col.effective_zone() {
            Zone::Reference => {
                let link_target = if let Some(ref ref_table) = col.references {
                    if ref_folder_types.contains(ref_table) {
                        format!("ddb/{ref_table}/{val}.md")
                    } else {
                        val.clone()
                    }
                } else {
                    val.clone()
                };
                ref_lines.push(format!("- {}:: [[{}]]", col.name, link_target));
                links.push(Link {
                    target: link_target.clone(),
                    display: None,
                    section: None,
                    kind: crate::types::LinkKind::WikiLink,
                    zone: Zone::Reference,
                });
                inline_fields.push(InlineField {
                    key: col.name.clone(),
                    value: link_target.clone(),
                    zone: Zone::Reference,
                });
            }
            Zone::Frontmatter => {
                // Priority 4: track first frontmatter string column
                if first_fm_string.is_none() && !is_numeric_type(&col.data_type) {
                    first_fm_string = Some(val.clone());
                }
                extra.insert(col.name.clone(), to_yaml_value(&val, &col.data_type));
            }
            Zone::Body => {
                // Priority 3: first body column value
                if title_value.is_none() {
                    title_value = Some(val.clone());
                }
                body_sections.push(format!("## {}\n\n{}", col.name, val));
            }
        }
    }

    // Priority 4: first frontmatter string column
    if title_value.is_none() {
        title_value = first_fm_string;
    }

    // Priority 5: "{type} {id}" fallback
    if title_value.is_none() {
        title_value = Some(format!("{} {}", schema.table_name, id.0));
    }

    let body = if body_sections.is_empty() {
        String::new()
    } else {
        format!("\n{}\n", body_sections.join("\n\n"))
    };

    let reference_section = if ref_lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", ref_lines.join("\n"))
    };

    // Derive date: schema column "date" in extra > ad-hoc INSERT "date" column > ID-derived
    let date = extra
        .remove("date")
        .and_then(|v| match v {
            Value::String(s) => Some(s),
            _ => None,
        })
        .or_else(|| col_values.get("date").cloned())
        .or_else(|| Some(format!("{}-{}-{}", &id.0[0..4], &id.0[4..6], &id.0[6..8])));

    ParsedDoogat {
        meta: DoogatMeta {
            id: Some(id.clone()),
            title: title_value,
            date,
            doogat_type: Some(schema.table_name.clone()),
            tags: vec![],
            extra,
        },
        body,
        sections: vec![],
        reference_section,
        inline_fields,
        links,
        body_tags: vec![],
        checkboxes: vec![],
        path: format!("ddb/{}.md", id.0),
        updated_at: None,
    }
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

    let mut columns = Vec::new();
    for item in columns_seq {
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
        columns.push(ColumnDef {
            name,
            data_type,
            references,
            zone,
            required,
            search_boost,
            allowed_values,
            default_value,
        });
    }

    let crdt_strategy = doogat
        .meta
        .extra
        .get("crdt_strategy")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let template_sections = doogat
        .meta
        .extra
        .get("template_sections")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let folder = doogat
        .meta
        .extra
        .get("folder")
        .map(|v| matches!(v, crate::types::Value::Bool(true)) || v.as_str() == Some("true"))
        .unwrap_or(false);

    let stale_after_days = doogat
        .meta
        .extra
        .get("stale_after_days")
        .and_then(|v| v.as_f64())
        .map(|n| n as u32);

    let title_template = doogat
        .meta
        .extra
        .get("title_template")
        .and_then(|v| v.as_str())
        .map(String::from);

    let origin = doogat
        .meta
        .extra
        .get("origin")
        .and_then(|v| v.as_str())
        .map(String::from);

    let unique_together = doogat
        .meta
        .extra
        .get("unique_together")
        .and_then(|v| v.as_sequence())
        .and_then(|outer| {
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
        });

    Ok(TableSchema {
        table_name,
        columns,
        crdt_strategy,
        template_sections,
        folder,
        stale_after_days,
        title_template,
        origin,
        unique_together,
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
