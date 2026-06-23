use regex::Regex;
use sqlparser::ast::{
    CharacterLength, ColumnOption, DataType, Expr, SetExpr, Statement, Value as SqlValue,
};
use std::sync::OnceLock;

use crate::error::{DoogatError, Result};
use crate::types::{Value, Zone};
use std::collections::HashMap;

// --- Regex statics ---

pub(super) fn re_set_zone() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+SET\s+ZONE\s+(frontmatter|body|reference)\s+FOR\s+(?:"([^"]+)"|(\w[\w-]*))\s*;?\s*$"#).expect("valid regex")
    })
}

pub(super) fn re_set_title_template() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+SET\s+TITLE\s+TEMPLATE\s+'([^']+)'\s*;?\s*$"#).expect("valid regex")
    })
}

pub(super) fn re_drop_title_template() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+DROP\s+TITLE\s+TEMPLATE\s*;?\s*$"#).expect("valid regex")
    })
}

pub(super) fn re_set_search_key() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+SET\s+SEARCH\s+KEY\s+(?:"([^"]+)"|(\w[\w-]*))\s*;?\s*$"#).expect("valid regex")
    })
}

pub(super) fn re_drop_search_key() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+DROP\s+SEARCH\s+KEY\s*;?\s*$"#,
        )
        .expect("valid regex")
    })
}

/// PRD 00139 §2: detect the trailing `SINGLETON [DEFAULT VALUES]` marker on
/// a CREATE TABLE source string. Anchored to the closing paren of the
/// column list so the marker can only appear after the table body, not
/// inside a column name.
///
/// Match group 1 captures the `DEFAULT VALUES` suffix when present so the
/// caller can branch on whether to auto-seed (T7).
pub(super) fn re_create_table_singleton() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\)\s*SINGLETON(\s+DEFAULT\s+VALUES)?\s*;?\s*$"#).expect("valid regex")
    })
}

/// PRD 00139 §8: `ALTER TABLE x SET SINGLETON` matcher. Mirrors the
/// quoted/bare table-name capture shape of `re_set_search_key`.
pub(super) fn re_set_singleton() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+SET\s+SINGLETON\s*;?\s*$"#)
            .expect("valid regex")
    })
}

/// PRD 00139 §8: `ALTER TABLE x DROP SINGLETON` matcher.
pub(super) fn re_drop_singleton() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+DROP\s+SINGLETON\s*;?\s*$"#,
        )
        .expect("valid regex")
    })
}

pub(super) fn re_unfilled_placeholder() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{[^}]+\}").expect("valid regex"))
}

fn re_title_placeholder() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{([^{}]*)\}").expect("valid regex"))
}

pub(super) fn is_safe_sql_identifier(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A placeholder token parsed from a `title_template`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplatePlaceholder {
    /// The full placeholder string including braces, e.g. `"{col.field}"`.
    pub raw: String,
    /// The column segment (before the dot, or the whole token if no dot).
    pub col: String,
    /// The field segment when the placeholder uses dotted form.
    pub field: Option<String>,
}

/// Parse all `{col}` and `{col.field}` placeholders from a `title_template`.
///
/// Rejects multi-hop paths (`{a.b.c}`) and malformed identifiers. Returns
/// placeholders in the order they appear in the template.
pub fn parse_title_template(tmpl: &str) -> Result<Vec<TemplatePlaceholder>> {
    let mut out = Vec::new();
    for cap in re_title_placeholder().captures_iter(tmpl) {
        let raw = cap.get(0).expect("regex group 0").as_str().to_string();
        let inner = cap.get(1).expect("regex group 1").as_str();
        let parts: Vec<&str> = inner.split('.').collect();
        match parts.as_slice() {
            [col] => {
                if !is_safe_sql_identifier(col) {
                    return Err(DoogatError::SqlEngine(format!(
                        "title_template has malformed placeholder {raw}"
                    )));
                }
                out.push(TemplatePlaceholder {
                    raw,
                    col: (*col).to_string(),
                    field: None,
                });
            }
            [col, field] => {
                if !is_safe_sql_identifier(col) || !is_safe_sql_identifier(field) {
                    return Err(DoogatError::SqlEngine(format!(
                        "title_template has malformed placeholder {raw}"
                    )));
                }
                out.push(TemplatePlaceholder {
                    raw,
                    col: (*col).to_string(),
                    field: Some((*field).to_string()),
                });
            }
            _ => {
                return Err(DoogatError::SqlEngine(format!(
                    "title_template {raw} uses multi-hop path; only one-level REFERENCES dereferencing is supported"
                )));
            }
        }
    }
    Ok(out)
}

// --- SQL identifier helpers ---

/// Strip surrounding double-quotes from a SQL identifier.
/// sqlparser preserves quotes in `to_string()` for identifiers like `"meeting-minutes"`.
pub(super) fn unquote_identifier(s: &str) -> String {
    s.trim_matches('"').to_lowercase()
}

/// Extract the primary table name from a statement's FROM clause.
/// Returns the first plain table relation found, or None for subqueries/joins/CTEs.
pub(super) fn extract_from_table(stmt: &Statement) -> Option<String> {
    let query = match stmt {
        Statement::Query(q) => q,
        _ => return None,
    };
    let select = match query.body.as_ref() {
        SetExpr::Select(s) => s,
        _ => return None,
    };
    if select.from.len() != 1 {
        return None;
    }
    if !select.from[0].joins.is_empty() {
        return None;
    }
    let relation = &select.from[0].relation;
    match relation {
        sqlparser::ast::TableFactor::Table { name, .. } => {
            Some(unquote_identifier(&name.to_string()))
        }
        _ => None,
    }
}

/// Reserved table names that cannot be used for CREATE TABLE.
pub(super) fn is_reserved_table(name: &str) -> bool {
    name == "doogats" || name.starts_with("_ddb_") || name.starts_with("sqlite_")
}

/// Validate a typedef rename target name. Rejects empty strings, non-identifier
/// shapes (leading digit, hyphen, dot, whitespace), and reserved internal names
/// (`doogats`, `_typedef`, `_ddb_*`, `sqlite_*`).
///
/// Mirrors `is_valid_graphql_name` from `ddb-server/src/schema/base_types.rs`
/// — keep both in sync. Duplicated here to keep `ddb-core` independent of the
/// server crate.
pub(super) fn validate_rename_target_name(new_name: &str) -> Result<()> {
    if new_name.is_empty() {
        return Err(DoogatError::SqlEngine(
            "invalid identifier: empty name".into(),
        ));
    }
    let mut chars = new_name.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => {
            return Err(DoogatError::SqlEngine(format!(
                "invalid identifier: {new_name} (must start with letter or underscore)"
            )));
        }
    }
    if !chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return Err(DoogatError::SqlEngine(format!(
            "invalid identifier: {new_name} (only letters, digits, and underscores)"
        )));
    }
    if is_reserved_table(new_name) {
        return Err(DoogatError::SqlEngine(format!(
            "reserved table name: {new_name}"
        )));
    }
    if new_name == "_typedef" {
        return Err(DoogatError::SqlEngine(
            "reserved table name: _typedef".into(),
        ));
    }
    Ok(())
}

