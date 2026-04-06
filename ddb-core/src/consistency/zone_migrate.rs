use crate::error::Result;
use crate::indexer::Index;
use crate::traits::DoogatStore;
use crate::types::{DoogatFix, Fix, FixReport, ParsedDoogat, Zone};

use super::{extract_body_section, remove_inline_field_from_body};

/// Migrate doogat data between zones to match current typedef schema.
///
/// For each typed doogat, compares where column data currently lives vs the
/// typedef's zone assignment. Rewrites mismatched columns in-place.
pub fn zone_migrate_all(
    repo: &impl DoogatStore,
    index: &Index,
    dry_run: bool,
) -> Result<FixReport> {
    let paths = repo.list_doogats()?;
    let typedef_schemas = index.load_all_typedefs(repo);

    let mut report = FixReport::default();
    let mut writes: Vec<(String, String)> = Vec::new();

    for path in &paths {
        let content = match repo.read_file(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let mut parsed = match crate::parser::parse(&content, path) {
            Ok(p) => p,
            Err(_) => continue,
        };
        report.files_scanned += 1;

        let schema = match parsed
            .meta
            .doogat_type
            .as_ref()
            .and_then(|t| typedef_schemas.get(t))
        {
            Some(s) => s,
            None => continue,
        };

        let mut fixes = Vec::new();
        for col in &schema.columns {
            let target_zone = col.effective_zone();
            let current_zone = detect_current_zone(&parsed, &col.name);
            let current_zone = match current_zone {
                Some(z) => z,
                None => continue, // no data for this column
            };
            if current_zone == target_zone {
                continue;
            }
            // Extract value from current zone
            let value = match extract_from_zone(&parsed, &col.name, &current_zone) {
                Some(v) => v,
                None => continue,
            };
            // Remove from current zone
            remove_from_zone(&mut parsed, &col.name, &current_zone);
            // Insert into target zone
            insert_into_zone(&mut parsed, &col.name, &value, &target_zone);
            fixes.push(Fix::ZoneMigrated {
                column: col.name.clone(),
                from: current_zone,
                to: target_zone,
            });
        }

        if fixes.is_empty() {
            continue;
        }

        if !dry_run {
            let new_content = crate::parser::serialize(&parsed);
            writes.push((path.clone(), new_content));
        }
        report.files_fixed += 1;
        report.fixes.push(DoogatFix {
            path: path.clone(),
            applied: fixes,
        });
    }

    if !dry_run && !writes.is_empty() {
        let total_fixes: usize = report.fixes.iter().map(|f| f.applied.len()).sum();
        let msg = format!(
            "fix: zone-migrate {} columns across {} doogats",
            total_fixes, report.files_fixed
        );
        let write_refs: Vec<(&str, &str)> = writes
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        repo.commit_batch(&write_refs, &[], &msg)?;
    }

    Ok(report)
}

/// Detect which zone a column's data currently lives in.
pub(crate) fn detect_current_zone(parsed: &ParsedDoogat, col_name: &str) -> Option<Zone> {
    // Check frontmatter first
    if parsed.meta.extra.contains_key(col_name) {
        return Some(Zone::Frontmatter);
    }
    // Check reference zone (inline fields)
    if parsed
        .inline_fields
        .iter()
        .any(|f| f.key == col_name && matches!(f.zone, Zone::Reference))
    {
        return Some(Zone::Reference);
    }
    // Check body section
    if extract_body_section(&parsed.body, col_name).is_some() {
        return Some(Zone::Body);
    }
    // Check body inline fields
    if parsed
        .inline_fields
        .iter()
        .any(|f| f.key == col_name && matches!(f.zone, Zone::Body))
    {
        return Some(Zone::Body);
    }
    None
}

/// Extract value from its current zone.
pub(crate) fn extract_from_zone(parsed: &ParsedDoogat, col_name: &str, zone: &Zone) -> Option<String> {
    match zone {
        Zone::Frontmatter => parsed.meta.extra.get(col_name).and_then(|v| match v {
            crate::types::Value::String(s) => Some(s.clone()),
            crate::types::Value::Number(n) => Some(n.to_string()),
            crate::types::Value::Bool(b) => Some(b.to_string()),
            crate::types::Value::Map(_) | crate::types::Value::List(_) => serde_yaml::to_string(v)
                .ok()
                .map(|s| s.trim_end().to_string()),
        }),
        Zone::Body => {
            // Check ## heading sections first, then body inline fields
            extract_body_section(&parsed.body, col_name).or_else(|| {
                parsed
                    .inline_fields
                    .iter()
                    .find(|f| f.key == col_name && matches!(f.zone, Zone::Body))
                    .map(|f| f.value.clone())
            })
        }
        Zone::Reference => {
            // Collect ALL matching values (multi-value references)
            let vals: Vec<&str> = parsed
                .inline_fields
                .iter()
                .filter(|f| f.key == col_name && matches!(f.zone, Zone::Reference))
                .map(|f| f.value.as_str())
                .collect();
            if vals.is_empty() {
                None
            } else {
                Some(vals.join(","))
            }
        }
    }
}

/// Remove data from its current zone.
pub(crate) fn remove_from_zone(parsed: &mut ParsedDoogat, col_name: &str, zone: &Zone) {
    match zone {
        Zone::Frontmatter => {
            parsed.meta.extra.remove(col_name);
        }
        Zone::Body => {
            remove_body_section(&mut parsed.body, col_name);
            // Also remove body inline fields and their text
            parsed
                .inline_fields
                .retain(|f| !(f.key == col_name && matches!(f.zone, Zone::Body)));
            remove_inline_field_from_body(&mut parsed.body, col_name);
        }
        Zone::Reference => {
            parsed
                .inline_fields
                .retain(|f| !(f.key == col_name && matches!(f.zone, Zone::Reference)));
            remove_reference_line(&mut parsed.reference_section, col_name);
        }
    }
}

/// Insert data into the target zone.
pub(crate) fn insert_into_zone(parsed: &mut ParsedDoogat, col_name: &str, value: &str, zone: &Zone) {
    match zone {
        Zone::Frontmatter => {
            parsed.meta.extra.insert(
                col_name.to_string(),
                crate::types::Value::String(value.to_string()),
            );
        }
        Zone::Body => {
            // Append as a new ## section
            if !parsed.body.is_empty() && !parsed.body.ends_with('\n') {
                parsed.body.push('\n');
            }
            if !parsed.body.is_empty() {
                parsed.body.push('\n');
            }
            parsed.body.push_str(&format!("## {col_name}\n\n{value}\n"));
        }
        Zone::Reference => {
            // Handle comma-separated multi-values from migration
            let values: Vec<&str> = value.split(',').collect();
            for val in &values {
                let line = format!("- {col_name}:: [[{val}]]");
                if !parsed.reference_section.is_empty() && !parsed.reference_section.ends_with('\n')
                {
                    parsed.reference_section.push('\n');
                }
                parsed.reference_section.push_str(&line);
                parsed.reference_section.push('\n');
                parsed.inline_fields.push(crate::types::InlineField {
                    key: col_name.to_string(),
                    value: val.to_string(),
                    zone: Zone::Reference,
                });
            }
        }
    }
}

/// Remove a `## heading` section from body text (heading + content until next ## or end).
pub(crate) fn remove_body_section(body: &mut String, heading: &str) {
    let target = format!("## {heading}");
    let lines: Vec<&str> = body.lines().collect();
    let mut result = Vec::new();
    let mut skipping = false;
    for line in &lines {
        if line.trim() == target {
            skipping = true;
            continue;
        }
        if skipping && line.starts_with("## ") {
            skipping = false;
        }
        if !skipping {
            result.push(*line);
        }
    }
    let mut new_body = result.join("\n");
    while new_body.contains("\n\n\n") {
        new_body = new_body.replace("\n\n\n", "\n\n");
    }
    if body.ends_with('\n') && !new_body.ends_with('\n') {
        new_body.push('\n');
    }
    *body = new_body;
}

/// Remove `- key:: ...` lines from reference section text.
fn remove_reference_line(reference: &mut String, key: &str) {
    let prefix = format!("- {key}::");
    let lines: Vec<&str> = reference.lines().collect();
    let filtered: Vec<&str> = lines
        .into_iter()
        .filter(|line| !line.trim_start().starts_with(&prefix))
        .collect();
    let mut result = filtered.join("\n");
    if reference.ends_with('\n') && !result.ends_with('\n') && !result.is_empty() {
        result.push('\n');
    }
    *reference = result;
}
