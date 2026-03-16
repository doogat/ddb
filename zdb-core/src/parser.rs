use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::error::{Result, ZettelError};
use crate::types::{InlineField, Link, Value, Zettel, ZettelId, ZettelMeta, Zone};

/// Shared regex for fenced code block markers (``` or ~~~).
fn fence_regex() -> &'static Regex {
    static FENCE_RE: OnceLock<Regex> = OnceLock::new();
    FENCE_RE.get_or_init(|| Regex::new(r"^(?:`{3,}|~{3,})").expect("valid regex: fence marker"))
}

impl From<serde_yaml::Error> for ZettelError {
    fn from(e: serde_yaml::Error) -> Self {
        Self::Yaml(e.to_string())
    }
}

/// Internal struct for YAML deserialization. Converts to public ZettelMeta at the boundary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RawZettelMeta {
    pub id: Option<ZettelId>,
    pub title: Option<String>,
    pub date: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub zettel_type: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

impl From<RawZettelMeta> for ZettelMeta {
    fn from(raw: RawZettelMeta) -> Self {
        ZettelMeta {
            id: raw.id,
            title: raw.title,
            date: raw.date,
            zettel_type: raw.zettel_type,
            tags: raw.tags,
            extra: raw
                .extra
                .into_iter()
                .map(|(k, v)| (k, from_serde_yaml(v)))
                .collect(),
        }
    }
}

fn from_serde_yaml(v: serde_yaml::Value) -> Value {
    match v {
        serde_yaml::Value::String(s) => Value::String(s),
        serde_yaml::Value::Number(n) => Value::Number(n.as_f64().unwrap_or(0.0)),
        serde_yaml::Value::Bool(b) => Value::Bool(b),
        serde_yaml::Value::Sequence(seq) => {
            Value::List(seq.into_iter().map(from_serde_yaml).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let m = map
                .into_iter()
                .filter_map(|(k, v)| k.as_str().map(|ks| (ks.to_string(), from_serde_yaml(v))))
                .collect();
            Value::Map(m)
        }
        serde_yaml::Value::Null | serde_yaml::Value::Tagged(_) => Value::String(String::new()),
    }
}

fn to_serde_yaml(v: &Value) -> serde_yaml::Value {
    match v {
        Value::String(s) => serde_yaml::Value::String(s.clone()),
        Value::Number(n) => serde_yaml::Value::Number(serde_yaml::Number::from(*n)),
        Value::Bool(b) => serde_yaml::Value::Bool(*b),
        Value::List(list) => serde_yaml::Value::Sequence(list.iter().map(to_serde_yaml).collect()),
        Value::Map(map) => {
            let m: serde_yaml::Mapping = map
                .iter()
                .map(|(k, v)| (serde_yaml::Value::String(k.clone()), to_serde_yaml(v)))
                .collect();
            serde_yaml::Value::Mapping(m)
        }
    }
}

/// Reference line pattern: `- key:: value` or `- key::` (empty value)
fn is_reference_line(line: &str) -> bool {
    lazy_static_regex().is_match(line)
}

fn lazy_static_regex() -> &'static Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^- [\w][\w\s-]*:: ?.*$").expect("valid regex: ref-line pattern"))
}

/// Split markdown content into three zones: frontmatter, body, reference section.
///
/// Heuristic for reference section: find last `---` on its own line (after frontmatter);
/// if ALL non-empty lines after it match `- key:: value` pattern, that's the boundary.
/// Backtracks if content after last `---` is empty/whitespace.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn split_zones(content: &str) -> Result<Zettel> {
    let lines: Vec<&str> = content.lines().collect();

    // Find frontmatter boundaries (first `---` pair), tracking fenced code blocks
    let (fm_start, fm_end) = find_frontmatter(&lines)?;

    let frontmatter = lines[fm_start + 1..fm_end].join("\n");

    // Collect all `---` positions after frontmatter, skipping those inside fenced code blocks
    let separator_positions = find_separators_after(&lines, fm_end);

    // Try separators from last to first, looking for valid reference boundary.
    // When backtracking, check content between this separator and the next one (or EOF).
    let mut ref_boundary = None;
    let mut end_boundary = lines.len(); // exclusive upper bound for reference content
    for &pos in separator_positions.iter().rev() {
        let after = &lines[pos + 1..end_boundary];
        if after.iter().all(|l| l.trim().is_empty()) {
            // Empty/whitespace only → skip this separator and narrow the window
            end_boundary = pos;
            continue;
        }
        if after
            .iter()
            .filter(|l| !l.trim().is_empty())
            .all(|l| is_reference_line(l))
        {
            ref_boundary = Some(pos);
            break;
        }
        // Content doesn't match reference pattern → stop searching
        break;
    }

    let (body, reference_section) = match ref_boundary {
        Some(pos) => {
            let body = lines[fm_end + 1..pos].join("\n");
            let reference = lines[pos + 1..end_boundary].join("\n");
            (body, reference)
        }
        None => {
            let body = lines[fm_end + 1..].join("\n");
            (body, String::new())
        }
    };

    Ok(Zettel {
        raw_frontmatter: frontmatter,
        body,
        reference_section,
    })
}

