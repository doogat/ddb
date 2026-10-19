use regex::Regex;

use super::yaml_bridge::{fence_regex, inline_code_regex};
use super::zones::strip_wikilink;
use crate::types::{InlineField, Link, Zone};

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
