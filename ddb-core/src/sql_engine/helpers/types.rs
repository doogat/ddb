use regex::Regex;
use sqlparser::ast::{CharacterLength, ColumnOption, DataType, Expr};
use std::sync::OnceLock;

use crate::error::Result;
use crate::types::Value;

// --- Data type conversion ---

pub(crate) fn data_type_to_string(dt: &DataType) -> String {
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

pub(crate) fn extract_references(options: &[sqlparser::ast::ColumnOptionDef]) -> Option<String> {
    for opt in options {
        if let ColumnOption::ForeignKey { foreign_table, .. } = &opt.option {
            return Some(super::identifiers::unquote_identifier(
                &foreign_table.to_string(),
            ));
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
pub(crate) fn extract_on_delete(
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
pub(crate) fn is_not_null(options: &[sqlparser::ast::ColumnOptionDef]) -> bool {
    options
        .iter()
        .any(|opt| matches!(opt.option, ColumnOption::NotNull))
}

pub(crate) fn extract_allowed_values(dt: &DataType) -> Option<Vec<String>> {
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

pub(crate) fn extract_default(
    options: &[sqlparser::ast::ColumnOptionDef],
) -> Result<Option<String>> {
    use crate::error::DoogatError;

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
            return Ok(super::eval::expr_to_string(expr).ok());
        }
    }
    Ok(None)
}

// --- Type detection ---

pub(crate) fn is_numeric_type(dt: &str) -> bool {
    matches!(dt.to_uppercase().as_str(), "INTEGER" | "REAL" | "BOOLEAN")
}

/// Rewrite the PostgreSQL shorthand `ALTER COLUMN <c> TYPE <t>` into the
/// standard `ALTER COLUMN <c> SET DATA TYPE <t>` form that the GenericDialect
/// parser accepts. The rewrite skips text inside SQL string literals (`'...'`,
/// `"..."`) and `--` comments so multi-statement batches that mix DDL with
/// DML literal text remain intact.
pub(crate) fn normalize_alter_column_type(sql: &str) -> std::borrow::Cow<'_, str> {
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
pub(crate) fn is_short_string_type(dt: &DataType) -> bool {
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

pub(crate) fn to_yaml_value(val: &str, data_type: &str) -> Value {
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
