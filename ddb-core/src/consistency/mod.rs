mod migrations;
mod zone_migrate;

pub use migrations::{migrate_all, Migration};
pub use zone_migrate::zone_migrate_all;
#[cfg(test)]
pub(crate) use migrations::{built_in_migrations, read_migration_version};
#[cfg(test)]
pub(crate) use zone_migrate::{
    detect_current_zone, extract_from_zone, insert_into_zone, remove_body_section,
    remove_from_zone,
};

use std::collections::HashSet;

use crate::error::Result;
use crate::indexer::Index;
use crate::traits::DoogatStore;
use crate::types::{
    ColumnDef, DoogatFix, Fix, FixReport, ParsedDoogat, TableSchema, TitleSource, Zone,
};
use regex::Regex;
use std::sync::OnceLock;

/// Convert a string to kebab-case.
///
/// Splits on uppercase boundaries, underscores, and spaces; lowercases; joins with `-`.
/// Examples: `CamelCase` → `camel-case`, `snake_case` → `snake-case`, `XMLParser` → `xml-parser`.
pub fn to_kebab_case(s: &str) -> String {
    let mut words = Vec::new();
    let mut current = String::new();

    for ch in s.chars() {
        if ch == '_' || ch == ' ' || ch == '-' {
            if !current.is_empty() {
                words.push(current.clone());
                current.clear();
            }
        } else if ch.is_uppercase() {
            if !current.is_empty() {
                // Split on uppercase boundary, but handle consecutive uppercase (acronyms)
                // e.g. "XMLParser" → ["XML", "Parser"] → "xml-parser"
                let last_was_upper = current.chars().last().is_some_and(|c| c.is_uppercase());
                if !last_was_upper {
                    words.push(current.clone());
                    current.clear();
                }
            }
            current.push(ch);
        } else {
            // Lowercase char after a run of uppercase: split the acronym
            // e.g. "XMLParser" at 'a': current="XMLP", split into "XML" + "P" → "Pa..."
            if current.len() > 1
                && current.chars().last().is_some_and(|c| c.is_uppercase())
                && current
                    .chars()
                    .rev()
                    .nth(1)
                    .is_some_and(|c| c.is_uppercase())
            {
                let last = current.pop().expect("current has >= 2 chars per preceding check");
                words.push(current.clone());
                current.clear();
                current.push(last);
            }
            current.push(ch);
        }
    }
    if !current.is_empty() {
        words.push(current);
    }

    words
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("-")
}

/// Check whether a key is already in kebab-case.
fn is_kebab_case(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    // kebab-case: lowercase letters, digits, and hyphens only; no leading/trailing hyphens
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

fn re_unfilled_placeholder() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{[^}]+\}").expect("valid regex"))
}

