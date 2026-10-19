use std::collections::BTreeMap;

use crate::types::{ParsedDoogat, TableSchema, Zone};

use super::super::helpers::to_yaml_value;

/// Apply UPDATE SET assignments to a ParsedDoogat according to schema zone mapping.
///
/// Visibility is `pub(crate)` so the service-layer typed UPDATE path
/// (`service::crud::update_doogat_parsed` / `prepare_update`) can route
/// REFERENCES-column updates to the reference zone instead of frontmatter.
/// PRD 00134 cycle-1 review C1 task #2: keeping this in sync with the
/// SQL-engine UPDATE path is the only way the auto-junction stays accurate
/// after a service-path UPDATE — `materialize_single` reads REFERENCES
/// values from `inline_fields` (reference zone), so a stale reference line
/// would re-insert the OLD junction row even after the DELETE pass.
pub(crate) fn apply_updates_to_doogat(
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
    let lines: Vec<&str> = reference.lines().collect();
    let has_existing = lines.iter().any(|l| l.starts_with(&prefix));

    // PRD 00134 cycle-3 review escalation: clearing a REFERENCES column
    // that was never set is a no-op. Without this guard, the `!found`
    // branch below would append `- {key}:: [[]]` to the reference section,
    // dirtying the raw markdown. `extract_multi_reference_values` already
    // filters empty values so no ghost junction row is created either way,
    // but the cosmetic regression matters for human-readable doogats.
    if !has_existing && new_val.is_empty() {
        return;
    }

    let new_line = format!("- {key}:: [[{new_val}]]");
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
pub(in crate::sql_engine) fn rename_key_in_doogat(
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
