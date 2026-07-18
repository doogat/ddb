use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::error::{DoogatError, Result};
use crate::types::{Doogat, DoogatId, DoogatMeta, InlineField, Link, Value, Zone};

/// Shared regex for fenced code block markers (``` or ~~~).
fn fence_regex() -> &'static Regex {
    static FENCE_RE: OnceLock<Regex> = OnceLock::new();
    FENCE_RE.get_or_init(|| Regex::new(r"^(?:`{3,}|~{3,})").expect("valid regex: fence marker"))
}

/// Shared regex for inline code spans (`...`).
fn inline_code_regex() -> &'static Regex {
    static INLINE_CODE_RE: OnceLock<Regex> = OnceLock::new();
    INLINE_CODE_RE.get_or_init(|| Regex::new(r"`[^`]+`").expect("valid regex: inline code"))
}

impl From<serde_yaml::Error> for DoogatError {
    fn from(e: serde_yaml::Error) -> Self {
        Self::Yaml(e.to_string())
    }
}

/// Internal struct for YAML deserialization. Converts to public DoogatMeta at the boundary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RawDoogatMeta {
    pub id: Option<DoogatId>,
    pub title: Option<String>,
    pub date: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub doogat_type: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

impl From<RawDoogatMeta> for DoogatMeta {
    fn from(raw: RawDoogatMeta) -> Self {
        DoogatMeta {
            id: raw.id,
            title: raw.title,
            date: raw.date,
            doogat_type: raw.doogat_type,
            tags: raw.tags,
            extra: raw
                .extra
                .into_iter()
                .map(|(k, v)| (k, from_serde_yaml(v)))
                .collect(),
        }
    }
}