/// Extract content under a `## heading` in the body, returning lines between that heading
/// and the next `## ` heading (or end of body).
pub fn extract_body_section(body: &str, heading: &str) -> Option<String> {
    let target = format!("## {heading}");
    let mut lines = body.lines();
    let found = lines.by_ref().any(|l| l.trim() == target);
    if !found {
        return None;
    }
    let mut content = Vec::new();
    for line in lines {
        if line.starts_with("## ") {
            break;
        }
        content.push(line);
    }
    let text = content.join("\n").trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Extract a column's value from a parsed doogat based on its effective zone.
fn extract_column_value(parsed: &ParsedDoogat, col: &ColumnDef) -> Option<String> {
    match col.effective_zone() {
        Zone::Frontmatter => parsed.meta.extra.get(&col.name).and_then(|v| match v {
            crate::types::Value::String(s) => Some(s.clone()),
            crate::types::Value::Number(n) => Some(n.to_string()),
            crate::types::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }),
        Zone::Body => extract_body_section(&parsed.body, &col.name),
        Zone::Reference => parsed
            .inline_fields
            .iter()
            .find(|f| f.key == col.name && matches!(f.zone, Zone::Reference))
            .map(|f| f.value.clone()),
    }
}

/// Interpolate a title template with column values, stripping unfilled placeholders.
fn interpolate_title_template(
    template: &str,
    parsed: &ParsedDoogat,
    schema: &TableSchema,
) -> Option<String> {
    let mut rendered = template.to_string();
    for col in &schema.columns {
        if let Some(val) = extract_column_value(parsed, col) {
            rendered = rendered.replace(&format!("{{{}}}", col.name), &val);
        }
    }
    let rendered = re_unfilled_placeholder()
        .replace_all(&rendered, "")
        .trim()
        .to_string();
    if rendered.is_empty() {
        None
    } else {
        Some(rendered)
    }
}

fn detect_title_compliance(parsed: &ParsedDoogat, schema: &TableSchema, fixes: &mut Vec<Fix>) {
    let template = match schema.title_template {
        Some(ref t) => t,
        None => return,
    };
    let expected = match interpolate_title_template(template, parsed, schema) {
        Some(e) => e,
        None => return,
    };
    let actual = parsed.meta.title.as_deref().unwrap_or("");
    if actual != expected {
        fixes.push(Fix::TitleNonCompliant { expected });
    }
}

/// Detect consistency fixes needed for a parsed doogat.
///
/// Returns a list of fixes ordered by severity (errors first, then warnings, then info).
/// Does not modify the doogat — use [`apply_fixes`] to apply.
pub fn detect_fixes(parsed: &ParsedDoogat, schema: Option<&TableSchema>) -> Vec<Fix> {
    let mut fixes = Vec::new();

    detect_tag_issues(&parsed.meta.tags, &mut fixes);
    detect_default_issues(parsed, &mut fixes);
    // Skip title fixes for typedefs — their titles are SQL table names
    if parsed.meta.doogat_type.as_deref() != Some("_typedef") {
        detect_title_issues(parsed, &mut fixes);
    }
    detect_key_issues(parsed, &mut fixes);
    detect_cross_zone_issues(parsed, &mut fixes);
    if let Some(s) = schema {
        detect_schema_issues(parsed, s, &mut fixes);
        detect_title_compliance(parsed, s, &mut fixes);
    }

    // Flag manual typedefs
    if parsed.meta.doogat_type.as_deref() == Some("_typedef") {
        if let Some(origin) = parsed.meta.extra.get("origin") {
            if origin.as_str() == Some("manual") {
                let type_name = parsed
                    .meta
                    .title
                    .clone()
                    .unwrap_or_else(|| "unknown".into());
                fixes.push(Fix::ManualTypedef { type_name });
            }
        }
    }

    // Stable sort: errors first, then warnings, then info
    fixes.sort_by_key(|f| std::cmp::Reverse(f.severity()));
    fixes
}

fn detect_tag_issues(tags: &[String], fixes: &mut Vec<Fix>) {
    // 1. Hash-prefixed tags (detect first since strip affects dedup)
    let hash_tags: Vec<String> = tags
        .iter()
        .filter(|t| t.starts_with('#'))
        .cloned()
        .collect();
    if !hash_tags.is_empty() {
        fixes.push(Fix::TagsStrippedHash { tags: hash_tags });
    }

    // 2. Duplicate tags (case-insensitive, checked on post-strip values
    //    so "#apple" + "apple" is detected as a duplicate)
    let normalized: Vec<String> = tags
        .iter()
        .map(|t| t.strip_prefix('#').unwrap_or(t).to_lowercase())
        .collect();
    let mut seen = HashSet::new();
    let mut removed = Vec::new();
    for (i, norm) in normalized.iter().enumerate() {
        if !seen.insert(norm.clone()) {
            removed.push(tags[i].clone());
        }
    }
    if !removed.is_empty() {
        fixes.push(Fix::TagsDeduped { removed });
    }

    // 3. Unsorted tags (checked on post-strip post-dedup projection)
    let mut unique_normalized: Vec<String> = Vec::new();
    let mut seen2 = HashSet::new();
    for norm in &normalized {
        if seen2.insert(norm.clone()) {
            unique_normalized.push(norm.clone());
        }
    }
    if unique_normalized.len() > 1 {
        let sorted = unique_normalized.windows(2).all(|w| w[0] <= w[1]);
        if !sorted {
            fixes.push(Fix::TagsSorted);
        }
    }
}

fn detect_default_issues(parsed: &ParsedDoogat, fixes: &mut Vec<Fix>) {
    // Missing type
    if parsed.meta.doogat_type.is_none()
        || parsed
            .meta
            .doogat_type
            .as_ref()
            .is_some_and(|t| t.is_empty())
    {
        fixes.push(Fix::DefaultSet {
            field: "type".to_string(),
            value: "note".to_string(),
        });
    }
}

fn detect_title_issues(parsed: &ParsedDoogat, fixes: &mut Vec<Fix>) {
    let title = parsed.meta.title.as_deref().unwrap_or("");

    if title.is_empty() {
        // Try to derive from first H1 heading
        if let Some(section) = parsed.sections.iter().find(|s| s.level == 1) {
            fixes.push(Fix::TitleDerived {
                source: TitleSource::FirstH1(section.heading.clone()),
            });
        } else {
            // Derive from filename
            let derived = title_from_path(&parsed.path);
            if !derived.is_empty() {
                fixes.push(Fix::TitleDerived {
                    source: TitleSource::Filename(derived),
                });
            }
        }
        return;
    }

    // Title whitespace
    if title != title.trim() {
        fixes.push(Fix::TitleTrimmed);
    }

    // Title capitalization (check after trim)
    let check_title = title.trim();
    if let Some(first) = check_title.chars().next() {
        if first.is_lowercase() {
            fixes.push(Fix::TitleCapitalized);
        }
    }

    // H1 alignment: if title exists and first H1 heading differs
    if let Some(section) = parsed.sections.iter().find(|s| s.level == 1) {
        if section.heading != check_title {
            fixes.push(Fix::H1Aligned {
                old_h1: section.heading.clone(),
                new_h1: check_title.to_string(),
            });
        }
    }
}

/// Derive a title from a doogat file path.
///
/// Strips the `ddb/` prefix and `.md` extension, removes the 14-digit ID prefix,
/// replaces `_` and `-` with spaces, trims, and capitalizes the first letter.
fn title_from_path(path: &str) -> String {
    let filename = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".md");

    // Strip 14-digit ID prefix (with optional separator)
    let stripped = if filename.len() >= 14 && filename[..14].chars().all(|c| c.is_ascii_digit()) {
        let rest = &filename[14..];
        rest.trim_start_matches(['-', '_'])
    } else {
        filename
    };

    if stripped.is_empty() {
        return String::new();
    }

    let title = stripped.replace(['_', '-'], " ");
    let title = title.trim();
    capitalize_first(title)
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let upper: String = first.to_uppercase().collect();
            upper + chars.as_str()
        }
    }
}