// --- Data type conversion ---

pub(super) fn data_type_to_string(dt: &DataType) -> String {
    match dt {
        DataType::Char(Some(CharacterLength::IntegerLength { length, .. })) => {
            format!("CHAR({length})")
        }
        DataType::Char(None) => "CHAR".into(),
        DataType::Character(Some(CharacterLength::IntegerLength { length, .. })) => {
            format!("CHAR({length})")
        }
        DataType::Character(None) => "CHAR".into(),
        DataType::Varchar(Some(CharacterLength::IntegerLength { length, .. }))
        | DataType::CharVarying(Some(CharacterLength::IntegerLength { length, .. })) => {
            format!("VARCHAR({length})")
        }
        DataType::Varchar(_) | DataType::CharVarying(_) => "VARCHAR".into(),
        DataType::TinyText => "TINYTEXT".into(),
        DataType::Text => "TEXT".into(),
        DataType::MediumText => "MEDIUMTEXT".into(),
        DataType::LongText => "LONGTEXT".into(),
        DataType::TinyBlob => "TINYBLOB".into(),
        DataType::Blob(_) => "BLOB".into(),
        DataType::MediumBlob => "MEDIUMBLOB".into(),
        DataType::LongBlob => "LONGBLOB".into(),
        DataType::Binary(_) => "BINARY".into(),
        DataType::Varbinary(_) => "VARBINARY".into(),
        DataType::Enum(..) | DataType::Set(_) => "TEXT".into(),
        DataType::Integer(_) | DataType::Int(_) | DataType::BigInt(_) | DataType::SmallInt(_) => {
            "INTEGER".into()
        }
        DataType::Real | DataType::Float(_) | DataType::Double(_) | DataType::DoublePrecision => {
            "REAL".into()
        }
        DataType::Boolean => "BOOLEAN".into(),
        _ => "TEXT".into(),
    }
}

pub(super) fn extract_references(options: &[sqlparser::ast::ColumnOptionDef]) -> Option<String> {
    for opt in options {
        if let ColumnOption::ForeignKey { foreign_table, .. } = &opt.option {
            return Some(unquote_identifier(&foreign_table.to_string()));
        }
    }
    None
}

/// PRD 00129 §2: extract the `ON DELETE` action off a column-level
/// `REFERENCES` option.
///
/// - Missing clause -> [`OnDeleteAction::Restrict`] (the default
///   established by issue #10 / commit 5a55296).
/// - Explicit `RESTRICT` or `NO ACTION` -> [`OnDeleteAction::Restrict`]
///   (NO ACTION is treated as RESTRICT here since v1 doesn't model the
///   deferred-check distinction).
/// - Explicit `CASCADE` -> [`OnDeleteAction::Cascade`].
/// - `SET NULL` / `SET DEFAULT` -> error (out of scope for v1 per the
///   PRD §Out of scope list).
/// - Any `ON UPDATE` clause -> error (out of scope; `ON UPDATE` is silent
///   today and the PRD explicitly leaves it out).
///
/// Non-FK columns return RESTRICT (the field is meaningless without a
/// REFERENCES clause; downstream code only consults `on_delete` when
/// `references.is_some()`).
pub(super) fn extract_on_delete(
    options: &[sqlparser::ast::ColumnOptionDef],
) -> crate::error::Result<crate::types::OnDeleteAction> {
    use crate::error::DoogatError;
    use crate::types::OnDeleteAction;
    use sqlparser::ast::ReferentialAction;

    for opt in options {
        if let ColumnOption::ForeignKey {
            on_delete,
            on_update,
            ..
        } = &opt.option
        {
            if let Some(action) = on_update {
                return Err(DoogatError::SqlEngine(format!(
                    "ON UPDATE {action} not supported: v1 supports only ON DELETE CASCADE | RESTRICT"
                )));
            }
            return Ok(match on_delete {
                None => OnDeleteAction::Restrict,
                Some(ReferentialAction::Restrict | ReferentialAction::NoAction) => {
                    OnDeleteAction::Restrict
                }
                Some(ReferentialAction::Cascade) => OnDeleteAction::Cascade,
                Some(ReferentialAction::SetNull) => {
                    return Err(DoogatError::SqlEngine(
                        "ON DELETE SET NULL not supported: v1 supports only ON DELETE CASCADE | RESTRICT"
                            .into(),
                    ));
                }
                Some(ReferentialAction::SetDefault) => {
                    return Err(DoogatError::SqlEngine(
                        "ON DELETE SET DEFAULT not supported: v1 supports only ON DELETE CASCADE | RESTRICT"
                            .into(),
                    ));
                }
            });
        }
    }
    Ok(crate::types::OnDeleteAction::Restrict)
}

