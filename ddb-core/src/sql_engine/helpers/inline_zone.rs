use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::error::{DoogatError, Result};
use crate::types::Zone;

/// Fast-path guard: true when the batch contains a `CREATE TABLE` anywhere.
/// Unanchored so a batch whose first statement is something else (e.g. a leading
/// `CREATE TYPE` or `BEGIN`) is still processed. Cached (like every other regex
/// helper here) so `strip_inline_zones` doesn't recompile per call.
fn re_create_table_guard() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\bCREATE\s+TABLE\b").expect("valid regex"))
}

/// Match each `CREATE TABLE [IF NOT EXISTS] <name>` preamble in a batch,
/// capturing the table name (quoted or bare). Used to locate every table's
/// column list so a multi-statement batch has all inline zones stripped.
fn re_create_table_preamble() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?("[^"]*"|\w+)"#)
            .expect("valid regex")
    })
}

/// The last word token (alphanumeric/underscore run) ending at or before
/// `pos`, skipping intervening whitespace. Empty when none precedes `pos`.
/// `pos` must be a char boundary in `text`.
fn preceding_word(text: &str, pos: usize) -> &str {
    let before = text[..pos].trim_end();
    match before
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
    {
        Some((i, c)) => &before[i + c.len_utf8()..],
        None => before,
    }
}

/// Find the first ZONE keyword followed by a value in text, ignoring
/// occurrences inside single-quoted strings and the `ZONE` that is the tail of
/// a `TIME ZONE` type keyword (so `TIMESTAMP WITH TIME ZONE` is not misread as
/// an inline zone attribute).
/// Returns (value, start_of_ZONE, end_of_value) relative to the input text.
fn find_zone_in_text(text: &str) -> Option<(String, usize, usize)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?i)\bZONE\s+(\w+)").expect("valid regex"));

    let bytes = text.as_bytes();
    let mut from = 0;

    while let Some(cap) = re.captures_at(text, from) {
        let m = cap.get(0).unwrap();
        let value = cap.get(1).unwrap().as_str().to_string();
        let start = m.start();

        // On a skip, advance the cursor past only the `ZONE` keyword (4 bytes),
        // NOT the whole `ZONE <word>` match, so a genuine inline ZONE that
        // immediately follows a skipped one (e.g. `... WITH TIME ZONE ZONE body`)
        // is still found rather than swallowed.
        let skip_to = start + "ZONE".len();

        // Skip a match inside a single-quoted string literal.
        let inside = bytes[..start].iter().filter(|&&b| b == b'\'').count() % 2 == 1;
        if inside {
            from = skip_to;
            continue;
        }

        // Skip the `ZONE` that closes a `TIME ZONE` type keyword.
        if preceding_word(text, start).eq_ignore_ascii_case("time") {
            from = skip_to;
            continue;
        }

        return Some((value, start, m.end()));
    }

    None
}

/// Inline column zones parked per CREATE TABLE, keyed by lowercase table name
/// then lowercase column name.
pub(crate) type TableColumnZones = HashMap<String, HashMap<String, Zone>>;

/// Strip inline `ZONE <frontmatter|body|reference>` column attributes from a
/// batch of SQL, returning the cleaned SQL and a map of table-name ->
/// (column-name -> zone). Every `CREATE TABLE` in the batch is processed (not
/// just the first), so multi-statement migrations that batch many table
/// declarations all have their inline zones stripped and attributed to the
/// right table.
///
/// Non-CREATE statements are a no-op. Columns without a ZONE attribute are
/// left untouched. If no ZONE was found, the original string is returned
/// unchanged (idempotent). Malformed input (unbalanced parens, unterminated
/// quoted identifier) is passed through to the parser rather than panicking.
pub(crate) fn strip_inline_zones(sql: &str) -> Result<(String, TableColumnZones)> {
    // Guard: a batch with no leading CREATE TABLE is a no-op.
    if !re_create_table_guard().is_match(sql) {
        return Ok((sql.to_string(), HashMap::new()));
    }

    let mut tables: TableColumnZones = HashMap::new();
    let mut out = String::with_capacity(sql.len());
    // Byte offset up to which `sql` has been copied into `out`.
    let mut cursor = 0usize;

    for caps in re_create_table_preamble().captures_iter(sql) {
        let preamble = caps.get(0).unwrap();

        // Skip a `CREATE TABLE` that sits inside a single-quoted string literal
        // (e.g. a column default `'CREATE TABLE fake (...)'`): an odd number of
        // `'` before the match means we are inside a literal.
        let in_literal = sql.as_bytes()[..preamble.start()]
            .iter()
            .filter(|&&b| b == b'\'')
            .count()
            % 2
            == 1;
        if in_literal {
            continue;
        }

        let table = caps.get(1).unwrap().as_str().trim_matches('"').to_lowercase();

        // The column list is the first balanced top-level `(...)` after the
        // `CREATE TABLE <name>` preamble. Missing (e.g. CTAS) -> leave alone.
        let Some((rel_open, rel_close)) = find_column_list_bounds(&sql[preamble.end()..]) else {
            continue;
        };
        let open = preamble.end() + rel_open;
        let close = preamble.end() + rel_close;

        // Defensive: never slice backwards into already-copied text. Unreachable
        // for valid input now that literal matches are skipped above, but it
        // guarantees the `&sql[cursor..=open]` below can never panic begin > end.
        if open < cursor {
            continue;
        }

        let Some((map, rebuilt)) = strip_column_list(&sql[open + 1..close])? else {
            continue; // no inline zone in this table -> copy it verbatim later
        };

        tables.insert(table, map);
        out.push_str(&sql[cursor..=open]);
        out.push_str(&rebuilt);
        cursor = close;
    }

    // cursor stays 0 only when no table was modified -> nothing stripped.
    if cursor == 0 {
        return Ok((sql.to_string(), HashMap::new()));
    }
    out.push_str(&sql[cursor..]);
    Ok((out, tables))
}