/// Find the opening and closing `---` lines for frontmatter.
fn find_frontmatter(lines: &[&str]) -> Result<(usize, usize)> {
    let first = lines
        .iter()
        .position(|l| l.trim() == "---")
        .ok_or_else(|| ZettelError::Parse("no frontmatter opening ---".into()))?;

    let second = lines[first + 1..]
        .iter()
        .position(|l| l.trim() == "---")
        .map(|i| i + first + 1)
        .ok_or_else(|| ZettelError::Parse("no frontmatter closing ---".into()))?;

    Ok((first, second))
}

/// Find all `---` separator positions after frontmatter, skipping fenced code blocks.
fn find_separators_after(lines: &[&str], fm_end: usize) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut in_fence = false;

    for (i, line) in lines.iter().enumerate().skip(fm_end + 1) {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence && trimmed == "---" {
            positions.push(i);
        }
    }

    positions
}

/// Parse YAML frontmatter string into ZettelMeta.
/// Falls back to filename-based ID when `id` field is missing.
pub fn parse_frontmatter(yaml: &str, path: &str) -> Result<ZettelMeta> {
    let raw: RawZettelMeta = if yaml.trim().is_empty() {
        RawZettelMeta::default()
    } else {
        serde_yaml::from_str(yaml)?
    };
    let mut meta: ZettelMeta = raw.into();

    // Fallback: derive ID from filename stem if not in frontmatter
    if meta.id.is_none() {
        if let Some(stem) = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
        {
            if stem.chars().all(|c| c.is_ascii_digit()) && !stem.is_empty() {
                meta.id = Some(ZettelId(stem.to_owned()));
            }
        }
    }

    Ok(meta)
}