/// PRD 00160: guard regex matching a leading `CREATE TABLE`. Cached (like every
/// other regex helper here) so `strip_inline_zones` doesn't recompile per call.
fn re_create_table_guard() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*CREATE\s+TABLE\b").expect("valid regex"))
}

/// PRD 00160: match each `CREATE TABLE [IF NOT EXISTS] <name>` preamble in a
/// batch, capturing the table name (quoted or bare). Used to locate every
/// table's column list so a multi-statement batch has all inline zones stripped.
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

/// PRD 00160: inline column zones parked per CREATE TABLE, keyed by lowercase
/// table name then lowercase column name.
pub(super) type TableColumnZones = HashMap<String, HashMap<String, Zone>>;

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
pub(super) fn strip_inline_zones(sql: &str) -> Result<(String, TableColumnZones)> {
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
        let table = caps.get(1).unwrap().as_str().trim_matches('"').to_lowercase();

        // The column list is the first balanced top-level `(...)` after the
        // `CREATE TABLE <name>` preamble. Missing (e.g. CTAS) -> leave alone.
        let Some((rel_open, rel_close)) = find_column_list_bounds(&sql[preamble.end()..]) else {
            continue;
        };
        let open = preamble.end() + rel_open;
        let close = preamble.end() + rel_close;

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
            code: "INVALID_ZONE",
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

/// Returns true when the column declares `NOT NULL` in its DDL options.
pub(super) fn is_not_null(options: &[sqlparser::ast::ColumnOptionDef]) -> bool {
    options
        .iter()
        .any(|opt| matches!(opt.option, ColumnOption::NotNull))
}

pub(super) fn extract_allowed_values(dt: &DataType) -> Option<Vec<String>> {
    match dt {
        DataType::Enum(members, _) => {
            let vals: Vec<String> = members
                .iter()
                .map(|m| match m {
                    sqlparser::ast::EnumMember::Name(n) => n.clone(),
                    sqlparser::ast::EnumMember::NamedValue(n, _) => n.clone(),
                })
                .collect();
            if vals.is_empty() {
                None
            } else {
                Some(vals)
            }
        }
        DataType::Set(vals) => {
            if vals.is_empty() {
                None
            } else {
                Some(vals.clone())
            }
        }
        _ => None,
    }
}

pub(super) fn extract_default(
    options: &[sqlparser::ast::ColumnOptionDef],
) -> Result<Option<String>> {
    for opt in options {
        if let ColumnOption::Default(expr) = &opt.option {
            // Bare DEFAULT NEXT
            if let Expr::Identifier(ident) = expr {
                if ident.value.eq_ignore_ascii_case("next") {
                    return Ok(Some("NEXT".to_string()));
                }
            }
            // DEFAULT NEXT(partition_col)
            if let Expr::Function(func) = expr {
                let func_name = func.name.to_string();
                if func_name.eq_ignore_ascii_case("next") {
                    if let sqlparser::ast::FunctionArguments::List(arg_list) = &func.args {
                        if arg_list.args.is_empty() {
                            return Err(DoogatError::SqlEngine(
                                "DEFAULT NEXT() requires exactly one partition column argument"
                                    .into(),
                            ));
                        }
                        if arg_list.args.len() > 1 {
                            return Err(DoogatError::SqlEngine(
                                "DEFAULT NEXT() accepts only one partition column argument".into(),
                            ));
                        }
                        if let Some(sqlparser::ast::FunctionArg::Unnamed(
                            sqlparser::ast::FunctionArgExpr::Expr(Expr::Identifier(ident)),
                        )) = arg_list.args.first()
                        {
                            return Ok(Some(format!("NEXT({})", ident.value)));
                        }
                        return Err(DoogatError::SqlEngine(
                            "DEFAULT NEXT() argument must be a column name".into(),
                        ));
                    }
                }
            }
            return Ok(expr_to_string(expr).ok());
        }
    }
    Ok(None)
}

// --- Expression evaluation ---

/// Allowlisted scalar functions that may appear in INSERT/UPDATE expressions.
const ALLOWED_SCALAR_FUNCTIONS: &[&str] = &[
    "COALESCE", "IFNULL", "NULLIF", "ABS", "LENGTH", "LOWER", "UPPER", "TRIM", "TYPEOF", "MIN",
    "MAX",
];

/// Returns true for expressions that are simple literals (no SQLite evaluation needed).
pub(super) fn is_literal_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Value(_) => true,
        Expr::UnaryOp { expr, .. } => is_literal_expr(expr),
        _ => false,
    }
}

