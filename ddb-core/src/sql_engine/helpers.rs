use regex::Regex;
use sqlparser::ast::{
    CharacterLength, ColumnOption, DataType, Expr, SetExpr, Statement, Value as SqlValue,
};
use std::sync::OnceLock;

use crate::error::{DoogatError, Result};
use crate::types::Value;

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
}

