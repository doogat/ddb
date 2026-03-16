use std::collections::HashSet;

use crate::error::Result;
use crate::indexer::Index;
use crate::traits::ZettelStore;
use crate::types::{Fix, FixReport, ParsedZettel, TableSchema, TitleSource, ZettelFix, Zone};

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

/// Detect consistency fixes needed for a parsed zettel.
///
/// Returns a list of fixes ordered by severity (errors first, then warnings, then info).
/// Does not modify the zettel — use [`apply_fixes`] to apply.
pub fn detect_fixes(parsed: &ParsedZettel, _schema: Option<&TableSchema>) -> Vec<Fix> {
    let mut fixes = Vec::new();

    detect_tag_issues(&parsed.meta.tags, &mut fixes);
    detect_default_issues(parsed, &mut fixes);
    detect_title_issues(parsed, &mut fixes);
    detect_key_issues(parsed, &mut fixes);
    detect_cross_zone_issues(parsed, &mut fixes);

    // Stable sort: errors first, then warnings, then info
    fixes.sort_by_key(|f| std::cmp::Reverse(f.severity()));
    fixes
}

fn detect_tag_issues(tags: &[String], fixes: &mut Vec<Fix>) {
    // 1. Duplicate tags (case-insensitive)
    let mut seen = HashSet::new();
    let mut removed = Vec::new();
    for tag in tags {
        let lower = tag.to_lowercase();
        if !seen.insert(lower) {
            removed.push(tag.clone());
        }
    }
    if !removed.is_empty() {
        fixes.push(Fix::TagsDeduped { removed });
    }

    // 2. Hash-prefixed tags
    let hash_tags: Vec<String> = tags
        .iter()
        .filter(|t| t.starts_with('#'))
        .cloned()
        .collect();
    if !hash_tags.is_empty() {
        fixes.push(Fix::TagsStrippedHash { tags: hash_tags });
    }

    // 3. Unsorted tags (case-insensitive, check after dedup/strip would apply)
    // We check the current state; apply_fixes will sort after other tag fixes.
    if tags.len() > 1 {
        let sorted = tags
            .windows(2)
            .all(|w| w[0].to_lowercase() <= w[1].to_lowercase());
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

/// Apply detected fixes to a parsed zettel and return the re-serialized content.
///
/// Modifies the zettel in-place, then calls `parser::serialize()` to produce the output string.
/// Fixes are applied in a deterministic order: tag fixes first, then defaults, title, keys,
/// and finally cross-zone resolution.
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
}