/// Format any expression as valid SQL text (with proper quoting for literals).
pub(super) fn value_to_sql(expr: &Expr) -> Result<String> {
    match expr {
        Expr::Value(v) => match &v.value {
            SqlValue::SingleQuotedString(s) => {
                let escaped = s.replace('\'', "''");
                Ok(format!("'{escaped}'"))
            }
            SqlValue::DoubleQuotedString(s) => {
                let escaped = s.replace('\'', "''");
                Ok(format!("'{escaped}'"))
            }
            SqlValue::Number(n, _) => Ok(n.clone()),
            SqlValue::Boolean(b) => Ok(if *b { "1" } else { "0" }.to_string()),
            SqlValue::Null => Ok("NULL".to_string()),
            _ => Err(DoogatError::SqlEngine(format!("unsupported value: {v}"))),
        },
        Expr::UnaryOp { op, expr } => {
            let inner = value_to_sql(expr)?;
            Ok(format!("{op}{inner}"))
        }
        Expr::Function(func) => {
            let func_name = func.name.to_string().to_uppercase();
            if !ALLOWED_SCALAR_FUNCTIONS.contains(&func_name.as_str()) {
                return Err(DoogatError::SqlEngine(format!(
                    "function not allowed: {func_name}. Allowed: {}",
                    ALLOWED_SCALAR_FUNCTIONS.join(", ")
                )));
            }
            let args = match &func.args {
                sqlparser::ast::FunctionArguments::List(arg_list) => {
                    let mut parts = Vec::new();
                    for arg in &arg_list.args {
                        match arg {
                            sqlparser::ast::FunctionArg::Unnamed(
                                sqlparser::ast::FunctionArgExpr::Expr(e),
                            ) => parts.push(value_to_sql(e)?),
                            _ => {
                                return Err(DoogatError::SqlEngine(format!(
                                    "unsupported function argument in {func_name}"
                                )))
                            }
                        }
                    }
                    parts.join(", ")
                }
                sqlparser::ast::FunctionArguments::None => String::new(),
                _ => {
                    return Err(DoogatError::SqlEngine(format!(
                        "unsupported function argument style in {func_name}"
                    )))
                }
            };
            Ok(format!("{func_name}({args})"))
        }
        Expr::Subquery(query) => Ok(format!("({query})")),
        Expr::BinaryOp { left, op, right } => {
            let l = value_to_sql(left)?;
            let r = value_to_sql(right)?;
            Ok(format!("({l} {op} {r})"))
        }
        Expr::Nested(inner) => {
            let s = value_to_sql(inner)?;
            Ok(format!("({s})"))
        }
        Expr::Identifier(ident) => Ok(format!("\"{}\"", ident.value)),
        _ => Err(DoogatError::SqlEngine(format!(
            "unsupported expression: {expr}"
        ))),
    }
}

pub(super) fn expr_to_string(expr: &Expr) -> Result<String> {
    Ok(expr_to_string_nullable(expr)?.unwrap_or_default())
}

/// Like `expr_to_string` but distinguishes a SQL `NULL` literal (returned as
/// `Ok(None)`) from an empty string. The validator-side caller (PRD 00122)
/// uses this to detect bare-NULL writes that would otherwise collapse to "".
pub(super) fn expr_to_string_nullable(expr: &Expr) -> Result<Option<String>> {
    match expr {
        Expr::Value(v) => match &v.value {
            SqlValue::SingleQuotedString(s) => Ok(Some(s.clone())),
            SqlValue::DoubleQuotedString(s) => Ok(Some(s.clone())),
            SqlValue::Number(n, _) => Ok(Some(n.clone())),
            SqlValue::Boolean(b) => Ok(Some(b.to_string())),
            SqlValue::Null => Ok(None),
            _ => Err(DoogatError::SqlEngine(format!("unsupported value: {v}"))),
        },
        Expr::UnaryOp { op, expr } => {
            let inner = expr_to_string_nullable(expr)?.unwrap_or_default();
            Ok(Some(format!("{op}{inner}")))
        }
        Expr::Function(_) | Expr::Subquery(_) | Expr::BinaryOp { .. } | Expr::Nested(_) => {
            value_to_sql(expr).map(Some)
        }
        _ => Err(DoogatError::SqlEngine(format!(
            "unsupported expression: {expr}"
        ))),
    }
}

/// Convert a rusqlite Value to an `Option<String>`. SQL NULL becomes
/// `Ok(None)`, distinct from `Ok(Some(""))`. PRD 00122 uses this to flag
/// synthesized NULL from `COALESCE`/`IFNULL`/subqueries against NOT NULL
/// columns.
pub(super) fn sqlite_value_to_string_nullable(
    result: rusqlite::types::Value,
) -> Result<Option<String>> {
    match result {
        rusqlite::types::Value::Text(s) => Ok(Some(s)),
        rusqlite::types::Value::Integer(n) => Ok(Some(n.to_string())),
        rusqlite::types::Value::Real(f) => Ok(Some(f.to_string())),
        rusqlite::types::Value::Null => Ok(None),
        rusqlite::types::Value::Blob(_) => Err(DoogatError::SqlEngine(
            "BLOB result not supported in expression".into(),
        )),
    }
}

/// Evaluate a SQL expression, using SQLite for complex expressions.
/// Simple literals are returned directly without a SQLite roundtrip.
pub(super) fn eval_expr(conn: &rusqlite::Connection, expr: &Expr) -> Result<String> {
    Ok(eval_expr_nullable(conn, expr)?.unwrap_or_default())
}

/// Like `eval_expr` but returns `Ok(None)` for SQL NULL results — including
/// expression-synthesized NULL from `COALESCE(NULL, NULL)`, `IFNULL(NULL,
/// NULL)`, `NULLIF(x, x)`, subqueries, etc. The PRD 00122 validator uses
/// this to detect NULL writes against `NOT NULL` columns even when the user
/// wraps NULL in an expression that round-trips through SQLite.
pub(super) fn eval_expr_nullable(
    conn: &rusqlite::Connection,
    expr: &Expr,
) -> Result<Option<String>> {
    if is_literal_expr(expr) {
        return expr_to_string_nullable(expr);
    }
    let sql = value_to_sql(expr)?;
    let result: rusqlite::types::Value = conn
        .query_row(&format!("SELECT {sql}"), [], |row| row.get(0))
        .map_err(|e| DoogatError::SqlEngine(format!("expression eval failed: {e}")))?;
    sqlite_value_to_string_nullable(result)
}

pub(super) fn eval_values(conn: &rusqlite::Connection, exprs: &[Expr]) -> Result<Vec<String>> {
    exprs.iter().map(|e| eval_expr(conn, e)).collect()
}

/// Like `eval_values` but returns `Ok(None)` for each NULL-producing expression.
pub(super) fn eval_values_nullable(
    conn: &rusqlite::Connection,
    exprs: &[Expr],
) -> Result<Vec<Option<String>>> {
    exprs.iter().map(|e| eval_expr_nullable(conn, e)).collect()
}

// --- WHERE clause extraction ---

