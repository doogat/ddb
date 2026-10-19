use regex::Regex;

use crate::error::Result;
use crate::types::DoogatId;
#[cfg(test)]
use crate::types::{DoogatMeta, Value};

mod links;
mod yaml_bridge;
mod zones;

pub use links::{
    extract_bare_urls, extract_embeds, extract_inline_fields, extract_links, extract_markdown_links,
    extract_wikilinks, rewrite_links, rewrite_wikilinks,
};
pub(crate) use yaml_bridge::{from_serde_yaml, yaml_quote};
use yaml_bridge::{fence_regex, inline_code_regex, serialize_yaml_value, to_serde_yaml};
pub(crate) use zones::has_frontmatter;
pub use zones::{parse_frontmatter, split_zones};
#[cfg(test)]
use zones::{strip_wikilink, MAX_FRONTMATTER_BYTES};

/// Serialize a ParsedDoogat back to Markdown string.
/// Frontmatter field order: id, title, date, tags, type, publish, processed, then extras.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn serialize(doogat: &crate::types::ParsedDoogat) -> String {
    let mut out = String::from("---\n");

    // Fixed-order core fields: id, title, date, tags, type, publish, processed
    if let Some(ref id) = doogat.meta.id {
        out.push_str(&format!("id: {}\n", id.0));
    }
    if let Some(ref title) = doogat.meta.title {
        out.push_str(&format!("title: {}\n", yaml_quote(title)));
    }
    if let Some(ref date) = doogat.meta.date {
        out.push_str(&format!("date: {date}\n"));
    }
    if !doogat.meta.tags.is_empty() {
        out.push_str("tags:\n");
        for tag in &doogat.meta.tags {
            out.push_str(&format!("  - {}\n", yaml_quote(tag)));
        }
    }
    if let Some(ref t) = doogat.meta.doogat_type {
        out.push_str(&format!("type: {}\n", yaml_quote(t)));
    }

    // Extract publish/processed from extras in canonical position
    let promoted = ["publish", "processed"];
    for key in &promoted {
        if let Some(value) = doogat.meta.extra.get(*key) {
            let sv = to_serde_yaml(value);
            let yaml_val = serde_yaml::to_string(&sv).unwrap_or_default();
            let yaml_val = yaml_val.trim().trim_end_matches('\n');
            out.push_str(&format!("{key}: {yaml_val}\n"));
        }
    }

    // Remaining extras alphabetically (BTreeMap is sorted), skip promoted keys
    for (key, value) in &doogat.meta.extra {
        if promoted.contains(&key.as_str()) {
            continue;
        }
        let sv = to_serde_yaml(value);
        serialize_yaml_value(&mut out, key, &sv);
    }

    out.push_str("---\n");

    // Body verbatim
    out.push_str(&doogat.body);

    // Reference section
    if !doogat.reference_section.is_empty() {
        out.push_str("\n---\n");
        out.push_str(&doogat.reference_section);
    }

    out
}

/// Update the `id` field in a doogat's frontmatter. Pure content transform —
/// no `GitBackend` dependency — used by collision-loser reassignment.
pub(crate) fn rewrite_id_field(content: &str, new_id: &str) -> Result<String> {
    let mut parsed = parse(content, "collision-loser")?;
    parsed.meta.id = Some(crate::types::DoogatId(new_id.to_string()));
    Ok(serialize(&parsed))
}

