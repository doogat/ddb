use regex::Regex;

use super::yaml_bridge::RawDoogatMeta;
use crate::error::{DoogatError, Result};
use crate::types::{Doogat, DoogatId, DoogatMeta};

/// Strip outer `[[...]]` wikilink syntax from a value, discarding any `|display` portion.
/// Returns the inner target unchanged when brackets are present; returns the original value otherwise.
pub(super) fn strip_wikilink(val: &str) -> &str {
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

/// Whether `content` has a delimited frontmatter block (an opening and a
/// closing `---`), judged by the same rule `split_zones` uses. Says nothing
/// about whether the YAML inside the block is valid. A body with two `---`
/// lines anywhere counts as a block, so such a collision loser takes the
/// rewrite branch (and may abort); tightening this is deferred to a follow-up
/// PRD.
pub(crate) fn has_frontmatter(content: &str) -> bool {
    let lines: Vec<&str> = content.lines().collect();
    find_frontmatter(&lines).is_ok()
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

/// Generous cap on a single doogat's frontmatter block, checked before
/// `serde_yaml::from_str` runs. Bounds parse cost against an adversarial or
/// corrupt synced file; degrades into the standard MalformedYaml skip path
/// rather than a fatal error (no new ConsistencyWarning variant needed —
/// this reuses the existing parse-error to MalformedYaml classification).
pub(super) const MAX_FRONTMATTER_BYTES: usize = 256 * 1024; // 256 KiB

/// Parse YAML frontmatter string into DoogatMeta.
/// Falls back to filename-based ID when `id` field is missing.
pub fn parse_frontmatter(yaml: &str, path: &str) -> Result<DoogatMeta> {
    if yaml.len() > MAX_FRONTMATTER_BYTES {
        return Err(DoogatError::Parse(format!(
            "frontmatter exceeds {MAX_FRONTMATTER_BYTES} byte cap ({} bytes)",
            yaml.len()
        )));
    }
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