fn detect_key_issues(parsed: &ParsedDoogat, fixes: &mut Vec<Fix>) {
    for key in parsed.meta.extra.keys() {
        if !is_kebab_case(key) {
            let normalized = to_kebab_case(key);
            if normalized != *key {
                fixes.push(Fix::KeyNormalized {
                    old: key.clone(),
                    new: normalized,
                });
            }
        }
    }
}

fn detect_cross_zone_issues(parsed: &ParsedDoogat, fixes: &mut Vec<Fix>) {
    let known_fm_keys: HashSet<&str> = {
        let mut keys: HashSet<&str> = parsed.meta.extra.keys().map(|k| k.as_str()).collect();
        if parsed.meta.title.is_some() {
            keys.insert("title");
        }
        if parsed.meta.doogat_type.is_some() {
            keys.insert("type");
        }
        if !parsed.meta.tags.is_empty() {
            keys.insert("tags");
        }
        if parsed.meta.date.is_some() {
            keys.insert("date");
        }
        if parsed.meta.id.is_some() {
            keys.insert("id");
        }
        keys
    };

    for field in &parsed.inline_fields {
        if field.zone != crate::types::Zone::Frontmatter
            && known_fm_keys.contains(field.key.as_str())
        {
            fixes.push(Fix::CrossZoneResolved {
                key: field.key.clone(),
                kept_zone: crate::types::Zone::Frontmatter,
            });
        }
    }
}

fn detect_schema_issues(parsed: &ParsedDoogat, schema: &TableSchema, fixes: &mut Vec<Fix>) {
    let known_fields: HashSet<&str> = {
        let mut keys: HashSet<&str> = parsed.meta.extra.keys().map(|k| k.as_str()).collect();
        if parsed.meta.title.is_some() {
            keys.insert("title");
        }
        if parsed.meta.doogat_type.is_some() {
            keys.insert("type");
        }
        if !parsed.meta.tags.is_empty() {
            keys.insert("tags");
        }
        if parsed.meta.date.is_some() {
            keys.insert("date");
        }
        if parsed.meta.id.is_some() {
            keys.insert("id");
        }
        // Also count inline fields
        for field in &parsed.inline_fields {
            keys.insert(&field.key);
        }
        keys
    };

    for col in &schema.columns {
        if col.required && !known_fields.contains(col.name.as_str()) {
            if let Some(ref default) = col.default_value {
                fixes.push(Fix::DefaultSet {
                    field: col.name.clone(),
                    value: default.clone(),
                });
            }
        }
    }
}