pub(crate) fn from_serde_yaml(v: serde_yaml::Value) -> Value {
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

/// Strip outer `[[...]]` wikilink syntax from a value, discarding any `|display` portion.
/// Returns the inner target unchanged when brackets are present; returns the original value otherwise.
fn strip_wikilink(val: &str) -> &str {
    let v = val.trim();
    let v = match v.strip_prefix("[[") {
        Some(inner) => inner,
        None => return v,
    };
    let v = match v.strip_suffix("]]") {
        Some(inner) => inner,
        None => return val.trim(),
    };
    // Drop display portion: [[target|display]] → target
    v.split('|').next().unwrap_or(v)
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
pub fn split_zones(content: &str) -> Result<Doogat> {
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

    Ok(Doogat {
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
        .ok_or_else(|| DoogatError::Parse("no frontmatter opening ---".into()))?;

    let second = lines[first + 1..]
        .iter()
        .position(|l| l.trim() == "---")
        .map(|i| i + first + 1)
        .ok_or_else(|| DoogatError::Parse("no frontmatter closing ---".into()))?;

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

/// Parse YAML frontmatter string into DoogatMeta.
/// Falls back to filename-based ID when `id` field is missing.
pub fn parse_frontmatter(yaml: &str, path: &str) -> Result<DoogatMeta> {
    let raw: RawDoogatMeta = if yaml.trim().is_empty() {
        RawDoogatMeta::default()
    } else {
        serde_yaml::from_str(yaml)?
    };
    let mut meta: DoogatMeta = raw.into();

    // Fallback: derive ID from filename stem if not in frontmatter
    if meta.id.is_none() {
        if let Some(stem) = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
        {
            if DoogatId::is_valid_shape(stem) {
                meta.id = Some(DoogatId(stem.to_owned()));
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

    let body_re = BODY_RE.get_or_init(|| {
        Regex::new(r"^([\w][\w\s-]*):: (.+)$").expect("valid regex: body inline field")
    });
    let ref_re = REF_RE.get_or_init(|| {
        Regex::new(r"^- ([\w][\w\s-]*):: ?(.*)$").expect("valid regex: ref inline field")
    });
    let inline_code_re = inline_code_regex();
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
                    return Err(crate::error::DoogatError::Validation(format!(
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
                Some(Zone::Reference) => {
                    // Multi-value: allow repeated keys in reference zone
                    fields.push(InlineField {
                        key,
                        value: strip_wikilink(&caps[2]).to_string(),
                        zone: Zone::Reference,
                    });
                }
                Some(_) => {
                    return Err(crate::error::DoogatError::Validation(format!(
                        "duplicate inline field '{key}' across body and reference zones"
                    )));
                }
                None => {
                    seen.insert(key.clone(), Zone::Reference);
                    fields.push(InlineField {
                        key,
                        value: strip_wikilink(&caps[2]).to_string(),
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

    let re = EMBED_RE.get_or_init(|| {
        Regex::new(r"!\[\[([^\]#|]+)(?:#([^\]|]+))?(?:\|([^\]]+))?\]\]")
            .expect("valid regex: embed")
    });
    let fence_re = fence_regex();
    let inline_code_re = inline_code_regex();

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

/// Extract `[display](target)` markdown links from text, skipping code blocks.
pub fn extract_markdown_links(text: &str, zone: Zone) -> Vec<Link> {
    use std::sync::OnceLock;
    static MD_LINK_RE: OnceLock<Regex> = OnceLock::new();

    // Captures optional `!` prefix to distinguish images from links
    let re = MD_LINK_RE.get_or_init(|| {
        Regex::new(r"(!?)\[([^\]]*)\]\(([^)]+)\)").expect("valid regex: markdown link")
    });
    let fence_re = fence_regex();
    let inline_code_re = inline_code_regex();

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
            // Skip markdown images (![alt](url))
            if &caps[1] == "!" {
                continue;
            }
            links.push(Link {
                target: caps[3].to_string(),
                display: Some(caps[2].to_string()),
                section: None,
                kind: crate::types::LinkKind::MarkdownLink,
                zone: zone.clone(),
            });
        }
    }

    links
}

/// Extract standalone `https://...` or `http://...` URLs from text, skipping code blocks.
///
/// `exclude_targets` contains URLs already captured as markdown link targets (avoid double-counting).
pub fn extract_bare_urls(text: &str, zone: Zone, exclude_targets: &[&str]) -> Vec<Link> {
    use std::sync::OnceLock;
    static URL_RE: OnceLock<Regex> = OnceLock::new();

    let re = URL_RE.get_or_init(|| {
        Regex::new(r"(?:^|\s)(https?://[^\s<>\[\]()]+)").expect("valid regex: bare url")
    });
    let fence_re = fence_regex();
    let inline_code_re = inline_code_regex();

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
            let mut url = caps[1].to_string();
            // Trim trailing punctuation
            while url.ends_with(['.', ',', ';', ':', '!', '?']) {
                url.pop();
            }
            // Trim trailing ) if unbalanced
            while url.matches(')').count() > url.matches('(').count() {
                url.pop();
            }
            if url.is_empty() {
                continue;
            }
            if exclude_targets.contains(&url.as_str()) {
                continue;
            }
            links.push(Link {
                target: url,
                display: None,
                section: None,
                kind: crate::types::LinkKind::BareUrl,
                zone: zone.clone(),
            });
        }
    }

    links
}

/// Extract all link types from the three doogat zones.
///
/// Extraction order: embeds → wikilinks → markdown links → bare URLs.
/// Embeds run before wikilinks so `![[x]]` isn't double-counted as `[[x]]`.
pub fn extract_links(frontmatter: &str, body: &str, reference: &str) -> Vec<Link> {
    let mut links = Vec::new();

    for (text, zone) in [
        (frontmatter, Zone::Frontmatter),
        (body, Zone::Body),
        (reference, Zone::Reference),
    ] {
        // Embeds first (before wikilinks to avoid double-counting)
        let embeds = extract_embeds(text, zone.clone());

        // Collect embed byte ranges to skip in wikilink pass
        let embed_targets: Vec<String> = embeds.iter().map(|e| e.target.clone()).collect();
        links.extend(embeds);

        // Wikilinks — filter out any that overlap with embed matches
        // Embed `![[file#sec|disp]]` also matches wikilink regex as `[[file#sec|disp]]`
        // so we exclude wikilinks whose target (with # stripped) matches an embed target
        let wl = extract_wikilinks_from(text, zone.clone());
        links.extend(wl.into_iter().filter(|l| {
            let base = l.target.split('#').next().unwrap_or(&l.target);
            !embed_targets.iter().any(|et| et == base)
        }));

        // Markdown links
        let md = extract_markdown_links(text, zone.clone());
        let md_targets: Vec<String> = md.iter().map(|l| l.target.clone()).collect();
        links.extend(md);

        // Bare URLs (excluding markdown link targets)
        let md_target_refs: Vec<&str> = md_targets.iter().map(|s| s.as_str()).collect();
        links.extend(extract_bare_urls(text, zone, &md_target_refs));
    }

    links
}

/// Extract `[[target|display]]` wikilinks from a single text zone, skipping code blocks.
fn extract_wikilinks_from(text: &str, zone: Zone) -> Vec<Link> {
    use std::sync::OnceLock;
    static WL_RE: OnceLock<Regex> = OnceLock::new();
    let re = WL_RE.get_or_init(|| {
        Regex::new(r"\[\[([^\]|]+)(?:\|([^\]]+))?\]\]").expect("valid regex: wikilink")
    });
    let fence_re = fence_regex();
    let inline_code_re = inline_code_regex();

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
                display: caps.get(2).map(|m| m.as_str().to_string()),
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: zone.clone(),
            });
        }
    }

    links
}

/// Extract `[[target|display]]` wikilinks from all three zones.
pub fn extract_wikilinks(frontmatter: &str, body: &str, reference: &str) -> Vec<Link> {
    let mut links = Vec::new();
    for (text, zone) in [
        (frontmatter, Zone::Frontmatter),
        (body, Zone::Body),
        (reference, Zone::Reference),
    ] {
        links.extend(extract_wikilinks_from(text, zone));
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

/// Rewrite all internal link types (wikilinks, markdown links, embeds) targeting `old_target`.
///
/// Bare URLs are external and never rewritten.
pub fn rewrite_links(content: &str, old_target: &str, new_target: &str) -> String {
    // 1. Rewrite wikilinks
    let result = rewrite_wikilinks(content, old_target, new_target);

    // 2. Rewrite markdown links: [display](old_target) → [display](new_target)
    use std::sync::OnceLock;
    static MD_REWRITE_RE: OnceLock<Regex> = OnceLock::new();
    let md_re = MD_REWRITE_RE.get_or_init(|| {
        Regex::new(r"\[([^\]]*)\]\(([^)]+)\)").expect("valid regex: md link rewrite")
    });
    let result = md_re
        .replace_all(&result, |caps: &regex::Captures| {
            let target = &caps[2];
            if target == old_target {
                format!("[{}]({})", &caps[1], new_target)
            } else {
                caps[0].to_string()
            }
        })
        .into_owned();

    // 3. Rewrite embeds: ![[old_target]] → ![[new_target]], preserving #section|display
    static EMBED_REWRITE_RE: OnceLock<Regex> = OnceLock::new();
    let embed_re = EMBED_REWRITE_RE.get_or_init(|| {
        Regex::new(r"!\[\[([^\]#|]+)((?:#[^\]|]+)?(?:\|[^\]]+)?)\]\]")
            .expect("valid regex: embed rewrite")
    });
    embed_re
        .replace_all(&result, |caps: &regex::Captures| {
            let target = &caps[1];
            if target == old_target {
                format!("![[{}{}]]", new_target, &caps[2])
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
            out.push_str(&format!("{key}: {}\n", yaml_quote(yaml_val)));
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