/// Extract Dataview-style inline fields from body and reference zones.
/// Body fields: `key:: value` on a line. Reference fields: `- key:: value` (list-item).
/// Cross-zone duplicate keys → validation error. Same-zone duplicates: first wins silently.
pub fn extract_inline_fields(
    body: &str,
    reference: &str,
) -> crate::error::Result<Vec<InlineField>> {
    use std::sync::OnceLock;
    static BODY_RE: OnceLock<Regex> = OnceLock::new();
    static REF_RE: OnceLock<Regex> = OnceLock::new();

    static INLINE_CODE_RE: OnceLock<Regex> = OnceLock::new();
    let body_re = BODY_RE.get_or_init(|| {
        Regex::new(r"^([\w][\w\s-]*):: (.+)$").expect("valid regex: body inline field")
    });
    let ref_re = REF_RE.get_or_init(|| {
        Regex::new(r"^- ([\w][\w\s-]*):: ?(.*)$").expect("valid regex: ref inline field")
    });
    let inline_code_re =
        INLINE_CODE_RE.get_or_init(|| Regex::new(r"`[^`]+`").expect("valid regex: inline code"));
    let fence_re = fence_regex();

    let mut fields = Vec::new();
    let mut seen: std::collections::HashMap<String, Zone> = std::collections::HashMap::new();
    let mut in_fence = false;

    for line in body.lines() {
        if fence_re.is_match(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let stripped = inline_code_re.replace_all(line, "");
        if let Some(caps) = body_re.captures(&stripped) {
            let key = caps[1].trim().to_string();
            match seen.get(&key) {
                Some(Zone::Body) => {} // same-zone dup, first wins
                Some(_) => {
                    return Err(crate::error::ZettelError::Validation(format!(
                        "duplicate inline field '{key}' across body and reference zones"
                    )));
                }
                None => {
                    seen.insert(key.clone(), Zone::Body);
                    fields.push(InlineField {
                        key,
                        value: caps[2].to_string(),
                        zone: Zone::Body,
                    });
                }
            }
        }
    }

    for line in reference.lines() {
        if let Some(caps) = ref_re.captures(line) {
            let key = caps[1].trim().to_string();
            match seen.get(&key) {
                Some(Zone::Reference) => {} // same-zone dup, first wins
                Some(_) => {
                    return Err(crate::error::ZettelError::Validation(format!(
                        "duplicate inline field '{key}' across body and reference zones"
                    )));
                }
                None => {
                    seen.insert(key.clone(), Zone::Reference);
                    fields.push(InlineField {
                        key,
                        value: caps[2].to_string(),
                        zone: Zone::Reference,
                    });
                }
            }
        }
    }

    Ok(fields)
}

/// Extract `![[target#section|display]]` embed links from text, skipping code blocks.
pub fn extract_embeds(text: &str, zone: Zone) -> Vec<Link> {
    use std::sync::OnceLock;
    static EMBED_RE: OnceLock<Regex> = OnceLock::new();
    static INLINE_CODE_RE: OnceLock<Regex> = OnceLock::new();

    let re = EMBED_RE.get_or_init(|| {
        Regex::new(r"!\[\[([^\]#|]+)(?:#([^\]|]+))?(?:\|([^\]]+))?\]\]")
            .expect("valid regex: embed")
    });
    let fence_re = fence_regex();
    let inline_code_re =
        INLINE_CODE_RE.get_or_init(|| Regex::new(r"`[^`]+`").expect("valid regex: inline code"));

    let mut links = Vec::new();
    let mut in_fence = false;

    for line in text.lines() {
        if fence_re.is_match(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let stripped = inline_code_re.replace_all(line, "");
        for caps in re.captures_iter(&stripped) {
            links.push(Link {
                target: caps[1].to_string(),
                section: caps.get(2).map(|m| m.as_str().to_string()),
                display: caps.get(3).map(|m| m.as_str().to_string()),
                kind: crate::types::LinkKind::Embed,
                zone: zone.clone(),
            });
        }
    }

    links
}

/// Extract `[[target|display]]` wikilinks from all three zones.
pub fn extract_wikilinks(frontmatter: &str, body: &str, reference: &str) -> Vec<Link> {
    use std::sync::OnceLock;
    static WL_RE: OnceLock<Regex> = OnceLock::new();
    let re = WL_RE.get_or_init(|| {
        Regex::new(r"\[\[([^\]|]+)(?:\|([^\]]+))?\]\]").expect("valid regex: wikilink")
    });

    let mut links = Vec::new();

    for (text, zone) in [
        (frontmatter, Zone::Frontmatter),
        (body, Zone::Body),
        (reference, Zone::Reference),
    ] {
        for caps in re.captures_iter(text) {
            links.push(Link {
                target: caps[1].to_string(),
                display: caps.get(2).map(|m| m.as_str().to_string()),
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: zone.clone(),
            });
        }
    }

    links
}

/// Replace wikilink targets in raw file content.
///
/// Rewrites `[[old_target]]` → `[[new_target]]` and
/// `[[old_target|display]]` → `[[new_target|display]]` across all zones.
pub fn rewrite_wikilinks(content: &str, old_target: &str, new_target: &str) -> String {
    use std::sync::OnceLock;
    static REWRITE_RE: OnceLock<Regex> = OnceLock::new();
    // Capture: [[target]] or [[target|display]]
    let re = REWRITE_RE.get_or_init(|| {
        Regex::new(r"\[\[([^\]|]+)(?:\|([^\]]+))?\]\]").expect("valid regex: wikilink rewrite")
    });

    re.replace_all(content, |caps: &regex::Captures| {
        let target = &caps[1];
        if target == old_target {
            match caps.get(2) {
                Some(display) => format!("[[{}|{}]]", new_target, display.as_str()),
                None => format!("[[{}]]", new_target),
            }
        } else {
            caps[0].to_string()
        }
    })
    .into_owned()
}

/// Quote a YAML string value if it contains special characters.
fn yaml_quote(s: &str) -> String {
    if s.contains(':')
        || s.contains('[')
        || s.contains(']')
        || s.contains('{')
        || s.contains('}')
        || s.contains('#')
        || s.contains("[[")
    {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Serialize a YAML value as a frontmatter field, handling complex types (sequences, mappings)
/// with proper block-style indentation.
fn serialize_yaml_value(out: &mut String, key: &str, value: &serde_yaml::Value) {
    match value {
        serde_yaml::Value::Sequence(_) | serde_yaml::Value::Mapping(_) => {
            // Use serde_yaml to serialize the full key-value pair as a YAML mapping,
            // then strip the trailing newline and append.
            let mut map = serde_yaml::Mapping::new();
            map.insert(serde_yaml::Value::String(key.into()), value.clone());
            let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(map)).unwrap_or_default();
            out.push_str(&yaml);
        }
        _ => {
            let yaml_val = serde_yaml::to_string(value).unwrap_or_default();
            let yaml_val = yaml_val.trim().trim_end_matches('\n');
            out.push_str(&format!("{key}: {}\n", yaml_quote(yaml_val)));
        }
    }
}

/// Serialize a ParsedZettel back to Markdown string.
/// Frontmatter field order: id, title, date, tags, type, publish, processed, then extras.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn serialize(zettel: &crate::types::ParsedZettel) -> String {
    let mut out = String::from("---\n");

    // Fixed-order core fields: id, title, date, tags, type, publish, processed
    if let Some(ref id) = zettel.meta.id {
        out.push_str(&format!("id: {}\n", id.0));
    }
    if let Some(ref title) = zettel.meta.title {
        out.push_str(&format!("title: {}\n", yaml_quote(title)));
    }
    if let Some(ref date) = zettel.meta.date {
        out.push_str(&format!("date: {date}\n"));
    }
    if !zettel.meta.tags.is_empty() {
        out.push_str("tags:\n");
        for tag in &zettel.meta.tags {
            out.push_str(&format!("  - {}\n", yaml_quote(tag)));
        }
    }
    if let Some(ref t) = zettel.meta.zettel_type {
        out.push_str(&format!("type: {}\n", yaml_quote(t)));
    }

    // Extract publish/processed from extras in canonical position
    let promoted = ["publish", "processed"];
    for key in &promoted {
        if let Some(value) = zettel.meta.extra.get(*key) {
            let sv = to_serde_yaml(value);
            let yaml_val = serde_yaml::to_string(&sv).unwrap_or_default();
            let yaml_val = yaml_val.trim().trim_end_matches('\n');
            out.push_str(&format!("{key}: {}\n", yaml_quote(yaml_val)));
        }
    }

    // Remaining extras alphabetically (BTreeMap is sorted), skip promoted keys
    for (key, value) in &zettel.meta.extra {
        if promoted.contains(&key.as_str()) {
            continue;
        }
        let sv = to_serde_yaml(value);
        serialize_yaml_value(&mut out, key, &sv);
    }

    out.push_str("---\n");

    // Body verbatim
    out.push_str(&zettel.body);

    // Reference section
    if !zettel.reference_section.is_empty() {
        out.push_str("\n---\n");
        out.push_str(&zettel.reference_section);
    }

    out
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

/// Extract hashtags from body text, respecting exclusion zones.
///
/// Skips fenced code blocks, inline code spans, and wikilinks.
/// Returns unique tags (without `#` prefix) in first-encountered order.
pub fn extract_hashtags(body: &str) -> Vec<String> {
    use std::sync::OnceLock;
    static INLINE_CODE_RE: OnceLock<Regex> = OnceLock::new();
    static WIKILINK_RE: OnceLock<Regex> = OnceLock::new();
    static HASHTAG_RE: OnceLock<Regex> = OnceLock::new();

    let fence_re = fence_regex();
    let inline_code_re =
        INLINE_CODE_RE.get_or_init(|| Regex::new(r"`[^`]+`").expect("valid regex: inline code"));
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

/// Parse a zettel Markdown file into a fully structured ParsedZettel.
#[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
pub fn parse(content: &str, path: &str) -> Result<crate::types::ParsedZettel> {
    let zettel = split_zones(content)?;
    let meta = parse_frontmatter(&zettel.raw_frontmatter, path)?;
    let inline_fields = extract_inline_fields(&zettel.body, &zettel.reference_section)?;
    let wikilinks = extract_wikilinks(
        &zettel.raw_frontmatter,
        &zettel.body,
        &zettel.reference_section,
    );
    let body_tags = extract_hashtags(&zettel.body);
    let checkboxes = extract_checkboxes(&zettel.body);

    Ok(crate::types::ParsedZettel {
        meta,
        body: zettel.body,
        reference_section: zettel.reference_section,
        inline_fields,
        links: wikilinks,
        body_tags,
        checkboxes,
        path: path.to_string(),
    })
}

/// Generate a zettel ID from the current local timestamp (YYYYMMDDHHmmss).
/// Generate a 14-digit timestamp ID (YYYYMMDDHHmmss).
///
/// Within a single process, consecutive calls in the same second will
/// spin-wait until the clock advances, preventing collisions.
pub fn generate_id() -> ZettelId {
    generate_unique_id(|_| false)
}

/// Generate a unique 14-digit timestamp ID, spin-waiting if `exists`
/// returns true for the candidate. Also deduplicates within-process.
pub fn generate_unique_id(exists: impl Fn(&str) -> bool) -> ZettelId {
    use std::sync::Mutex;
    static LAST: Mutex<String> = Mutex::new(String::new());

    let mut last = LAST.lock().unwrap();
    loop {
        let now = chrono::Local::now();
        let candidate = now.format("%Y%m%d%H%M%S").to_string();
        if candidate != *last && !exists(&candidate) {
            *last = candidate.clone();
            return ZettelId(candidate);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// Extract zettel ID from a file path like `zettelkasten/20240101120000.md`
/// or `zettelkasten/_typedef/20240101120000.md`.
pub fn extract_id_from_path(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_three_zone_split() {
        let content = "\
---
title: Test
---
Body content here.

Some more body.
---
- source:: Wikipedia
- tags:: test";

        let z = split_zones(content).unwrap();
        assert_eq!(z.raw_frontmatter, "title: Test");
        assert!(z.body.contains("Body content here."));
        assert!(z.body.contains("Some more body."));
        assert!(z.reference_section.contains("- source:: Wikipedia"));
        assert!(z.reference_section.contains("- tags:: test"));
    }

    #[test]
    fn no_reference_section() {
        let content = "\
---
title: Test
---
Just body content.";

        let z = split_zones(content).unwrap();
        assert_eq!(z.raw_frontmatter, "title: Test");
        assert_eq!(z.body, "Just body content.");
        assert!(z.reference_section.is_empty());
    }

    #[test]
    fn code_block_with_separator() {
        let content = "\
---
title: Test
---
Before code.

```
---
this is not a separator
---
```

After code.";

        let z = split_zones(content).unwrap();
        assert!(z.body.contains("---"));
        assert!(z.body.contains("this is not a separator"));
        assert!(z.reference_section.is_empty());
    }

    #[test]
    fn thematic_break_not_reference_boundary() {
        let content = "\
---
title: Test
---
Paragraph one.

---

Paragraph two with no reference fields.";

        let z = split_zones(content).unwrap();
        // The `---` in the body is a thematic break, not a reference boundary
        // because the content after it doesn't match `- key:: value`
        assert!(z.body.contains("Paragraph one."));
        assert!(z.body.contains("Paragraph two"));
        assert!(z.reference_section.is_empty());
    }

    #[test]
    fn trailing_separator_after_reference_backtracks() {
        let content = "\
---
title: Test
---
Body here.
---
- source:: Wikipedia
---
";

        let z = split_zones(content).unwrap();
        // Last `---` has only whitespace/empty after it → backtrack to previous `---`
        assert_eq!(z.body, "Body here.");
        assert!(z.reference_section.contains("- source:: Wikipedia"));
    }

    #[test]
    fn empty_after_last_separator_backtracks() {
        let content = "\
---
title: Test
---
Body here.
---
";

        let z = split_zones(content).unwrap();
        // Last `---` has nothing after it → backtrack, no valid reference boundary
        assert!(z.body.contains("Body here."));
        assert!(z.reference_section.is_empty());
    }

    // -- frontmatter parsing tests --

    use crate::types::Zone;

    #[test]
    fn frontmatter_all_fields() {
        let yaml = "id: 20260226120000\ntitle: My Note\ndate: 2026-02-26\ntype: permanent\ntags:\n  - test\n  - demo";
        let meta = parse_frontmatter(yaml, "20260226120000.md").unwrap();
        assert_eq!(meta.id, Some(ZettelId("20260226120000".into())));
        assert_eq!(meta.title.as_deref(), Some("My Note"));
        assert_eq!(meta.date.as_deref(), Some("2026-02-26"));
        assert_eq!(meta.zettel_type.as_deref(), Some("permanent"));
        assert_eq!(meta.tags, vec!["test", "demo"]);
    }

    #[test]
    fn frontmatter_empty() {
        let meta = parse_frontmatter("", "20260226120000.md").unwrap();
        assert_eq!(meta.id, Some(ZettelId("20260226120000".into())));
        assert!(meta.title.is_none());
        assert!(meta.tags.is_empty());
    }

    #[test]
    fn frontmatter_extra_fields_preserved() {
        let yaml = "title: Test\ncustom_field: hello\nanother: 42";
        let meta = parse_frontmatter(yaml, "note.md").unwrap();
        assert_eq!(meta.title.as_deref(), Some("Test"));
        assert!(meta.extra.contains_key("custom_field"));
        assert!(meta.extra.contains_key("another"));
    }

    #[test]
    fn frontmatter_id_fallback_from_filename() {
        let yaml = "title: No ID here";
        let meta = parse_frontmatter(yaml, "zettelkasten/20260226130000.md").unwrap();
        assert_eq!(meta.id, Some(ZettelId("20260226130000".into())));
    }

    // -- inline field extraction tests --

    #[test]
    fn inline_fields_body_only() {
        let fields = extract_inline_fields("source:: Wikipedia\nstatus:: draft", "").unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].key, "source");
        assert_eq!(fields[0].value, "Wikipedia");
        assert_eq!(fields[0].zone, Zone::Body);
    }

    #[test]
    fn inline_fields_reference_only() {
        let fields = extract_inline_fields("", "- source:: Wikipedia\n- tags:: test").unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].zone, Zone::Reference);
        assert_eq!(fields[1].key, "tags");
    }

    #[test]
    fn inline_fields_mixed() {
        let fields = extract_inline_fields("status:: draft", "- source:: Wikipedia").unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].zone, Zone::Body);
        assert_eq!(fields[1].zone, Zone::Reference);
    }

    #[test]
    fn inline_fields_empty_reference_value() {
        let fields = extract_inline_fields("", "- emptykey::").unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].key, "emptykey");
        assert_eq!(fields[0].value, "");
    }

    #[test]
    fn inline_fields_cross_zone_duplicate_errors() {
        let result = extract_inline_fields("source:: Body Version", "- source:: Ref Version");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("duplicate inline field 'source'"));
    }

    #[test]
    fn inline_fields_same_zone_duplicate_first_wins() {
        let fields = extract_inline_fields("source:: First\nsource:: Second", "").unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].value, "First");
    }

    #[test]
    fn inline_fields_skip_fenced_code_block() {
        let body = "status:: draft\n```\nsource:: Wikipedia\n```\nvisible:: yes";
        let fields = extract_inline_fields(body, "").unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].key, "status");
        assert_eq!(fields[1].key, "visible");
    }

    #[test]
    fn inline_fields_skip_tilde_fenced_code_block() {
        let body = "status:: draft\n~~~\nsource:: Wikipedia\n~~~\nvisible:: yes";
        let fields = extract_inline_fields(body, "").unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].key, "status");
        assert_eq!(fields[1].key, "visible");
    }

    #[test]
    fn inline_fields_skip_inline_code() {
        let body = "some `key:: value` text\nreal:: field";
        let fields = extract_inline_fields(body, "").unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].key, "real");
    }

    #[test]
    fn inline_fields_normal_next_to_inline_code() {
        let body = "status:: draft with `some code` here";
        let fields = extract_inline_fields(body, "").unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].key, "status");
    }

    // -- wikilink extraction tests --

    #[test]
    fn extract_embed_basic() {
        let links = extract_embeds("![[myfile]]", Zone::Body);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "myfile");
        assert!(links[0].section.is_none());
        assert!(links[0].display.is_none());
        assert_eq!(links[0].kind, crate::types::LinkKind::Embed);
    }

    #[test]
    fn extract_embed_with_section() {
        let links = extract_embeds("![[file#heading]]", Zone::Body);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "file");
        assert_eq!(links[0].section.as_deref(), Some("heading"));
        assert!(links[0].display.is_none());
    }

    #[test]
    fn extract_embed_with_display() {
        let links = extract_embeds("![[file|alt text]]", Zone::Body);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "file");
        assert_eq!(links[0].display.as_deref(), Some("alt text"));
    }

    #[test]
    fn extract_embed_full() {
        let links = extract_embeds("![[file#section|display]]", Zone::Body);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "file");
        assert_eq!(links[0].section.as_deref(), Some("section"));
        assert_eq!(links[0].display.as_deref(), Some("display"));
    }

    #[test]
    fn embed_in_code_block_skipped() {
        let text = "before\n```\n![[inside]]\n```\n![[outside]]";
        let links = extract_embeds(text, Zone::Body);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target, "outside");
    }

    #[test]
    fn wikilinks_body() {
        let links = extract_wikilinks("", "See [[some/note]] and [[other|Other Note]].", "");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "some/note");
        assert!(links[0].display.is_none());
        assert_eq!(links[1].target, "other");
        assert_eq!(links[1].display.as_deref(), Some("Other Note"));
    }

    #[test]
    fn wikilinks_reference() {
        let links = extract_wikilinks("", "", "- related:: [[20260226120000]]");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].zone, Zone::Reference);
    }

    #[test]
    fn wikilinks_frontmatter() {
        let links = extract_wikilinks("related: \"[[20260226120000|My Note]]\"", "", "");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].zone, Zone::Frontmatter);
        assert_eq!(links[0].display.as_deref(), Some("My Note"));
    }

    // -- wikilink rewriting tests --

    #[test]
    fn rewrite_wikilinks_bare() {
        let content = "See [[old_target]] here.";
        let result = rewrite_wikilinks(content, "old_target", "new_target");
        assert_eq!(result, "See [[new_target]] here.");
    }

    #[test]
    fn rewrite_wikilinks_with_display() {
        let content = "Link: [[old_target|Display Name]]";
        let result = rewrite_wikilinks(content, "old_target", "new_target");
        assert_eq!(result, "Link: [[new_target|Display Name]]");
    }

    #[test]
    fn rewrite_wikilinks_yaml_quoted() {
        let content = "related: \"[[old_target|Name]]\"";
        let result = rewrite_wikilinks(content, "old_target", "new_target");
        assert_eq!(result, "related: \"[[new_target|Name]]\"");
    }

    #[test]
    fn rewrite_wikilinks_reference_section() {
        let content = "- related:: [[old_target]]";
        let result = rewrite_wikilinks(content, "old_target", "new_target");
        assert_eq!(result, "- related:: [[new_target]]");
    }

    #[test]
    fn rewrite_wikilinks_multiple_occurrences() {
        let content = "First [[old_target]] then [[old_target|Alt]] and [[other]]";
        let result = rewrite_wikilinks(content, "old_target", "new_target");
        assert_eq!(
            result,
            "First [[new_target]] then [[new_target|Alt]] and [[other]]"
        );
    }

    #[test]
    fn rewrite_wikilinks_no_match() {
        let content = "Nothing to change [[unrelated]]";
        let result = rewrite_wikilinks(content, "old_target", "new_target");
        assert_eq!(result, "Nothing to change [[unrelated]]");
    }

    #[test]
    fn rewrite_wikilinks_path_qualified() {
        let content = "See [[zettelkasten/20260301120000]]";
        let result = rewrite_wikilinks(
            content,
            "zettelkasten/20260301120000",
            "zettelkasten/contact/20260301120000",
        );
        assert_eq!(result, "See [[zettelkasten/contact/20260301120000]]");
    }

    // -- checkbox tests --

    #[test]
    fn checkboxes_all_states() {
        let body = "- [ ] open item\n- [x] done item\n- [i] info item";
        let cbs = extract_checkboxes(body);
        assert_eq!(cbs.len(), 3);
        assert_eq!(cbs[0].state, crate::types::CheckboxState::Open);
        assert_eq!(cbs[0].content, "open item");
        assert_eq!(cbs[1].state, crate::types::CheckboxState::Done);
        assert_eq!(cbs[1].content, "done item");
        assert_eq!(cbs[2].state, crate::types::CheckboxState::Info);
        assert_eq!(cbs[2].content, "info item");
    }

    #[test]
    fn checkboxes_date_prefix() {
        let body = "- [i] 2026-02-20 20:54 - Issue created: ENG-1234";
        let cbs = extract_checkboxes(body);
        assert_eq!(cbs.len(), 1);
        assert_eq!(cbs[0].date, Some("2026-02-20 20:54".into()));
        assert_eq!(cbs[0].content, "Issue created: ENG-1234");
    }

    #[test]
    fn checkboxes_due_date() {
        let body = "- [ ] Do the thing ⏳ 2026-03-20";
        let cbs = extract_checkboxes(body);
        assert_eq!(cbs.len(), 1);
        assert_eq!(cbs[0].due_date, Some("2026-03-20".into()));
        assert_eq!(cbs[0].content, "Do the thing");
    }

    #[test]
    fn checkboxes_skip_code_block() {
        let body = "- [ ] real\n```\n- [ ] fake\n```\n- [x] also real";
        let cbs = extract_checkboxes(body);
        assert_eq!(cbs.len(), 2);
        assert_eq!(cbs[0].content, "real");
        assert_eq!(cbs[1].content, "also real");
    }

    #[test]
    fn checkboxes_line_numbers() {
        let body = "Some text\n- [ ] first\nmore text\n- [x] second";
        let cbs = extract_checkboxes(body);
        assert_eq!(cbs.len(), 2);
        assert_eq!(cbs[0].line_number, 2);
        assert_eq!(cbs[1].line_number, 4);
    }

    #[test]
    fn checkboxes_indent_level() {
        let body = "- [ ] parent\n  - [ ] child\n    - [x] grandchild";
        let cbs = extract_checkboxes(body);
        assert_eq!(cbs.len(), 3);
        assert_eq!(cbs[0].indent_level, 0);
        assert_eq!(cbs[0].content, "parent");
        assert_eq!(cbs[1].indent_level, 2);
        assert_eq!(cbs[1].content, "child");
        assert_eq!(cbs[2].indent_level, 4);
        assert_eq!(cbs[2].content, "grandchild");
    }

    // -- hashtag tests --

    #[test]
    fn hashtags_basic() {
        let body = "Some text #gtd/act/next and more";
        let tags = extract_hashtags(body);
        assert_eq!(tags, vec!["gtd/act/next"]);
    }

    #[test]
    fn hashtags_hierarchical() {
        let body = "Tagged #client/100-acme-corp here";
        let tags = extract_hashtags(body);
        assert_eq!(tags, vec!["client/100-acme-corp"]);
    }

    #[test]
    fn hashtags_skip_fenced_code() {
        let body = "Before\n```\n#not-a-tag\n```\nAfter #real-tag";
        let tags = extract_hashtags(body);
        assert_eq!(tags, vec!["real-tag"]);
    }

    #[test]
    fn hashtags_skip_inline_code() {
        let body = "See `#not-a-tag` but #real-tag";
        let tags = extract_hashtags(body);
        assert_eq!(tags, vec!["real-tag"]);
    }

    #[test]
    fn hashtags_skip_wikilinks() {
        let body = "Link [[#heading]] and #real-tag";
        let tags = extract_hashtags(body);
        assert_eq!(tags, vec!["real-tag"]);
    }

    #[test]
    fn hashtags_dedup() {
        let body = "#duplicate and #duplicate again";
        let tags = extract_hashtags(body);
        assert_eq!(tags, vec!["duplicate"]);
    }

    #[test]
    fn hashtags_line_start_and_mid() {
        let body = "#start-tag\nsome #mid-tag text";
        let tags = extract_hashtags(body);
        assert_eq!(tags, vec!["start-tag", "mid-tag"]);
    }

    #[test]
    fn hashtags_not_in_urls() {
        let body = "Visit https://example.com#section for info";
        let tags = extract_hashtags(body);
        assert!(
            tags.is_empty(),
            "URL fragments should not be extracted: {tags:?}"
        );
    }

    #[test]
    fn hashtags_whitespace_required() {
        let body = "word#not-a-tag but #real-tag";
        let tags = extract_hashtags(body);
        assert_eq!(tags, vec!["real-tag"]);
    }

    // -- serialization tests --

    #[test]
    fn serialize_round_trip() {
        let content = "\
---
id: 20260226120000
title: Test Note
date: 2026-02-26
type: permanent
tags:
  - test
  - demo
---
Body content here.

Some more body.
---
- source:: Wikipedia
- tags:: test";

        let z = split_zones(content).unwrap();
        let meta = parse_frontmatter(&z.raw_frontmatter, "20260226120000.md").unwrap();
        let inline_fields = extract_inline_fields(&z.body, &z.reference_section).unwrap();
        let wikilinks = extract_wikilinks(&z.raw_frontmatter, &z.body, &z.reference_section);

        let parsed = crate::types::ParsedZettel {
            meta,
            body: z.body.clone(),
            reference_section: z.reference_section.clone(),
            inline_fields,
            links: wikilinks,
            body_tags: vec![],
            checkboxes: vec![],
            path: "20260226120000.md".into(),
        };

        let serialized = serialize(&parsed);

        // Re-parse and verify equivalence
        let z2 = split_zones(&serialized).unwrap();
        let meta2 = parse_frontmatter(&z2.raw_frontmatter, "20260226120000.md").unwrap();
        assert_eq!(meta2.id, parsed.meta.id);
        assert_eq!(meta2.title, parsed.meta.title);
        assert_eq!(meta2.tags, parsed.meta.tags);
        assert!(z2.body.contains("Body content here."));
        assert!(z2.reference_section.contains("- source:: Wikipedia"));
    }

    #[test]
    fn serialize_no_reference_section() {
        let parsed = crate::types::ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260226120000".into())),
                title: Some("Test".into()),
                ..Default::default()
            },
            body: "Just body.".into(),
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "test.md".into(),
        };

        let serialized = serialize(&parsed);
        assert!(serialized.contains("title: Test"));
        assert!(serialized.contains("Just body."));
        // Should not have trailing ---
        assert_eq!(serialized.matches("---").count(), 2);
    }

    #[test]
    fn serialize_canonical_yaml_key_ordering() {
        let mut extra = std::collections::BTreeMap::new();
        extra.insert("zeta".to_string(), Value::String("last".into()));
        extra.insert("publish".to_string(), Value::Bool(true));
        extra.insert("alpha".to_string(), Value::String("first".into()));
        extra.insert("processed".to_string(), Value::Bool(false));

        let parsed = crate::types::ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260226120000".into())),
                title: Some("Ordering Test".into()),
                date: Some("2026-02-26".into()),
                tags: vec!["test".into(), "order".into()],
                zettel_type: Some("permanent".into()),
                extra,
            },
            body: "Body.".into(),
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "test.md".into(),
        };

        let serialized = serialize(&parsed);
        let lines: Vec<&str> = serialized.lines().collect();

        let tags_idx = lines.iter().position(|l| *l == "tags:").unwrap();
        let type_idx = lines.iter().position(|l| *l == "type: permanent").unwrap();
        let publish_idx = lines.iter().position(|l| *l == "publish: true").unwrap();
        let processed_idx = lines.iter().position(|l| *l == "processed: false").unwrap();
        let alpha_idx = lines.iter().position(|l| *l == "alpha: first").unwrap();
        let zeta_idx = lines.iter().position(|l| *l == "zeta: last").unwrap();

        assert!(tags_idx < type_idx);
        assert!(type_idx < publish_idx);
        assert!(type_idx < processed_idx);
        assert!(publish_idx < processed_idx);
        assert!(processed_idx < alpha_idx);
        assert!(alpha_idx < zeta_idx);
    }

    // -- top-level parse tests --

    #[test]
    fn parse_full_zettel() {
        let content = "\
---
id: 20260226120000
title: Full Note
date: 2026-02-26
type: permanent
tags:
  - test
---
Body with [[some/link|Link]] and source:: Wikipedia
---
- related:: [[20260101000000]]";

        let p = parse(content, "zettelkasten/20260226120000.md").unwrap();
        assert_eq!(p.meta.id, Some(ZettelId("20260226120000".into())));
        assert_eq!(p.meta.title.as_deref(), Some("Full Note"));
        assert_eq!(p.inline_fields.len(), 1); // related from ref (source:: not at line start)
        assert_eq!(p.links.len(), 2); // one in body, one in ref
    }

    #[test]
    fn parse_minimal_zettel() {
        let content = "\
---
title: Minimal
---
Just body.";

        let p = parse(content, "zettelkasten/20260226130000.md").unwrap();
        assert_eq!(p.meta.id, Some(ZettelId("20260226130000".into()))); // from filename
        assert!(p.reference_section.is_empty());
    }

    #[test]
    fn parse_obsidian_passthrough() {
        let content = "\
---
title: Obsidian Test
---
Some text.

```dataview
TABLE file.ctime AS Created
FROM #notes
```

<% tp.date.now() %>

Body continues.";

        let p = parse(content, "test.md").unwrap();
        // Obsidian-specific syntax preserved verbatim
        let rt = serialize(&p);
        assert!(rt.contains("```dataview"));
        assert!(rt.contains("<% tp.date.now() %>"));
    }

    // -- ID generation tests --

    #[test]
    fn id_generation_14_digits() {
        let id = generate_id();
        let s = id.0.to_string();
        assert_eq!(s.len(), 14);
    }

    #[test]
    fn id_generation_no_duplicates() {
        let a = generate_id();
        let b = generate_id();
        assert_ne!(a, b, "rapid consecutive calls must produce unique IDs");
        assert_eq!(b.0.len(), 14);
    }
}
