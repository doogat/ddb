use std::collections::HashSet;

use crate::error::Result;
use crate::indexer::Index;
use crate::traits::ZettelStore;
use crate::types::{
    ColumnDef, Fix, FixReport, ParsedZettel, TableSchema, TitleSource, ZettelFix, Zone,
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
                let last = current.pop().unwrap();
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
    RE.get_or_init(|| Regex::new(r"\{[^}]+\}").unwrap())
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

/// Extract a column's value from a parsed zettel based on its effective zone.
fn extract_column_value(parsed: &ParsedZettel, col: &ColumnDef) -> Option<String> {
    match col.effective_zone() {
        Zone::Frontmatter => parsed.meta.extra.get(&col.name).and_then(|v| {
            match v {
                crate::types::Value::String(s) => Some(s.clone()),
                crate::types::Value::Number(n) => Some(n.to_string()),
                crate::types::Value::Bool(b) => Some(b.to_string()),
                _ => None,
            }
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
fn interpolate_title_template(template: &str, parsed: &ParsedZettel, schema: &TableSchema) -> Option<String> {
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

fn detect_title_compliance(parsed: &ParsedZettel, schema: &TableSchema, fixes: &mut Vec<Fix>) {
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

/// Detect consistency fixes needed for a parsed zettel.
///
/// Returns a list of fixes ordered by severity (errors first, then warnings, then info).
/// Does not modify the zettel — use [`apply_fixes`] to apply.
pub fn detect_fixes(parsed: &ParsedZettel, schema: Option<&TableSchema>) -> Vec<Fix> {
    let mut fixes = Vec::new();

    detect_tag_issues(&parsed.meta.tags, &mut fixes);
    detect_default_issues(parsed, &mut fixes);
    detect_title_issues(parsed, &mut fixes);
    detect_key_issues(parsed, &mut fixes);
    detect_cross_zone_issues(parsed, &mut fixes);
    if let Some(s) = schema {
        detect_schema_issues(parsed, s, &mut fixes);
        detect_title_compliance(parsed, s, &mut fixes);
    }

    // Flag manual typedefs
    if parsed.meta.zettel_type.as_deref() == Some("_typedef") {
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

fn detect_default_issues(parsed: &ParsedZettel, fixes: &mut Vec<Fix>) {
    // Missing type
    if parsed.meta.zettel_type.is_none()
        || parsed
            .meta
            .zettel_type
            .as_ref()
            .is_some_and(|t| t.is_empty())
    {
        fixes.push(Fix::DefaultSet {
            field: "type".to_string(),
            value: "note".to_string(),
        });
    }
}

fn detect_title_issues(parsed: &ParsedZettel, fixes: &mut Vec<Fix>) {
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

/// Derive a title from a zettel file path.
///
/// Strips the `zettelkasten/` prefix and `.md` extension, removes the 14-digit ID prefix,
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

fn detect_key_issues(parsed: &ParsedZettel, fixes: &mut Vec<Fix>) {
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

fn detect_cross_zone_issues(parsed: &ParsedZettel, fixes: &mut Vec<Fix>) {
    let known_fm_keys: HashSet<&str> = {
        let mut keys: HashSet<&str> = parsed.meta.extra.keys().map(|k| k.as_str()).collect();
        if parsed.meta.title.is_some() {
            keys.insert("title");
        }
        if parsed.meta.zettel_type.is_some() {
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

fn detect_schema_issues(parsed: &ParsedZettel, schema: &TableSchema, fixes: &mut Vec<Fix>) {
    let known_fields: HashSet<&str> = {
        let mut keys: HashSet<&str> = parsed.meta.extra.keys().map(|k| k.as_str()).collect();
        if parsed.meta.title.is_some() {
            keys.insert("title");
        }
        if parsed.meta.zettel_type.is_some() {
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

/// Apply detected fixes to a parsed zettel and return the re-serialized content.
///
/// Modifies the zettel in-place, then calls `parser::serialize()` to produce the output string.
/// Fixes are applied in the order given (typically severity-descending from `detect_fixes`).
pub fn apply_fixes(parsed: &mut ParsedZettel, fixes: &[Fix]) -> Result<String> {
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
                    parsed.meta.zettel_type = Some(value.clone());
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
                parsed.meta.zettel_type = Some(new.clone());
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
            Fix::ZoneMigrated { .. } => {}      // zone migration applied separately
        }
    }

    Ok(crate::parser::serialize(parsed))
}

/// Scan all zettels, detect and apply fixes, commit atomically.
///
/// When `dry_run` is true, detects fixes and builds a report but does not modify files or commit.
pub fn fix_all(repo: &impl ZettelStore, index: &Index, dry_run: bool) -> Result<FixReport> {
    let paths = repo.list_zettels()?;
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
            .zettel_type
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
        report.fixes.push(ZettelFix {
            path: path.clone(),
            applied: fixes,
        });
    }

    if !dry_run && !writes.is_empty() {
        let total_fixes: usize = report.fixes.iter().map(|f| f.applied.len()).sum();
        let msg = format!(
            "fix: auto-fix {} issues across {} zettels",
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

// ── Migration framework ──────────────────────────────────────────

/// A field-level migration that transforms zettels during format evolution.
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub apply: fn(&mut ParsedZettel) -> Vec<Fix>,
}

/// Built-in migrations for known field renames and type normalizations.
fn built_in_migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 1,
            name: "zkn-id-to-id",
            apply: |p| {
                if let Some(value) = p.meta.extra.get("zkn-id").cloned() {
                    if p.meta.id.is_none() {
                        if let Some(s) = value.as_str() {
                            p.meta.id = Some(crate::types::ZettelId(s.to_string()));
                        }
                    }
                    return vec![Fix::FieldRenamed {
                        old: "zkn-id".into(),
                        new: "id".into(),
                    }];
                }
                vec![]
            },
        },
        Migration {
            version: 2,
            name: "tag-to-tags",
            apply: |p| {
                if let Some(value) = p.meta.extra.get("tag").cloned() {
                    if let Some(s) = value.as_str() {
                        if p.meta.tags.is_empty() {
                            p.meta.tags = vec![s.to_string()];
                        }
                    }
                    return vec![Fix::FieldRenamed {
                        old: "tag".into(),
                        new: "tags".into(),
                    }];
                }
                vec![]
            },
        },
        Migration {
            version: 3,
            name: "type-normalize",
            apply: |p| {
                let old_type = match p.meta.zettel_type.as_deref() {
                    Some(t) => t.to_string(),
                    None => return vec![],
                };
                let new_type = match old_type.as_str() {
                    "loop" => "project",
                    "wiki-article" | "zettel" => "note",
                    _ => return vec![],
                };
                p.meta.zettel_type = Some(new_type.to_string());
                vec![Fix::TypeNormalized {
                    old: old_type,
                    new: new_type.to_string(),
                }]
            },
        },
    ]
}

/// Read the current migration version from `.zdb/migration-version`.
fn read_migration_version(repo: &impl crate::traits::ZettelSource) -> u32 {
    repo.read_file(".zdb/migration-version")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Run pending migrations on all zettels.
///
/// Applies migrations with version > current, commits changes, and updates the version file.
pub fn migrate_all(repo: &impl ZettelStore, dry_run: bool) -> Result<FixReport> {
    let current_version = read_migration_version(repo);
    let migrations = built_in_migrations();
    let pending: Vec<&Migration> = migrations
        .iter()
        .filter(|m| m.version > current_version)
        .collect();

    if pending.is_empty() {
        return Ok(FixReport::default());
    }

    let max_version = pending.iter().map(|m| m.version).max().unwrap_or(0);
    let paths = repo.list_zettels()?;

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

        let mut all_fixes = Vec::new();
        for migration in &pending {
            let fixes = (migration.apply)(&mut parsed);
            // Remove migrated fields from extras
            for fix in &fixes {
                if let Fix::FieldRenamed { old, .. } = fix {
                    parsed.meta.extra.remove(old);
                }
            }
            all_fixes.extend(fixes);
        }

        if all_fixes.is_empty() {
            continue;
        }

        if !dry_run {
            let new_content = crate::parser::serialize(&parsed);
            writes.push((path.clone(), new_content));
        }

        report.files_fixed += 1;
        report.fixes.push(ZettelFix {
            path: path.clone(),
            applied: all_fixes,
        });
    }

    if !dry_run {
        // Always include version file in the commit (even if no zettels changed)
        let version_content = max_version.to_string();
        writes.push((".zdb/migration-version".to_string(), version_content));

        let names: Vec<&str> = pending.iter().map(|m| m.name).collect();
        if report.files_fixed > 0 {
            let total_fixes: usize = report.fixes.iter().map(|f| f.applied.len()).sum();
            let msg = format!(
                "fix: migrate {} fields across {} zettels ({})",
                total_fixes,
                report.files_fixed,
                names.join(", ")
            );
            let write_refs: Vec<(&str, &str)> = writes
                .iter()
                .map(|(p, c)| (p.as_str(), c.as_str()))
                .collect();
            repo.commit_batch(&write_refs, &[], &msg)?;
        } else {
            // No zettels affected, but still advance the version
            repo.commit_file(
                ".zdb/migration-version",
                &max_version.to_string(),
                &format!(
                    "fix: advance migration version to {max_version} ({})",
                    names.join(", ")
                ),
            )?;
        }
    }

    Ok(report)
}

/// Migrate zettel data between zones to match current typedef schema.
///
/// For each typed zettel, compares where column data currently lives vs the
/// typedef's zone assignment. Rewrites mismatched columns in-place.
pub fn zone_migrate_all(
    repo: &impl ZettelStore,
    index: &Index,
    dry_run: bool,
) -> Result<FixReport> {
    let paths = repo.list_zettels()?;
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
            .zettel_type
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
        report.fixes.push(ZettelFix {
            path: path.clone(),
            applied: fixes,
        });
    }

    if !dry_run && !writes.is_empty() {
        let total_fixes: usize = report.fixes.iter().map(|f| f.applied.len()).sum();
        let msg = format!(
            "fix: zone-migrate {} columns across {} zettels",
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
fn detect_current_zone(parsed: &ParsedZettel, col_name: &str) -> Option<Zone> {
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
fn extract_from_zone(parsed: &ParsedZettel, col_name: &str, zone: &Zone) -> Option<String> {
    match zone {
        Zone::Frontmatter => parsed.meta.extra.get(col_name).and_then(|v| match v {
            crate::types::Value::String(s) => Some(s.clone()),
            crate::types::Value::Number(n) => Some(n.to_string()),
            crate::types::Value::Bool(b) => Some(b.to_string()),
            _ => Some(format!("{v:?}")),
        }),
        Zone::Body => extract_body_section(&parsed.body, col_name),
        Zone::Reference => parsed
            .inline_fields
            .iter()
            .find(|f| f.key == col_name && matches!(f.zone, Zone::Reference))
            .map(|f| f.value.clone()),
    }
}

/// Remove data from its current zone.
fn remove_from_zone(parsed: &mut ParsedZettel, col_name: &str, zone: &Zone) {
    match zone {
        Zone::Frontmatter => {
            parsed.meta.extra.remove(col_name);
        }
        Zone::Body => {
            remove_body_section(&mut parsed.body, col_name);
        }
        Zone::Reference => {
            // Remove from inline_fields
            parsed
                .inline_fields
                .retain(|f| !(f.key == col_name && matches!(f.zone, Zone::Reference)));
            // Remove from reference_section text
            remove_reference_line(&mut parsed.reference_section, col_name);
        }
    }
}

/// Insert data into the target zone.
fn insert_into_zone(parsed: &mut ParsedZettel, col_name: &str, value: &str, zone: &Zone) {
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
            parsed
                .body
                .push_str(&format!("## {col_name}\n\n{value}\n"));
        }
        Zone::Reference => {
            // Add as `- col:: [[value]]` line
            let line = format!("- {col_name}:: [[{value}]]");
            if !parsed.reference_section.is_empty()
                && !parsed.reference_section.ends_with('\n')
            {
                parsed.reference_section.push('\n');
            }
            parsed.reference_section.push_str(&line);
            parsed.reference_section.push('\n');
            // Also add to inline_fields for consistency
            parsed.inline_fields.push(crate::types::InlineField {
                key: col_name.to_string(),
                value: value.to_string(),
                zone: Zone::Reference,
            });
        }
    }
}

/// Remove a `## heading` section from body text (heading + content until next ## or end).
fn remove_body_section(body: &mut String, heading: &str) {
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

/// Remove lines matching `key:: ...` from body text.
fn remove_inline_field_from_body(body: &mut String, key: &str) {
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
mod tests {
    use super::*;
    use crate::types::{InlineField, Section, Severity, TitleSource, ZettelMeta, Zone};
    use std::collections::BTreeMap;

    fn empty_parsed() -> ParsedZettel {
        ParsedZettel {
            meta: ZettelMeta {
                id: Some(crate::types::ZettelId("20260315120000".to_string())),
                title: Some("Test note".to_string()),
                date: Some("2026-03-15".to_string()),
                zettel_type: Some("note".to_string()),
                tags: vec!["alpha".to_string(), "beta".to_string()],
                extra: BTreeMap::new(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260315120000.md".to_string(),
        }
    }

    #[test]
    fn no_fixes_clean_zettel() {
        let parsed = empty_parsed();
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes.is_empty(),
            "clean zettel should need no fixes: {fixes:?}"
        );
    }

    #[test]
    fn detect_duplicate_tags() {
        let mut parsed = empty_parsed();
        parsed.meta.tags = vec!["a".into(), "b".into(), "a".into()];
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes
                .iter()
                .any(|f| matches!(f, Fix::TagsDeduped { removed } if removed == &["a"])),
            "should detect duplicate tag: {fixes:?}"
        );
    }

    #[test]
    fn detect_hash_tags() {
        let mut parsed = empty_parsed();
        parsed.meta.tags = vec!["#gtd".into(), "work".into()];
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes
                .iter()
                .any(|f| matches!(f, Fix::TagsStrippedHash { tags } if tags == &["#gtd"])),
            "should detect #-prefixed tag: {fixes:?}"
        );
    }

    #[test]
    fn detect_unsorted_tags() {
        let mut parsed = empty_parsed();
        parsed.meta.tags = vec!["zebra".into(), "apple".into()];
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes.iter().any(|f| matches!(f, Fix::TagsSorted)),
            "should detect unsorted tags: {fixes:?}"
        );
    }

    #[test]
    fn detect_missing_type() {
        let mut parsed = empty_parsed();
        parsed.meta.zettel_type = None;
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::DefaultSet { field, value } if field == "type" && value == "note")
            ),
            "should detect missing type: {fixes:?}"
        );
    }

    #[test]
    fn detect_missing_title_from_h1() {
        let mut parsed = empty_parsed();
        parsed.meta.title = None;
        parsed.sections = vec![Section {
            heading: "My Heading".to_string(),
            level: 1,
            content: String::new(),
        }];
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::TitleDerived { source: TitleSource::FirstH1(h) } if h == "My Heading")
            ),
            "should derive title from H1: {fixes:?}"
        );
    }

    #[test]
    fn detect_missing_title_from_filename() {
        let mut parsed = empty_parsed();
        parsed.meta.title = None;
        parsed.sections = vec![];
        parsed.path = "zettelkasten/20260315120000-project-plan.md".to_string();
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::TitleDerived { source: TitleSource::Filename(n) } if n == "Project plan")
            ),
            "should derive title from filename: {fixes:?}"
        );
    }

    #[test]
    fn detect_camel_case_key() {
        let mut parsed = empty_parsed();
        parsed
            .meta
            .extra
            .insert("CamelKey".into(), crate::types::Value::String("val".into()));
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::KeyNormalized { old, new } if old == "CamelKey" && new == "camel-key")
            ),
            "should detect CamelCase key: {fixes:?}"
        );
    }

    #[test]
    fn detect_snake_case_key() {
        let mut parsed = empty_parsed();
        parsed.meta.extra.insert(
            "snake_key".into(),
            crate::types::Value::String("val".into()),
        );
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::KeyNormalized { old, new } if old == "snake_key" && new == "snake-key")
            ),
            "should detect snake_case key: {fixes:?}"
        );
    }

    #[test]
    fn detect_cross_zone_duplicate() {
        let mut parsed = empty_parsed();
        parsed
            .meta
            .extra
            .insert("project".into(), crate::types::Value::String("foo".into()));
        parsed.inline_fields.push(InlineField {
            key: "project".into(),
            value: "bar".into(),
            zone: Zone::Body,
        });
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::CrossZoneResolved { key, kept_zone } if key == "project" && *kept_zone == Zone::Frontmatter)
            ),
            "should detect cross-zone duplicate: {fixes:?}"
        );
    }

    #[test]
    fn severity_classification() {
        assert_eq!(
            Fix::CrossZoneResolved {
                key: "k".into(),
                kept_zone: Zone::Frontmatter
            }
            .severity(),
            Severity::Error
        );
        assert_eq!(
            Fix::DefaultSet {
                field: "type".into(),
                value: "note".into()
            }
            .severity(),
            Severity::Warning
        );
        assert_eq!(
            Fix::TitleDerived {
                source: TitleSource::Filename("t".into())
            }
            .severity(),
            Severity::Warning
        );
        assert_eq!(Fix::TagsSorted.severity(), Severity::Info);
        assert_eq!(Fix::TitleTrimmed.severity(), Severity::Info);
        assert_eq!(Fix::TitleCapitalized.severity(), Severity::Info);
        assert_eq!(
            Fix::TagsDeduped { removed: vec![] }.severity(),
            Severity::Info
        );
        assert_eq!(
            Fix::TagsStrippedHash { tags: vec![] }.severity(),
            Severity::Info
        );
        assert_eq!(
            Fix::KeyNormalized {
                old: "a".into(),
                new: "b".into()
            }
            .severity(),
            Severity::Info
        );
        assert_eq!(
            Fix::H1Aligned {
                old_h1: "a".into(),
                new_h1: "b".into()
            }
            .severity(),
            Severity::Info
        );
        assert_eq!(
            Fix::FieldRenamed {
                old: "a".into(),
                new: "b".into()
            }
            .severity(),
            Severity::Warning
        );
        assert_eq!(
            Fix::TypeNormalized {
                old: "loop".into(),
                new: "project".into()
            }
            .severity(),
            Severity::Info
        );
    }

    #[test]
    fn to_kebab_case_camel() {
        assert_eq!(to_kebab_case("CamelCase"), "camel-case");
    }

    #[test]
    fn to_kebab_case_snake() {
        assert_eq!(to_kebab_case("snake_case"), "snake-case");
    }

    #[test]
    fn to_kebab_case_acronym() {
        assert_eq!(to_kebab_case("XMLParser"), "xml-parser");
    }

    #[test]
    fn to_kebab_case_already_kebab() {
        assert_eq!(to_kebab_case("already-kebab"), "already-kebab");
    }

    #[test]
    fn to_kebab_case_mixed() {
        assert_eq!(to_kebab_case("myField_Name"), "my-field-name");
    }

    #[test]
    fn title_from_path_with_id_and_slug() {
        assert_eq!(
            title_from_path("zettelkasten/20260315120000-project-plan.md"),
            "Project plan"
        );
    }

    #[test]
    fn title_from_path_id_only() {
        assert_eq!(title_from_path("zettelkasten/20260315120000.md"), "");
    }

    #[test]
    fn title_from_path_underscore_slug() {
        assert_eq!(
            title_from_path("zettelkasten/20260315120000_my_note.md"),
            "My note"
        );
    }

    #[test]
    fn detect_title_trim() {
        let mut parsed = empty_parsed();
        parsed.meta.title = Some("  spaced  ".to_string());
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes.iter().any(|f| matches!(f, Fix::TitleTrimmed)),
            "should detect untrimmed title: {fixes:?}"
        );
    }

    #[test]
    fn detect_title_capitalize() {
        let mut parsed = empty_parsed();
        parsed.meta.title = Some("lowercase start".to_string());
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes.iter().any(|f| matches!(f, Fix::TitleCapitalized)),
            "should detect uncapitalized title: {fixes:?}"
        );
    }

    #[test]
    fn empty_tags_no_fix() {
        let mut parsed = empty_parsed();
        parsed.meta.tags = vec![];
        let fixes = detect_fixes(&parsed, None);
        assert!(
            !fixes
                .iter()
                .any(|f| matches!(f, Fix::DefaultSet { field, .. } if field == "tags")),
            "empty tags should not trigger a fix: {fixes:?}"
        );
    }

    #[test]
    fn detect_schema_required_field_with_default() {
        let parsed = empty_parsed();
        let schema = crate::types::TableSchema {
            table_name: "note".into(),
            columns: vec![crate::types::ColumnDef {
                name: "priority".into(),
                data_type: "INTEGER".into(),
                references: None,
                zone: None,
                required: true,
                search_boost: None,
                allowed_values: None,
                default_value: Some("0".into()),
            }],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
        };
        let fixes = detect_fixes(&parsed, Some(&schema));
        assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::DefaultSet { field, value } if field == "priority" && value == "0")
            ),
            "should detect missing required field from schema: {fixes:?}"
        );
    }

    #[test]
    fn detect_schema_required_field_present_no_fix() {
        let mut parsed = empty_parsed();
        parsed
            .meta
            .extra
            .insert("priority".into(), crate::types::Value::Number(5.0));
        let schema = crate::types::TableSchema {
            table_name: "note".into(),
            columns: vec![crate::types::ColumnDef {
                name: "priority".into(),
                data_type: "INTEGER".into(),
                references: None,
                zone: None,
                required: true,
                search_boost: None,
                allowed_values: None,
                default_value: Some("0".into()),
            }],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
        };
        let fixes = detect_fixes(&parsed, Some(&schema));
        assert!(
            !fixes
                .iter()
                .any(|f| matches!(f, Fix::DefaultSet { field, .. } if field == "priority")),
            "present field should not trigger schema fix: {fixes:?}"
        );
    }

    #[test]
    fn detect_hash_apple_plus_apple_dedup() {
        let mut parsed = empty_parsed();
        parsed.meta.tags = vec!["#apple".into(), "apple".into()];
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes
                .iter()
                .any(|f| matches!(f, Fix::TagsDeduped { removed } if !removed.is_empty())),
            "should detect post-strip duplicate: {fixes:?}"
        );
    }

    #[test]
    fn apply_hash_apple_plus_apple_roundtrip() {
        let mut parsed = empty_parsed();
        parsed.meta.tags = vec!["#apple".into(), "apple".into()];
        let fixes = detect_fixes(&parsed, None);
        let content = apply_fixes(&mut parsed, &fixes).unwrap();
        let reparsed = crate::parser::parse(&content, &parsed.path).unwrap();
        assert_eq!(reparsed.meta.tags, vec!["apple".to_string()]);
        // Idempotent
        let second_fixes = detect_fixes(&reparsed, None);
        assert!(
            !second_fixes
                .iter()
                .any(|f| matches!(f, Fix::TagsDeduped { .. } | Fix::TagsStrippedHash { .. })),
            "should be idempotent: {second_fixes:?}"
        );
    }

    // ── H1 alignment tests ──────────────────────────────────────────

    #[test]
    fn detect_h1_mismatch() {
        let mut parsed = empty_parsed();
        parsed.meta.title = Some("My Note".to_string());
        parsed.sections = vec![Section {
            heading: "Old Title".to_string(),
            level: 1,
            content: String::new(),
        }];
        let fixes = detect_fixes(&parsed, None);
        assert!(
            fixes.iter().any(
                |f| matches!(f, Fix::H1Aligned { old_h1, new_h1 } if old_h1 == "Old Title" && new_h1 == "My Note")
            ),
            "should detect H1 mismatch: {fixes:?}"
        );
    }

    #[test]
    fn detect_h1_match_no_fix() {
        let mut parsed = empty_parsed();
        parsed.meta.title = Some("Same Title".to_string());
        parsed.sections = vec![Section {
            heading: "Same Title".to_string(),
            level: 1,
            content: String::new(),
        }];
        let fixes = detect_fixes(&parsed, None);
        assert!(
            !fixes.iter().any(|f| matches!(f, Fix::H1Aligned { .. })),
            "matching H1 should not produce fix: {fixes:?}"
        );
    }

    #[test]
    fn detect_no_h1_no_fix() {
        let mut parsed = empty_parsed();
        parsed.meta.title = Some("My Note".to_string());
        parsed.sections = vec![Section {
            heading: "Sub heading".to_string(),
            level: 2,
            content: String::new(),
        }];
        let fixes = detect_fixes(&parsed, None);
        assert!(
            !fixes.iter().any(|f| matches!(f, Fix::H1Aligned { .. })),
            "no H1 should not produce alignment fix: {fixes:?}"
        );
    }

    #[test]
    fn apply_h1_alignment() {
        let mut parsed = empty_parsed();
        parsed.meta.title = Some("My Note".to_string());
        parsed.body = "# Old Title\n\nSome content.\n".to_string();
        parsed.sections = vec![Section {
            heading: "Old Title".to_string(),
            level: 1,
            content: "\nSome content.\n".to_string(),
        }];
        let fixes = vec![Fix::H1Aligned {
            old_h1: "Old Title".into(),
            new_h1: "My Note".into(),
        }];
        let result = apply_fixes(&mut parsed, &fixes).unwrap();
        assert!(result.contains("# My Note\n"));
        assert!(!result.contains("# Old Title"));
    }

    // ── apply_fixes tests ──────────────────────────────────────────

    #[test]
    fn apply_tag_dedup() {
        let mut parsed = empty_parsed();
        parsed.meta.tags = vec!["a".into(), "b".into(), "a".into()];
        let fixes = vec![Fix::TagsDeduped {
            removed: vec!["a".into()],
        }];
        let result = apply_fixes(&mut parsed, &fixes).unwrap();
        assert!(result.contains("  - a\n  - b\n"));
        assert_eq!(result.matches("  - a").count(), 1);
    }

    #[test]
    fn apply_tag_sort() {
        let mut parsed = empty_parsed();
        parsed.meta.tags = vec!["zebra".into(), "apple".into()];
        let fixes = vec![Fix::TagsSorted];
        let result = apply_fixes(&mut parsed, &fixes).unwrap();
        let tag_section = result
            .lines()
            .skip_while(|l| !l.starts_with("tags:"))
            .skip(1)
            .take_while(|l| l.starts_with("  - "))
            .collect::<Vec<_>>();
        assert_eq!(tag_section, vec!["  - apple", "  - zebra"]);
    }

    #[test]
    fn apply_tag_strip_hash() {
        let mut parsed = empty_parsed();
        parsed.meta.tags = vec!["#gtd".into(), "work".into()];
        let fixes = vec![Fix::TagsStrippedHash {
            tags: vec!["#gtd".into()],
        }];
        let result = apply_fixes(&mut parsed, &fixes).unwrap();
        assert!(result.contains("  - gtd\n"));
        assert!(!result.contains("#gtd"));
    }

    #[test]
    fn apply_default_type() {
        let mut parsed = empty_parsed();
        parsed.meta.zettel_type = None;
        let fixes = vec![Fix::DefaultSet {
            field: "type".into(),
            value: "note".into(),
        }];
        let result = apply_fixes(&mut parsed, &fixes).unwrap();
        assert!(result.contains("type: note"));
    }

    #[test]
    fn apply_title_derived() {
        let mut parsed = empty_parsed();
        parsed.meta.title = None;
        let fixes = vec![Fix::TitleDerived {
            source: TitleSource::FirstH1("Derived Title".into()),
        }];
        let result = apply_fixes(&mut parsed, &fixes).unwrap();
        assert!(result.contains("title: Derived Title"));
    }

    #[test]
    fn apply_key_normalize() {
        let mut parsed = empty_parsed();
        parsed
            .meta
            .extra
            .insert("CamelKey".into(), crate::types::Value::String("val".into()));
        let fixes = vec![Fix::KeyNormalized {
            old: "CamelKey".into(),
            new: "camel-key".into(),
        }];
        let result = apply_fixes(&mut parsed, &fixes).unwrap();
        assert!(result.contains("camel-key: val"));
        assert!(!result.contains("CamelKey"));
    }

    #[test]
    fn apply_cross_zone_resolved() {
        let mut parsed = empty_parsed();
        parsed.body = "Some text.\nproject:: bar\nMore text.\n".to_string();
        parsed
            .meta
            .extra
            .insert("project".into(), crate::types::Value::String("foo".into()));
        let fixes = vec![Fix::CrossZoneResolved {
            key: "project".into(),
            kept_zone: Zone::Frontmatter,
        }];
        let result = apply_fixes(&mut parsed, &fixes).unwrap();
        assert!(!result.contains("project:: bar"));
        assert!(result.contains("project: foo"));
    }

    #[test]
    fn round_trip_fidelity() {
        let mut parsed = empty_parsed();
        parsed.meta.tags = vec![
            "#gtd".into(),
            "zebra".into(),
            "apple".into(),
            "apple".into(),
        ];
        parsed.meta.zettel_type = None;
        parsed.meta.title = Some("  lowercase  ".into());
        parsed
            .meta
            .extra
            .insert("CamelKey".into(), crate::types::Value::String("val".into()));

        let fixes = detect_fixes(&parsed, None);
        assert!(!fixes.is_empty());

        let content = apply_fixes(&mut parsed, &fixes).unwrap();

        // Re-parse and detect again — should find no fixes
        let reparsed = crate::parser::parse(&content, &parsed.path).unwrap();
        let second_fixes = detect_fixes(&reparsed, None);
        assert!(
            second_fixes.is_empty(),
            "round-trip should produce clean zettel, but found: {second_fixes:?}"
        );
    }

    // ── migration tests ──────────────────────────────────────────

    #[test]
    fn migrate_zkn_id_to_id() {
        let mut parsed = empty_parsed();
        parsed.meta.id = None;
        parsed.meta.extra.insert(
            "zkn-id".into(),
            crate::types::Value::String("20260101120000".into()),
        );

        let migrations = super::built_in_migrations();
        let m = &migrations[0]; // v1: zkn-id-to-id
        let fixes = (m.apply)(&mut parsed);

        assert_eq!(fixes.len(), 1);
        assert!(
            matches!(&fixes[0], Fix::FieldRenamed { old, new } if old == "zkn-id" && new == "id")
        );
        assert_eq!(
            parsed.meta.id,
            Some(crate::types::ZettelId("20260101120000".into()))
        );
    }

    #[test]
    fn migrate_tag_singular_to_tags() {
        let mut parsed = empty_parsed();
        parsed.meta.tags = vec![];
        parsed
            .meta
            .extra
            .insert("tag".into(), crate::types::Value::String("gtd".into()));

        let migrations = super::built_in_migrations();
        let m = &migrations[1]; // v2: tag-to-tags
        let fixes = (m.apply)(&mut parsed);

        assert_eq!(fixes.len(), 1);
        assert!(
            matches!(&fixes[0], Fix::FieldRenamed { old, new } if old == "tag" && new == "tags")
        );
        assert_eq!(parsed.meta.tags, vec!["gtd".to_string()]);
    }

    #[test]
    fn migrate_type_loop_to_project() {
        let mut parsed = empty_parsed();
        parsed.meta.zettel_type = Some("loop".into());

        let migrations = super::built_in_migrations();
        let m = &migrations[2]; // v3: type-normalize
        let fixes = (m.apply)(&mut parsed);

        assert_eq!(fixes.len(), 1);
        assert!(
            matches!(&fixes[0], Fix::TypeNormalized { old, new } if old == "loop" && new == "project")
        );
        assert_eq!(parsed.meta.zettel_type, Some("project".into()));
    }

    #[test]
    fn migrate_type_zettel_to_note() {
        let mut parsed = empty_parsed();
        parsed.meta.zettel_type = Some("zettel".into());

        let migrations = super::built_in_migrations();
        let m = &migrations[2];
        let fixes = (m.apply)(&mut parsed);

        assert_eq!(fixes.len(), 1);
        assert!(
            matches!(&fixes[0], Fix::TypeNormalized { old, new } if old == "zettel" && new == "note")
        );
    }

    #[test]
    fn migrate_type_normal_no_change() {
        let mut parsed = empty_parsed();
        parsed.meta.zettel_type = Some("project".into());

        let migrations = super::built_in_migrations();
        let m = &migrations[2];
        let fixes = (m.apply)(&mut parsed);
        assert!(fixes.is_empty());
    }

    #[test]
    fn migration_version_tracking() {
        // read_migration_version returns 0 when file doesn't exist
        // (can't test file I/O without a repo, but we test the function logic)
        assert_eq!(super::read_migration_version(&MockSource), 0);
    }

    /// Minimal mock for testing read_migration_version when file doesn't exist.
    struct MockSource;

    impl crate::traits::ZettelSource for MockSource {
        fn list_zettels(&self) -> crate::error::Result<Vec<String>> {
            Ok(vec![])
        }
        fn read_file(&self, _path: &str) -> crate::error::Result<String> {
            Err(crate::error::ZettelError::Git("not found".into()))
        }
        fn head_oid(&self) -> crate::error::Result<crate::types::CommitHash> {
            Ok(crate::types::CommitHash("abc".into()))
        }
        fn diff_paths(
            &self,
            _old: &str,
            _new: &str,
        ) -> crate::error::Result<Vec<(crate::types::DiffKind, String)>> {
            Ok(vec![])
        }
        fn read_files_batch(
            &self,
            _paths: &[String],
        ) -> crate::error::Result<Vec<(String, crate::error::Result<String>)>> {
            Ok(vec![])
        }
    }

    fn make_schema_with_template(template: &str, columns: Vec<ColumnDef>) -> TableSchema {
        TableSchema {
            table_name: "widget".into(),
            columns,
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: Some(template.into()),
            origin: Some("ddl".into()),
        }
    }

    #[test]
    fn title_noncompliant_detected() {
        let mut parsed = empty_parsed();
        parsed.meta.title = Some("Foo".into());
        parsed.meta.zettel_type = Some("widget".into());
        parsed
            .meta
            .extra
            .insert("name".into(), crate::types::Value::String("Bar".into()));
        let schema = make_schema_with_template("{name} Widget", vec![ColumnDef {
            name: "name".into(),
            data_type: "TEXT".into(),
            references: None,
            zone: Some(Zone::Frontmatter),
            required: false,
            search_boost: None,
            allowed_values: None,
            default_value: None,
        }]);
        let fixes = detect_fixes(&parsed, Some(&schema));
        assert!(
            fixes
                .iter()
                .any(|f| matches!(f, Fix::TitleNonCompliant { expected } if expected == "Bar Widget")),
            "should detect title mismatch: {fixes:?}"
        );
    }

    #[test]
    fn title_compliant_not_flagged() {
        let mut parsed = empty_parsed();
        parsed.meta.title = Some("Bar Widget".into());
        parsed.meta.zettel_type = Some("widget".into());
        parsed
            .meta
            .extra
            .insert("name".into(), crate::types::Value::String("Bar".into()));
        let schema = make_schema_with_template("{name} Widget", vec![ColumnDef {
            name: "name".into(),
            data_type: "TEXT".into(),
            references: None,
            zone: Some(Zone::Frontmatter),
            required: false,
            search_boost: None,
            allowed_values: None,
            default_value: None,
        }]);
        let fixes = detect_fixes(&parsed, Some(&schema));
        assert!(
            !fixes.iter().any(|f| matches!(f, Fix::TitleNonCompliant { .. })),
            "compliant title should not be flagged: {fixes:?}"
        );
    }

    #[test]
    fn title_template_unfilled_placeholders_stripped() {
        let mut parsed = empty_parsed();
        parsed.meta.title = Some("Wrong".into());
        parsed.meta.zettel_type = Some("widget".into());
        parsed
            .meta
            .extra
            .insert("name".into(), crate::types::Value::String("Bar".into()));
        let schema = make_schema_with_template("{name} {missing}", vec![
            ColumnDef {
                name: "name".into(),
                data_type: "TEXT".into(),
                references: None,
                zone: Some(Zone::Frontmatter),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
            },
            ColumnDef {
                name: "missing".into(),
                data_type: "TEXT".into(),
                references: None,
                zone: Some(Zone::Frontmatter),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
            },
        ]);
        let fixes = detect_fixes(&parsed, Some(&schema));
        assert!(
            fixes
                .iter()
                .any(|f| matches!(f, Fix::TitleNonCompliant { expected } if expected == "Bar")),
            "unfilled placeholders should be stripped: {fixes:?}"
        );
    }

    #[test]
    fn title_compliance_no_template_skipped() {
        let mut parsed = empty_parsed();
        parsed.meta.zettel_type = Some("widget".into());
        let schema = TableSchema {
            table_name: "widget".into(),
            columns: vec![],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: None,
        };
        let fixes = detect_fixes(&parsed, Some(&schema));
        assert!(
            !fixes.iter().any(|f| matches!(f, Fix::TitleNonCompliant { .. })),
            "no template means no compliance check: {fixes:?}"
        );
    }

    #[test]
    fn title_noncompliant_applied() {
        let mut parsed = empty_parsed();
        parsed.meta.title = Some("Old Title".into());
        parsed.body = "# Old Title\n\nSome content".into();
        let fixes = vec![Fix::TitleNonCompliant {
            expected: "New Title".into(),
        }];
        let result = apply_fixes(&mut parsed, &fixes).unwrap();
        assert_eq!(parsed.meta.title.as_deref(), Some("New Title"));
        assert!(result.contains("# New Title"), "H1 should be updated in body");
    }

    // ── Zone migration tests ────────────────────────────────────────

    #[test]
    fn migrate_body_to_frontmatter() {
        let mut parsed = empty_parsed();
        parsed.body = "## description\n\nSome content here\n".into();
        let col = ColumnDef {
            name: "description".into(),
            data_type: "TEXT".into(),
            references: None,
            zone: Some(Zone::Frontmatter),
            required: false,
            search_boost: None,
            allowed_values: None,
            default_value: None,
        };

        let current = detect_current_zone(&parsed, "description");
        assert_eq!(current, Some(Zone::Body));

        let value = extract_from_zone(&parsed, "description", &Zone::Body).unwrap();
        assert_eq!(value, "Some content here");

        remove_from_zone(&mut parsed, "description", &Zone::Body);
        insert_into_zone(&mut parsed, "description", &value, &col.effective_zone());

        assert_eq!(
            parsed.meta.extra.get("description"),
            Some(&crate::types::Value::String("Some content here".into()))
        );
        assert!(!parsed.body.contains("## description"));
    }

    #[test]
    fn migrate_frontmatter_to_body() {
        let mut parsed = empty_parsed();
        parsed.meta.extra.insert(
            "notes".into(),
            crate::types::Value::String("Important stuff".into()),
        );

        let current = detect_current_zone(&parsed, "notes");
        assert_eq!(current, Some(Zone::Frontmatter));

        let value = extract_from_zone(&parsed, "notes", &Zone::Frontmatter).unwrap();
        remove_from_zone(&mut parsed, "notes", &Zone::Frontmatter);
        insert_into_zone(&mut parsed, "notes", &value, &Zone::Body);

        assert!(parsed.meta.extra.get("notes").is_none());
        assert!(parsed.body.contains("## notes"));
        assert!(parsed.body.contains("Important stuff"));
    }

    #[test]
    fn migrate_to_reference() {
        let mut parsed = empty_parsed();
        parsed.meta.extra.insert(
            "category".into(),
            crate::types::Value::String("20260301120000".into()),
        );

        let value = extract_from_zone(&parsed, "category", &Zone::Frontmatter).unwrap();
        remove_from_zone(&mut parsed, "category", &Zone::Frontmatter);
        insert_into_zone(&mut parsed, "category", &value, &Zone::Reference);

        assert!(parsed.meta.extra.get("category").is_none());
        assert!(parsed.reference_section.contains("- category:: [[20260301120000]]"));
        assert!(parsed
            .inline_fields
            .iter()
            .any(|f| f.key == "category" && f.value == "20260301120000"));
    }

    #[test]
    fn migrate_from_reference() {
        let mut parsed = empty_parsed();
        parsed.reference_section = "- source:: [[20260301120000]]\n".into();
        parsed.inline_fields.push(InlineField {
            key: "source".into(),
            value: "20260301120000".into(),
            zone: Zone::Reference,
        });

        let current = detect_current_zone(&parsed, "source");
        assert_eq!(current, Some(Zone::Reference));

        let value = extract_from_zone(&parsed, "source", &Zone::Reference).unwrap();
        remove_from_zone(&mut parsed, "source", &Zone::Reference);
        insert_into_zone(&mut parsed, "source", &value, &Zone::Frontmatter);

        assert!(parsed.reference_section.trim().is_empty() || !parsed.reference_section.contains("source"));
        assert_eq!(
            parsed.meta.extra.get("source"),
            Some(&crate::types::Value::String("20260301120000".into()))
        );
    }

    #[test]
    fn migrate_preserves_subheadings() {
        let mut parsed = empty_parsed();
        parsed.body = "## notes\n\nTop content\n\n### Sub-heading\n\nSub content\n\n## other\n\nOther stuff\n".into();

        let value = extract_from_zone(&parsed, "notes", &Zone::Body).unwrap();
        assert!(value.contains("### Sub-heading"), "sub-headings preserved in extraction");
        assert!(value.contains("Sub content"));

        remove_from_zone(&mut parsed, "notes", &Zone::Body);
        assert!(!parsed.body.contains("## notes"));
        assert!(parsed.body.contains("## other"), "other section preserved");
    }

    #[test]
    fn migrate_multiline_body_to_frontmatter() {
        let mut parsed = empty_parsed();
        parsed.body = "## bio\n\nLine one\nLine two\nLine three\n".into();

        let value = extract_from_zone(&parsed, "bio", &Zone::Body).unwrap();
        assert!(value.contains("Line one\nLine two\nLine three"));

        remove_from_zone(&mut parsed, "bio", &Zone::Body);
        insert_into_zone(&mut parsed, "bio", &value, &Zone::Frontmatter);

        let stored = parsed.meta.extra.get("bio").unwrap();
        match stored {
            crate::types::Value::String(s) => {
                assert!(s.contains("Line one"));
                assert!(s.contains("Line three"));
            }
            _ => panic!("expected String value"),
        }
    }

    #[test]
    fn migrate_idempotent() {
        let mut parsed = empty_parsed();
        parsed.meta.extra.insert(
            "status".into(),
            crate::types::Value::String("active".into()),
        );
        let col = ColumnDef {
            name: "status".into(),
            data_type: "TEXT".into(),
            references: None,
            zone: Some(Zone::Frontmatter),
            required: false,
            search_boost: None,
            allowed_values: None,
            default_value: None,
        };

        // Data already in correct zone
        let current = detect_current_zone(&parsed, "status").unwrap();
        assert_eq!(current, col.effective_zone(), "already in correct zone");
    }

    #[test]
    fn migrate_no_data_no_changes() {
        let parsed = empty_parsed();
        // Column exists in schema but zettel has no data for it
        let current = detect_current_zone(&parsed, "nonexistent");
        assert_eq!(current, None);
    }
}