/// Extract checkbox items from body text.
///
/// Parses `- [ ]`, `- [x]`, `- [i]` items with optional date prefix and due date.
/// Skips fenced code blocks.
pub fn extract_checkboxes(body: &str) -> Vec<crate::types::CheckboxItem> {
    use crate::types::{CheckboxItem, CheckboxState};
    use std::sync::OnceLock;
    static CB_RE: OnceLock<Regex> = OnceLock::new();
    static DATE_RE: OnceLock<Regex> = OnceLock::new();
    static DUE_RE: OnceLock<Regex> = OnceLock::new();

    let fence_re = fence_regex();
    let cb_re = CB_RE
        .get_or_init(|| Regex::new(r"^(\s*)- \[([ xi])\]\s+(.+)$").expect("valid regex: checkbox"));
    let date_re = DATE_RE.get_or_init(|| {
        Regex::new(r"^(\d{4}-\d{2}-\d{2}\s+\d{2}:\d{2})\s*[-–]\s*(.+)$")
            .expect("valid regex: date prefix")
    });
    let due_re = DUE_RE
        .get_or_init(|| Regex::new(r"⏳\s*(\d{4}-\d{2}-\d{2})").expect("valid regex: due date"));

    let mut items = Vec::new();
    let mut in_fence = false;

    for (line_idx, line) in body.lines().enumerate() {
        if fence_re.is_match(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(caps) = cb_re.captures(line) {
            let indent = caps[1].len();
            let state = match &caps[2] {
                " " => CheckboxState::Open,
                "x" => CheckboxState::Done,
                "i" => CheckboxState::Info,
                _ => continue,
            };
            let raw_content = caps[3].to_string();

            // Parse optional date prefix
            let (date, content) = if let Some(dc) = date_re.captures(&raw_content) {
                (Some(dc[1].to_string()), dc[2].to_string())
            } else {
                (None, raw_content)
            };

            // Parse and strip optional due date
            let (due_date, content) = if let Some(dc) = due_re.captures(&content) {
                let stripped = due_re.replace(&content, "").trim().to_string();
                (Some(dc[1].to_string()), stripped)
            } else {
                (None, content)
            };

            items.push(CheckboxItem {
                state,
                content,
                date,
                due_date,
                line_number: line_idx + 1,
                indent_level: indent,
            });
        }
    }

    items
}

/// Parse body text into sections at ATX headings, respecting fenced code blocks.
///
/// Returns sections in document order. Pre-heading content is level 0 with empty heading.
pub fn extract_sections(body: &str) -> Vec<crate::types::Section> {
    use std::sync::OnceLock;
    static HEADING_RE: OnceLock<Regex> = OnceLock::new();

    let re = HEADING_RE.get_or_init(|| {
        Regex::new(r"^(#{1,6})\s+(.+?)(?:\s+#+)?$").expect("valid regex: atx heading")
    });
    let fence_re = fence_regex();

    let mut sections = Vec::new();
    let mut current_heading = String::new();
    let mut current_level: u8 = 0;
    let mut current_lines: Vec<&str> = Vec::new();
    let mut in_fence = false;

    for line in body.lines() {
        if fence_re.is_match(line) {
            in_fence = !in_fence;
            current_lines.push(line);
            continue;
        }
        if in_fence {
            current_lines.push(line);
            continue;
        }

        if let Some(caps) = re.captures(line) {
            // Push previous section
            if current_level > 0 || !current_lines.is_empty() {
                sections.push(crate::types::Section {
                    heading: current_heading.clone(),
                    level: current_level,
                    content: current_lines.join("\n"),
                });
            }
            // Start new section
            current_level = caps[1].len() as u8;
            current_heading = caps[2].to_string();
            current_lines.clear();
        } else {
            current_lines.push(line);
        }
    }

    // Push final section
    if current_level > 0 || !current_lines.is_empty() {
        sections.push(crate::types::Section {
            heading: current_heading,
            level: current_level,
            content: current_lines.join("\n"),
        });
    }

    sections
}

/// Extract hashtags from body text, respecting exclusion zones.
///
/// Skips fenced code blocks, inline code spans, and wikilinks.
/// Returns unique tags (without `#` prefix) in first-encountered order.
pub fn extract_hashtags(body: &str) -> Vec<String> {
    use std::sync::OnceLock;
    static WIKILINK_RE: OnceLock<Regex> = OnceLock::new();
    static HASHTAG_RE: OnceLock<Regex> = OnceLock::new();

    let fence_re = fence_regex();
    let inline_code_re = inline_code_regex();
    let wikilink_re =
        WIKILINK_RE.get_or_init(|| Regex::new(r"\[\[[^\]]*\]\]").expect("valid regex: wikilink"));
    let hashtag_re = HASHTAG_RE
        .get_or_init(|| Regex::new(r"(?:^|\s)#([\w][\w/-]*)").expect("valid regex: hashtag"));

    let mut tags = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut in_fence = false;

    for line in body.lines() {
        if fence_re.is_match(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        // Strip inline code and wikilinks before matching hashtags
        let stripped = inline_code_re.replace_all(line, "");
        let stripped = wikilink_re.replace_all(&stripped, "");

        for caps in hashtag_re.captures_iter(&stripped) {
            let tag = caps[1].to_string();
            // Check that the # is not preceded by :// (URL fragment)
            if let Some(m) = caps.get(0) {
                let before = &stripped[..m.start()];
                if before.ends_with("://") {
                    continue;
                }
            }
            if seen.insert(tag.clone()) {
                tags.push(tag);
            }
        }
    }

    tags
}

/// Parse a doogat Markdown file into a fully structured ParsedDoogat.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn parse(content: &str, path: &str) -> Result<crate::types::ParsedDoogat> {
    let doogat = split_zones(content)?;
    let meta = parse_frontmatter(&doogat.raw_frontmatter, path)?;
    let inline_fields = extract_inline_fields(&doogat.body, &doogat.reference_section)?;
    let wikilinks = extract_links(
        &doogat.raw_frontmatter,
        &doogat.body,
        &doogat.reference_section,
    );
    let sections = extract_sections(&doogat.body);
    let body_tags = extract_hashtags(&doogat.body);
    let checkboxes = extract_checkboxes(&doogat.body);

    Ok(crate::types::ParsedDoogat {
        meta,
        body: doogat.body,
        sections,
        reference_section: doogat.reference_section,
        inline_fields,
        links: wikilinks,
        body_tags,
        checkboxes,
        path: path.to_string(),
        updated_at: None,
    })
}

/// Generate a doogat ID from the current local timestamp (YYYYMMDDHHmmss).
/// Generate a 14-digit timestamp ID (YYYYMMDDHHmmss).
///
/// Within a single process, consecutive calls in the same second will
/// spin-wait until the clock advances, preventing collisions.
pub fn generate_id() -> DoogatId {
    generate_unique_id(|_| false)
}

/// Generate a unique 14-digit timestamp ID, spin-waiting if `exists`
/// returns true for the candidate. Also deduplicates within-process.
pub fn generate_unique_id(exists: impl Fn(&str) -> bool) -> DoogatId {
    use std::sync::Mutex;
    static LAST: Mutex<String> = Mutex::new(String::new());

    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let now = chrono::Local::now();
        let candidate = now.format("%Y%m%d%H%M%S").to_string();
        if candidate != *last && !exists(&candidate) {
            *last = candidate.clone();
            return DoogatId(candidate);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Extract doogat ID from a file path like `ddb/20240101120000.md`
/// or `ddb/_typedef/20240101120000.md`.
pub fn extract_id_from_path(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests;
