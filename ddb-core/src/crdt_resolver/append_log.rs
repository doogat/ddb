use super::body::{merge_body, merge_reference};
use super::frontmatter::{merge_frontmatter, parse_zones};
use crate::error::Result;
use crate::hlc::Hlc;
use crate::parser;
use crate::types::{ConflictFile, ResolvedFile};

/// Resolve conflicts using append-log strategy.
/// Body log sections: parse entries, dedup, union, sort chronologically.
/// Non-log body sections: default text CRDT merge.
/// Frontmatter + references: same as default.
pub fn resolve_append_log(conflicts: Vec<ConflictFile>) -> Result<Vec<ResolvedFile>> {
    let mut resolved = Vec::new();

    for conflict in conflicts {
        let ancestor_content = conflict.ancestor.as_deref().unwrap_or("");

        let ancestor = parse_zones(ancestor_content)?;
        let ours = parse_zones(&conflict.ours)?;
        let theirs = parse_zones(&conflict.theirs)?;

        // Frontmatter: same as default (Automerge Map)
        let (merged_fm, fm_crdt_bytes) = merge_frontmatter(
            &ancestor.raw_frontmatter,
            &ours.raw_frontmatter,
            &theirs.raw_frontmatter,
        )?;

        // Body: section-aware merge
        let merged_body = merge_body_append_log(
            &ancestor.body,
            &ours.body,
            &theirs.body,
            &conflict.ours_hlc,
            &conflict.theirs_hlc,
        )?;

        // Reference: same as default (set merge)
        let merged_ref = merge_reference(
            &ancestor.reference_section,
            &ours.reference_section,
            &theirs.reference_section,
        )?;

        let meta = parser::parse_frontmatter(&merged_fm, &conflict.path)?;
        let inline_fields = parser::extract_inline_fields(&merged_body, &merged_ref)?;
        let wikilinks = parser::extract_links(&merged_fm, &merged_body, &merged_ref);

        let parsed = crate::types::ParsedDoogat {
            meta,
            body: merged_body,
            sections: vec![],
            reference_section: merged_ref,
            inline_fields,
            links: wikilinks,
            body_tags: vec![],
            checkboxes: vec![],
            path: conflict.path.clone(),
            updated_at: None,
        };

        resolved.push(ResolvedFile {
            path: conflict.path,
            content: parser::serialize(&parsed),
            fm_crdt_bytes: Some(fm_crdt_bytes),
        });
    }

    Ok(resolved)
}

/// Split body into sections by `## ` headings.
fn split_body_sections(body: &str) -> Vec<(Option<String>, String)> {
    let mut sections: Vec<(Option<String>, String)> = Vec::new();
    let mut current_heading: Option<String> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in body.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            // Push previous section
            if !current_lines.is_empty() || current_heading.is_some() {
                sections.push((current_heading.take(), current_lines.join("\n")));
                current_lines.clear();
            }
            current_heading = Some(heading.trim().to_string());
        } else {
            current_lines.push(line);
        }
    }
    // Push final section
    sections.push((current_heading, current_lines.join("\n")));

    sections
}

/// Check if a section body is a log section (entries matching `- [x] YYYY-MM-DD` or `- [ ] YYYY-MM-DD`).
fn is_log_section(body: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let log_pattern = RE
        .get_or_init(|| regex::Regex::new(r"^- \[[ xi]\] \d{4}-\d{2}-\d{2}").expect("valid regex"));
    body.lines()
        .filter(|l| !l.trim().is_empty())
        .any(|l| log_pattern.is_match(l))
}