/// Strip inline zones from a single column-list body (the text between the
/// table's outer parens). Returns the per-column zone map and the rebuilt body,
/// or None when the column list carries no inline zone. Errors only on a
/// recognized-but-invalid zone value.
fn strip_column_list(column_list: &str) -> Result<Option<(HashMap<String, Zone>, String)>> {
    let mut map = HashMap::new();
    let mut new_segments: Vec<String> = Vec::new();
    let mut modified = false;
    for seg in split_top_level_segments(column_list) {
        match parse_segment_zone(seg)? {
            Some((name, zone, new_seg)) => {
                map.insert(name, zone);
                new_segments.push(new_seg);
                modified = true;
            }
            None => new_segments.push(seg.to_string()),
        }
    }
    if modified {
        Ok(Some((map, new_segments.join(", "))))
    } else {
        Ok(None)
    }
}

/// Locate the first top-level parenthesized column list, returning the byte
/// indices of its opening and matching closing paren. Quote-aware: parens
/// inside single-quoted literals don't affect depth. Returns None on a
/// CREATE TABLE with no balanced top-level `(...)` (treated as a no-op by the
/// caller rather than a panic).
fn find_column_list_bounds(sql: &str) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut open_pos = None;
    let mut in_sq = false;
    for (i, c) in sql.char_indices() {
        match c {
            '\'' => in_sq = !in_sq,
            '(' if !in_sq => {
                if depth == 0 {
                    open_pos = Some(i);
                }
                depth += 1;
            }
            ')' if !in_sq => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return open_pos.map(|o| (o, i));
                }
            }
            _ => {}
        }
    }
    None
}

/// Split a column list on top-level commas, tracking paren depth and
/// single-quote state so commas inside `ENUM('a','b')`, `VARCHAR(255)`,
/// `CHECK(...)`, or quoted literals never split a column.
fn split_top_level_segments(column_list: &str) -> Vec<&str> {
    let mut segs = Vec::new();
    let mut depth = 0usize;
    let mut in_sq = false;
    let mut start = 0;
    for (i, c) in column_list.char_indices() {
        match c {
            '\'' => in_sq = !in_sq,
            '(' if !in_sq => depth += 1,
            ')' if !in_sq => depth = depth.saturating_sub(1),
            ',' if depth == 0 && !in_sq => {
                segs.push(&column_list[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    segs.push(&column_list[start..]);
    segs
}

/// Extract a column-def segment's leading identifier (lowercased, quotes
/// stripped) and the byte offset just past it in the original `seg`. Leading
/// whitespace only — trailing whitespace must NOT shift the offset.
fn column_name_and_end(seg: &str) -> (String, usize) {
    let trimmed = seg.trim_start();
    let leading_ws = seg.len() - trimmed.len();
    if let Some(inner) = trimmed.strip_prefix('"') {
        // Quoted identifier: runs to the next quote (or end if unterminated).
        let end = inner.find('"').unwrap_or(inner.len());
        (inner[..end].to_lowercase(), leading_ws + 1 + end + 1)
    } else {
        let name = trimmed.split_whitespace().next().unwrap_or(trimmed);
        (name.to_lowercase(), leading_ws + name.len())
    }
}

/// Validate a zone value token. Reuses the exact wording of `handle_set_zone`.
fn parse_zone_value(value: &str) -> Result<Zone> {
    match value.to_lowercase().as_str() {
        "frontmatter" => Ok(Zone::Frontmatter),
        "body" => Ok(Zone::Body),
        "reference" => Ok(Zone::Reference),
        other => Err(DoogatError::Structured {
            code: crate::error::codes::INVALID_ZONE,
            message: format!("invalid zone: {} (use frontmatter, body, or reference)", other),
            context: vec![],
        }),
    }
}

/// Parse one column-def segment for an inline `ZONE <value>`. Returns the
/// lowercased column name, the parsed zone, and the segment with the matched
/// ` ZONE <value>` run excised — or None if the segment has no inline zone.
/// Errors only on a recognized-but-invalid zone value.
fn parse_segment_zone(seg: &str) -> Result<Option<(String, Zone, String)>> {
    if seg.trim().is_empty() {
        return Ok(None);
    }
    let (name_lower, name_end) = column_name_and_end(seg);

    // Bounds-safe: an unterminated quoted identifier can push name_end past the
    // segment. Treat that as "no inline zone" rather than slicing out of bounds.
    let Some(rest) = seg.get(name_end..) else {
        return Ok(None);
    };

    let Some((zone_value, match_start, match_end)) = find_zone_in_text(rest) else {
        return Ok(None);
    };
    let zone = parse_zone_value(&zone_value)?;
    let new_seg = format!(
        "{}{}",
        &seg[..name_end + match_start],
        &seg[name_end + match_end..]
    );
    Ok(Some((name_lower, zone, new_seg)))
}