pub(super) fn extract_where_id(selection: &Option<Expr>) -> Result<String> {
    match selection {
        Some(Expr::BinaryOp { left, op, right }) => {
            if format!("{op}") != "=" {
                return Err(DoogatError::SqlEngine(
                    "only WHERE id = '<value>' supported".into(),
                ));
            }
            let col = match left.as_ref() {
                Expr::Identifier(ident) => ident.value.to_lowercase(),
                _ => {
                    return Err(DoogatError::SqlEngine(
                        "WHERE clause must be id = '<value>'".into(),
                    ))
                }
            };
            if col != "id" {
                return Err(DoogatError::SqlEngine(
                    "only WHERE id = '<value>' supported".into(),
                ));
            }
            expr_to_string(right)
        }
        _ => Err(DoogatError::SqlEngine(
            "WHERE id = '<value>' required".into(),
        )),
    }
}

/// Try to parse an expression as `column = 'value'`, returning (column_name, value).
fn extract_eq_pair(expr: &Expr) -> Option<(String, String)> {
    let Expr::BinaryOp { left, op, right } = expr else {
        return None;
    };
    if format!("{op}") != "=" {
        return None;
    }
    let Expr::Identifier(ident) = left.as_ref() else {
        return None;
    };
    Some((ident.value.to_lowercase(), expr_to_string(right).ok()?))
}

/// Extract two column values from a WHERE clause like
/// `{col1} = 'val1' AND {col2} = 'val2'`.
pub(super) fn extract_junction_where(
    selection: &Option<Expr>,
    col1_name: &str,
    col2_name: &str,
) -> Result<(String, String)> {
    let err = || {
        DoogatError::SqlEngine(format!(
            "junction DELETE requires WHERE {col1_name} = '...' AND {col2_name} = '...'"
        ))
    };

    let Some(Expr::BinaryOp { left, op, right }) = selection else {
        return Err(err());
    };
    if format!("{op}") != "AND" {
        return Err(err());
    }

    let mut val1 = None;
    let mut val2 = None;
    for side in [left.as_ref(), right.as_ref()] {
        if let Some((col, val)) = extract_eq_pair(side) {
            if col == col1_name {
                val1 = Some(val);
            } else if col == col2_name {
                val2 = Some(val);
            }
        }
    }

    match (val1, val2) {
        (Some(v1), Some(v2)) => Ok((v1, v2)),
        _ => Err(err()),
    }
}

// --- Type detection ---

pub(super) fn is_numeric_type(dt: &str) -> bool {
    matches!(dt.to_uppercase().as_str(), "INTEGER" | "REAL" | "BOOLEAN")
}

/// Rewrite the PostgreSQL shorthand `ALTER COLUMN <c> TYPE <t>` into the
/// standard `ALTER COLUMN <c> SET DATA TYPE <t>` form that the GenericDialect
/// parser accepts. The rewrite skips text inside SQL string literals (`'...'`,
/// `"..."`) and `--` comments so multi-statement batches that mix DDL with
/// DML literal text remain intact.
pub(super) fn normalize_alter_column_type(sql: &str) -> std::borrow::Cow<'_, str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)\b(ALTER\s+COLUMN\s+(?:"[^"]+"|[A-Za-z_][A-Za-z0-9_-]*)\s+)TYPE(\s+)"#)
            .expect("valid regex")
    });

    // Cheap fast path: if neither shorthand fragment nor any quote/comment
    // appears, return the original borrow.
    if !sql.to_ascii_uppercase().contains("ALTER COLUMN") {
        return std::borrow::Cow::Borrowed(sql);
    }

    // Scan the SQL once, copying segments outside string literals/comments
    // into a buffer where the regex rewrite is safe to apply. Inside literals
    // and comments, segments are appended verbatim.
    let bytes = sql.as_bytes();
    let mut out = String::with_capacity(sql.len() + 16);
    let mut i = 0;
    let mut code_start = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'\'' | b'"' => {
                let quote = b;
                // Flush accumulated code-region.
                let segment = &sql[code_start..i];
                out.push_str(&re.replace_all(segment, "${1}SET DATA TYPE$2"));
                // Emit the literal verbatim, including doubled-quote escapes.
                let literal_start = i;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == quote {
                        if i + 1 < bytes.len() && bytes[i + 1] == quote {
                            i += 2; // doubled-quote escape
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                out.push_str(&sql[literal_start..i]);
                code_start = i;
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                let segment = &sql[code_start..i];
                out.push_str(&re.replace_all(segment, "${1}SET DATA TYPE$2"));
                let comment_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                out.push_str(&sql[comment_start..i]);
                code_start = i;
            }
            _ => i += 1,
        }
    }
    let tail = &sql[code_start..];
    out.push_str(&re.replace_all(tail, "${1}SET DATA TYPE$2"));

    if out == sql {
        std::borrow::Cow::Borrowed(sql)
    } else {
        std::borrow::Cow::Owned(out)
    }
}

/// Determine if a SQL data type represents a short string (<=255 chars) that
/// should default to frontmatter zone rather than body.
pub(super) fn is_short_string_type(dt: &DataType) -> bool {
    match dt {
        DataType::Char(_) | DataType::Character(_) | DataType::TinyText => true,
        DataType::Varchar(Some(CharacterLength::IntegerLength { length, .. }))
        | DataType::CharVarying(Some(CharacterLength::IntegerLength { length, .. })) => {
            *length <= 255
        }
        // No size specified — assume short
        DataType::Varchar(None) | DataType::CharVarying(None) => true,
        DataType::Enum(..) | DataType::Set(_) => true,
        _ => false,
    }
}