/// Parse log entries: each entry starts with `- [` and may have continuation lines.
fn parse_log_entries(body: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut current = String::new();

    for line in body.lines() {
        if line.starts_with("- [") {
            if !current.is_empty() {
                entries.push(current.trim_end().to_string());
            }
            current = line.to_string();
        } else if !current.is_empty() && !line.trim().is_empty() {
            current.push('\n');
            current.push_str(line);
        } else if current.is_empty() && !line.trim().is_empty() {
            // Non-entry content before first entry — skip
        } else if !current.is_empty() && line.trim().is_empty() {
            current.push('\n');
        }
    }
    if !current.is_empty() {
        entries.push(current.trim_end().to_string());
    }
    entries
}

/// Dedup key: (date, first line of entry text after the checkbox).
fn entry_dedup_key(entry: &str) -> String {
    let first_line = entry.lines().next().unwrap_or("");
    // Extract the date and text after `- [x] `
    if first_line.len() > 6 {
        first_line[6..].to_string() // skip "- [x] " prefix
    } else {
        first_line.to_string()
    }
}

/// Merge log sections: union entries from both sides, dedup, sort chronologically.
fn merge_log_section(ancestor: &str, ours: &str, theirs: &str) -> String {
    let ancestor_entries = parse_log_entries(ancestor);
    let ours_entries = parse_log_entries(ours);
    let theirs_entries = parse_log_entries(theirs);

    let mut seen = std::collections::BTreeSet::new();
    let mut merged = Vec::new();

    // Add all unique entries (by dedup key)
    for entry in ours_entries
        .iter()
        .chain(theirs_entries.iter())
        .chain(ancestor_entries.iter())
    {
        let key = entry_dedup_key(entry);
        if seen.insert(key) {
            merged.push(entry.clone());
        }
    }

    // Sort chronologically by the date in the entry
    merged.sort_by(|a, b| {
        let date_a = extract_entry_date(a).unwrap_or_default();
        let date_b = extract_entry_date(b).unwrap_or_default();
        date_a.cmp(&date_b)
    });

    merged.join("\n")
}

fn extract_entry_date(entry: &str) -> Option<String> {
    let first_line = entry.lines().next()?;
    // Pattern: `- [x] YYYY-MM-DD`
    if first_line.len() >= 16 {
        Some(first_line[6..16].to_string())
    } else {
        None
    }
}

/// Merge body using append-log strategy.
fn merge_body_append_log(
    ancestor: &str,
    ours: &str,
    theirs: &str,
    _ours_hlc: &Option<Hlc>,
    _theirs_hlc: &Option<Hlc>,
) -> Result<String> {
    let ancestor_sections = split_body_sections(ancestor);
    let ours_sections = split_body_sections(ours);
    let theirs_sections = split_body_sections(theirs);

    // Build section maps keyed by heading
    let ours_map: std::collections::BTreeMap<Option<String>, String> =
        ours_sections.into_iter().collect();
    let theirs_map: std::collections::BTreeMap<Option<String>, String> =
        theirs_sections.into_iter().collect();
    let ancestor_map: std::collections::BTreeMap<Option<String>, String> =
        ancestor_sections.into_iter().collect();

    // Collect all headings in order (ours first, then new from theirs)
    let mut headings: Vec<Option<String>> = Vec::new();
    for key in ours_map.keys() {
        headings.push(key.clone());
    }
    for key in theirs_map.keys() {
        if !headings.contains(key) {
            headings.push(key.clone());
        }
    }

    let mut output = String::new();
    for heading in headings {
        if let Some(h) = &heading {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&format!("## {h}\n"));
        }

        let a = ancestor_map.get(&heading).map(|s| s.as_str()).unwrap_or("");
        let o = ours_map.get(&heading).map(|s| s.as_str()).unwrap_or("");
        let t = theirs_map.get(&heading).map(|s| s.as_str()).unwrap_or("");

        let merged_section = if is_log_section(o) || is_log_section(t) {
            merge_log_section(a, o, t)
        } else {
            merge_body(a, o, t)?
        };

        output.push_str(&merged_section);
        if !merged_section.ends_with('\n') {
            output.push('\n');
        }
    }

    // Trim trailing whitespace
    Ok(output.trim_end().to_string())
}