/// Apply detected fixes to a parsed doogat and return the re-serialized content.
///
/// Modifies the doogat in-place, then calls `parser::serialize()` to produce the output string.
/// Fixes are applied in the order given (typically severity-descending from `detect_fixes`).
pub fn apply_fixes(parsed: &mut ParsedDoogat, fixes: &[Fix]) -> Result<String> {
    for fix in fixes {
        match fix {
            Fix::TagsStrippedHash { .. } => {
                parsed.meta.tags = parsed
                    .meta
                    .tags
                    .iter()
                    .map(|t| t.strip_prefix('#').unwrap_or(t).to_string())
                    .collect();
            }
            Fix::TagsDeduped { .. } => {
                let mut seen = HashSet::new();
                parsed.meta.tags.retain(|t| seen.insert(t.to_lowercase()));
            }
            Fix::TagsSorted => {
                parsed.meta.tags.sort_by_key(|a| a.to_lowercase());
            }
            Fix::DefaultSet { field, value } => {
                if field == "type" {
                    parsed.meta.doogat_type = Some(value.clone());
                } else {
                    parsed
                        .meta
                        .extra
                        .insert(field.clone(), crate::types::Value::String(value.clone()));
                }
            }
            Fix::TitleDerived { source } => {
                let title = match source {
                    TitleSource::FirstH1(h) => h.clone(),
                    TitleSource::Filename(n) => n.clone(),
                };
                parsed.meta.title = Some(title);
            }
            Fix::TitleTrimmed => {
                if let Some(ref mut title) = parsed.meta.title {
                    *title = title.trim().to_string();
                }
            }
            Fix::TitleCapitalized => {
                if let Some(ref mut title) = parsed.meta.title {
                    *title = capitalize_first(title);
                }
            }
            Fix::KeyNormalized { old, new } => {
                if let Some(value) = parsed.meta.extra.remove(old) {
                    parsed.meta.extra.insert(new.clone(), value);
                }
            }
            Fix::H1Aligned { old_h1, new_h1 } => {
                // Replace first matching H1 line in body
                let target = format!("# {old_h1}");
                let replacement = format!("# {new_h1}");
                if let Some(pos) = parsed.body.find(&target) {
                    let end = pos + target.len();
                    parsed.body.replace_range(pos..end, &replacement);
                }
            }
            Fix::CrossZoneResolved { key, kept_zone } => {
                if *kept_zone == Zone::Frontmatter {
                    // Remove inline field line from body: `key:: value`
                    remove_inline_field_from_body(&mut parsed.body, key);
                }
            }
            Fix::FieldRenamed { old, new } => {
                if let Some(value) = parsed.meta.extra.remove(old) {
                    parsed.meta.extra.insert(new.clone(), value);
                }
            }
            Fix::TypeNormalized { new, .. } => {
                parsed.meta.doogat_type = Some(new.clone());
            }
            Fix::ManualTypedef { .. } => {} // informational only, no mutation
            Fix::TitleNonCompliant { expected } => {
                // Update H1 in body if present
                if let Some(ref old_title) = parsed.meta.title {
                    let old_h1 = format!("# {old_title}");
                    let new_h1 = format!("# {expected}");
                    if let Some(pos) = parsed.body.find(&old_h1) {
                        let end = pos + old_h1.len();
                        parsed.body.replace_range(pos..end, &new_h1);
                    }
                }
                parsed.meta.title = Some(expected.clone());
            }
            Fix::ZoneMigrated { .. } => {} // zone migration applied separately
        }
    }

    Ok(crate::parser::serialize(parsed))
}

/// Scan all doogats, detect and apply fixes, commit atomically.
///
/// When `dry_run` is true, detects fixes and builds a report but does not modify files or commit.
pub fn fix_all(repo: &impl DoogatStore, index: &Index, dry_run: bool) -> Result<FixReport> {
    let paths = repo.list_doogats()?;
    let typedef_schemas = index.load_all_typedefs(repo);

    let mut report = FixReport {
        files_scanned: 0,
        files_fixed: 0,
        fixes: Vec::new(),
    };

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

        let schema = parsed
            .meta
            .doogat_type
            .as_ref()
            .and_then(|t| typedef_schemas.get(t));

        let fixes = detect_fixes(&parsed, schema);
        if fixes.is_empty() {
            continue;
        }

        if !dry_run {
            let new_content = apply_fixes(&mut parsed, &fixes)?;
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
            "fix: auto-fix {} issues across {} doogats",
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

/// Remove lines matching `key:: ...` from body text.
pub(super) fn remove_inline_field_from_body(body: &mut String, key: &str) {
    let prefix = format!("{key}::");
    let lines: Vec<&str> = body.lines().collect();
    let filtered: Vec<&str> = lines
        .into_iter()
        .filter(|line| !line.trim_start().starts_with(&prefix))
        .collect();
    let mut result = filtered.join("\n");
    // Preserve trailing newline if original had one
    if body.ends_with('\n') {
        result.push('\n');
    }
    *body = result;
}

#[cfg(test)]
mod tests;