pub(super) fn to_yaml_value(val: &str, data_type: &str) -> Value {
    match data_type.to_uppercase().as_str() {
        "INTEGER" => val
            .parse::<i64>()
            .map(|i| Value::Number(i as f64))
            .unwrap_or_else(|_| Value::String(val.into())),
        "REAL" => val
            .parse::<f64>()
            .map(Value::Number)
            .unwrap_or_else(|_| Value::String(val.into())),
        "BOOLEAN" => {
            let b = matches!(val.to_lowercase().as_str(), "true" | "1" | "yes");
            Value::Bool(b)
        }
        _ => Value::String(val.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rename_target_name_accepts_valid_identifiers() {
        assert!(validate_rename_target_name("foo").is_ok());
        assert!(validate_rename_target_name("snake_case").is_ok());
        assert!(validate_rename_target_name("camelCase").is_ok());
        assert!(validate_rename_target_name("Field123").is_ok());
        assert!(validate_rename_target_name("_private").is_ok());
    }

    #[test]
    fn validate_rename_target_name_rejects_empty() {
        let err = validate_rename_target_name("").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn validate_rename_target_name_rejects_leading_digit() {
        let err = validate_rename_target_name("123abc").unwrap_err();
        assert!(err.to_string().contains("must start with"));
    }

    #[test]
    fn validate_rename_target_name_rejects_hyphen() {
        let err = validate_rename_target_name("with-hyphen").unwrap_err();
        assert!(err.to_string().contains("only letters"));
    }

    #[test]
    fn validate_rename_target_name_rejects_dot_and_space() {
        assert!(validate_rename_target_name("with.dot").is_err());
        assert!(validate_rename_target_name("with space").is_err());
    }

    #[test]
    fn re_create_table_singleton_matches_trailing_marker() {
        let re = re_create_table_singleton();
        assert!(re.is_match("CREATE TABLE x (a INTEGER) SINGLETON"));
        assert!(re.is_match("create table x (a integer) singleton"));
        assert!(re.is_match("CREATE TABLE x (a INTEGER) SINGLETON;"));
        assert!(re.is_match("CREATE TABLE x (a INTEGER) SINGLETON DEFAULT VALUES"));
        assert!(re.is_match("CREATE TABLE x (a INTEGER) SINGLETON   DEFAULT   VALUES"));
        assert!(re.is_match("CREATE TABLE \"x\" (a INTEGER, b TEXT) SINGLETON DEFAULT VALUES;"));
    }

    #[test]
    fn re_create_table_singleton_captures_default_values_marker() {
        let re = re_create_table_singleton();
        let caps = re
            .captures("CREATE TABLE x (a INTEGER) SINGLETON DEFAULT VALUES")
            .unwrap();
        assert!(caps.get(1).is_some(), "DEFAULT VALUES must be captured");
        let caps_no_dv = re.captures("CREATE TABLE x (a INTEGER) SINGLETON").unwrap();
        assert!(
            caps_no_dv.get(1).is_none(),
            "bare SINGLETON must leave group 1 empty"
        );
    }

    #[test]
    fn re_create_table_singleton_rejects_non_marker_uses() {
        let re = re_create_table_singleton();
        // Plain CREATE TABLE without SINGLETON.
        assert!(!re.is_match("CREATE TABLE x (a INTEGER)"));
        // SINGLETON inside the column list (not a marker).
        assert!(!re.is_match("CREATE TABLE x (singleton INTEGER)"));
        // Marker without closing paren preceding (anchored to `)`).
        assert!(!re.is_match("SELECT 'SINGLETON'"));
    }

    #[test]
    fn re_set_singleton_matches_quoted_and_bare_table_names() {
        let re = re_set_singleton();
        assert!(re.is_match("ALTER TABLE x SET SINGLETON"));
        assert!(re.is_match("ALTER TABLE x SET SINGLETON;"));
        assert!(re.is_match("alter table \"meeting-minutes\" set singleton"));
        assert!(re.is_match("ALTER TABLE  app_config   SET   SINGLETON   ;"));

        let caps = re.captures("ALTER TABLE app_config SET SINGLETON").unwrap();
        assert!(caps.get(1).is_none(), "bare name uses group 2");
        assert_eq!(caps.get(2).unwrap().as_str(), "app_config");

        let caps = re
            .captures("ALTER TABLE \"meeting-minutes\" SET SINGLETON")
            .unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "meeting-minutes");
        assert!(caps.get(2).is_none());
    }

    #[test]
    fn re_set_singleton_rejects_unrelated_alter_forms() {
        let re = re_set_singleton();
        assert!(!re.is_match("ALTER TABLE x SET SINGLETONS"));
        assert!(!re.is_match("ALTER TABLE x SET SINGLETON foo"));
        assert!(!re.is_match("ALTER TABLE x DROP SINGLETON"));
        assert!(!re.is_match("ALTER TABLE x SET SEARCH KEY foo"));
    }

    #[test]
    fn re_drop_singleton_matches_quoted_and_bare_table_names() {
        let re = re_drop_singleton();
        assert!(re.is_match("ALTER TABLE x DROP SINGLETON"));
        assert!(re.is_match("ALTER TABLE x DROP SINGLETON;"));
        assert!(re.is_match("alter table \"meeting-minutes\" drop singleton"));

        let caps = re
            .captures("ALTER TABLE app_config DROP SINGLETON")
            .unwrap();
        assert_eq!(caps.get(2).unwrap().as_str(), "app_config");
    }

    #[test]
    fn re_drop_singleton_rejects_unrelated_alter_forms() {
        let re = re_drop_singleton();
        assert!(!re.is_match("ALTER TABLE x SET SINGLETON"));
        assert!(!re.is_match("ALTER TABLE x DROP SINGLETONS"));
        assert!(!re.is_match("ALTER TABLE x DROP SINGLETON foo"));
    }

    #[test]
    fn validate_rename_target_name_rejects_reserved_names() {
        assert!(validate_rename_target_name("doogats")
            .unwrap_err()
            .to_string()
            .contains("reserved"));
        assert!(validate_rename_target_name("_typedef")
            .unwrap_err()
            .to_string()
            .contains("reserved"));
        assert!(validate_rename_target_name("_ddb_locks")
            .unwrap_err()
            .to_string()
            .contains("reserved"));
        assert!(validate_rename_target_name("sqlite_master")
            .unwrap_err()
            .to_string()
            .contains("reserved"));
    }

    // --- strip_inline_zones tests ---
    //
    // PRD 00160: `strip_inline_zones` returns a table-name -> (column -> zone)
    // map. Single-table tests dig into the one table they declare.

    #[test]
    fn strip_inline_zones_single_zoned_column_maps_and_strips_token() {
        use crate::types::Zone;
        let sql = "CREATE TABLE notes (descr TEXT ZONE frontmatter)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map["notes"]["descr"], Zone::Frontmatter);
        assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
        // Type and column name survive
        assert!(cleaned.to_lowercase().contains("descr"));
        assert!(cleaned.to_lowercase().contains("text"));
    }

    #[test]
    fn strip_inline_zones_maps_two_zoned_columns() {
        use crate::types::Zone;
        let sql = "CREATE TABLE t (title TEXT ZONE frontmatter, body TEXT ZONE body)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert_eq!(map["t"].len(), 2);
        assert_eq!(map["t"]["title"], Zone::Frontmatter);
        assert_eq!(map["t"]["body"], Zone::Body);
        // Neither ZONE token survives
        assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
        assert!(!cleaned.to_lowercase().contains("zone body"));
    }

    #[test]
    fn strip_inline_zones_preserves_enum_comma() {
        use crate::types::Zone;
        let sql = "CREATE TABLE t (status ENUM('a','b') ZONE frontmatter, id INTEGER)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert_eq!(map["t"].len(), 1);
        assert_eq!(map["t"]["status"], Zone::Frontmatter);
        // The ENUM comma must be inside the cleaned column definition, not split
        assert!(cleaned.contains("ENUM('a','b')") || cleaned.contains("ENUM('a', 'b')"));
        assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
    }

    #[test]
    fn strip_inline_zones_quoted_identifier_key_strips_quotes() {
        use crate::types::Zone;
        let sql = r#"CREATE TABLE t ("long-desc" TEXT ZONE frontmatter)"#;
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert_eq!(map["t"].len(), 1);
        assert_eq!(map["t"]["long-desc"], Zone::Frontmatter);
        assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
    }

    #[test]
    fn strip_inline_zones_non_create_table_is_noop() {
        let sql = "INSERT INTO t (a) VALUES (1)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert_eq!(cleaned, sql);
        assert!(map.is_empty());
    }

    #[test]
    fn strip_inline_zones_rejects_invalid_zone_value() {
        let sql = "CREATE TABLE t (x TEXT ZONE sidebar)";
        let err = strip_inline_zones(sql).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid zone: sidebar (use frontmatter, body, or reference)"
        );
    }

    #[test]
    fn strip_inline_zones_column_named_zone_does_not_misfire() {
        use crate::types::Zone;
        // A column named `zone` with no ZONE attribute -> empty map
        let sql = "CREATE TABLE t (zone TEXT)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert!(map.is_empty(), "column named 'zone' must not misfire");
        assert!(
            cleaned.to_lowercase().contains("zone text") || cleaned.to_lowercase().contains("zone")
        );

        // A quoted string default that contains 'zone' must not misfire
        let sql2 = "CREATE TABLE t (descr TEXT DEFAULT 'zone body')";
        let (cleaned2, map2) = strip_inline_zones(sql2).unwrap();
        assert!(
            map2.is_empty(),
            "quoted literal 'zone body' must not misfire"
        );
        assert!(!cleaned2.is_empty());

        // A column named `zone` with a ZONE attribute -> map key `zone`, value Body
        let sql3 = "CREATE TABLE t (zone TEXT ZONE body)";
        let (cleaned3, map3) = strip_inline_zones(sql3).unwrap();
        assert_eq!(map3["t"].len(), 1);
        assert_eq!(map3["t"]["zone"], Zone::Body);
        assert!(
            !cleaned3.to_lowercase().contains("zone body") || {
                // Only allowed if "zone body" in cleaned is the "zone TEXT" column def,
                // not the ZONE attribute. A stricter check: the ZONE keyword appearing
                // as an attribute token should be absent after the column name.
                cleaned3.to_lowercase().matches("zone").count() == 1
            }
        );
    }

    #[test]
    fn strip_inline_zones_case_insensitive_keyword_and_value() {
        use crate::types::Zone;
        let sql = "CREATE TABLE t (descr TEXT zone FRONTMATTER)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert_eq!(map["t"].len(), 1);
        assert_eq!(map["t"]["descr"], Zone::Frontmatter);
        assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
    }

    #[test]
    fn strip_inline_zones_no_zone_create_table_is_idempotent() {
        let sql = "CREATE TABLE t (id INTEGER, name TEXT)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert_eq!(cleaned, sql);
        assert!(map.is_empty());
    }

    #[test]
    fn strip_inline_zones_reference_zone_value_accepted() {
        use crate::types::Zone;
        let sql = "CREATE TABLE t (owner_id INTEGER ZONE reference)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert_eq!(map["t"].len(), 1);
        assert_eq!(map["t"]["owner_id"], Zone::Reference);
        assert!(!cleaned.to_lowercase().contains("zone reference"));
    }

    #[test]
    fn strip_inline_zones_strips_every_table_in_multi_statement_batch() {
        use crate::types::Zone;
        // PRD 00160 Critical (blind review): a multi-statement batch must have
        // EVERY table's inline ZONE stripped and attributed to the right table,
        // not just the first. Same column name in two tables with DIFFERENT
        // zones proves per-table keying (a flat column-keyed map would collide).
        let sql = "CREATE TABLE a (descr TEXT ZONE frontmatter); \
                   CREATE TABLE b (descr TEXT ZONE body)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert_eq!(map["a"]["descr"], Zone::Frontmatter);
        assert_eq!(map["b"]["descr"], Zone::Body);
        // No ZONE attribute token survives in either table (would fail sqlparser).
        assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
        assert!(!cleaned.to_lowercase().contains("zone body"));
    }

    #[test]
    fn strip_inline_zones_batch_with_unzoned_first_table_strips_later_table() {
        use crate::types::Zone;
        // The exact Critical scenario: the first table has no inline zone, a
        // later one does. The later table's ZONE must still be stripped.
        let sql = "CREATE TABLE a (x TEXT); CREATE TABLE b (descr TEXT ZONE frontmatter)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert!(map.get("a").is_none(), "unzoned table absent from map");
        assert_eq!(map["b"]["descr"], Zone::Frontmatter);
        assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
        // The first table's body is untouched.
        assert!(cleaned.contains("CREATE TABLE a (x TEXT)"));
    }

    // PRD 00160 rework (review cycle 1): regression tests for the defects the
    // multi-reviewer cycle found in strip_inline_zones.

    #[test]
    fn strip_inline_zones_trailing_whitespace_before_delimiter_still_maps() {
        use crate::types::Zone;
        // Trailing whitespace before the closing paren / comma must NOT cause the
        // ZONE token to be skipped. (Bug: leading_ws counted trailing whitespace,
        // over-skipping past ZONE so it survived to sqlparser as a parse error.)
        let sql = "CREATE TABLE t (a TEXT ZONE body          , b TEXT)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert_eq!(map["t"].len(), 1, "trailing whitespace must not lose the ZONE");
        assert_eq!(map["t"]["a"], Zone::Body);
        assert!(!cleaned.to_lowercase().contains("zone body"));

        // Also before the closing paren.
        let sql2 = "CREATE TABLE t (a TEXT ZONE frontmatter          )";
        let (_c2, map2) = strip_inline_zones(sql2).unwrap();
        assert_eq!(map2["t"].get("a"), Some(&Zone::Frontmatter));
    }

    #[test]
    fn strip_inline_zones_does_not_panic_on_unterminated_quoted_identifier() {
        // Malformed user SQL must not panic (AGENTS.md no-panic guardrail).
        // (Bug: name_end for an unterminated quoted identifier exceeded seg.len(),
        // making &seg[name_end..] an out-of-bounds slice.)
        let sql = "CREATE TABLE t (\"unterminated TEXT)";
        // Must return (Ok or Err) without panicking.
        let _ = strip_inline_zones(sql);
    }

    #[test]
    fn strip_inline_zones_does_not_panic_on_unbalanced_parens() {
        // A stray ) before any ( must not underflow the depth counter.
        let sql = "CREATE TABLE t )";
        let _ = strip_inline_zones(sql);
        let sql2 = "CREATE TABLE t (a TEXT))";
        let _ = strip_inline_zones(sql2);
    }

    #[test]
    fn strip_inline_zones_time_zone_type_is_not_treated_as_inline_zone() {
        // `TIMESTAMP WITH TIME ZONE` is a standard type; the ZONE inside it must
        // NOT be parsed as an inline zone attribute (which previously errored with
        // `invalid zone: not`). A genuine inline ZONE on another column still maps.
        let sql = "CREATE TABLE t (created TIMESTAMP WITH TIME ZONE NOT NULL)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert!(map.is_empty(), "TIME ZONE must not yield an inline zone");
        assert_eq!(cleaned, sql, "no-zone input round-trips unchanged");

        // A real inline ZONE alongside a TIME ZONE typed column still maps, and
        // the TIME ZONE token is left intact.
        let sql2 = "CREATE TABLE t (created TIMESTAMP WITH TIME ZONE NOT NULL, descr TEXT ZONE frontmatter)";
        let (cleaned2, map2) = strip_inline_zones(sql2).unwrap();
        use crate::types::Zone;
        assert_eq!(map2["t"].get("descr"), Some(&Zone::Frontmatter));
        assert!(map2["t"].get("created").is_none());
        assert!(cleaned2.to_uppercase().contains("TIME ZONE"));
    }

    #[test]
    fn strip_inline_zones_still_errors_on_genuine_bad_value() {
        // The TIME ZONE carve-out must not swallow a real typo: `ZONE sidebar`
        // still errors clearly.
        let sql = "CREATE TABLE t (x TEXT ZONE sidebar)";
        let err = strip_inline_zones(sql).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid zone: sidebar (use frontmatter, body, or reference)"
        );
    }

    #[test]
    fn strip_inline_zones_quoted_paren_before_zone() {
        use crate::types::Zone;
        // A single-quoted '(' before the inline ZONE must not desync the paren
        // scanner that finds the column-list bounds.
        let sql = "CREATE TABLE t (a TEXT DEFAULT '(' ZONE frontmatter)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert_eq!(map["t"].get("a"), Some(&Zone::Frontmatter));
        assert!(cleaned.contains("DEFAULT '('"), "quoted default preserved");
        assert!(!cleaned.to_lowercase().contains("zone frontmatter"));
    }

    #[test]
    fn strip_inline_zones_inline_zone_after_time_zone_type() {
        use crate::types::Zone;
        // A `TIMESTAMP WITH TIME ZONE` column that ALSO declares an inline zone:
        // the TIME ZONE type keyword is skipped, but the trailing real ZONE must
        // still be mapped (the carve-out must not swallow it).
        let sql = "CREATE TABLE t (created TIMESTAMP WITH TIME ZONE ZONE body)";
        let (cleaned, map) = strip_inline_zones(sql).unwrap();
        assert_eq!(map["t"].get("created"), Some(&Zone::Body));
        // The TIME ZONE type keyword survives; the trailing inline ZONE is excised.
        assert!(cleaned.to_uppercase().contains("TIME ZONE"));
        assert!(!cleaned.to_lowercase().contains("zone body"));
    }
}
