use sqlparser::ast::{Expr, Value as SqlValue};

use crate::error::{DoogatError, Result};

// --- Expression evaluation ---

/// Allowlisted scalar functions that may appear in INSERT/UPDATE expressions.
const ALLOWED_SCALAR_FUNCTIONS: &[&str] = &[
    "COALESCE", "IFNULL", "NULLIF", "ABS", "LENGTH", "LOWER", "UPPER", "TRIM", "TYPEOF", "MIN",
    "MAX",
];

/// Returns true for expressions that are simple literals (no SQLite evaluation needed).
pub(crate) fn is_literal_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Value(_) => true,
        Expr::UnaryOp { expr, .. } => is_literal_expr(expr),
        _ => false,
    }
}

/// Format any expression as valid SQL text (with proper quoting for literals).
pub(crate) fn value_to_sql(expr: &Expr) -> Result<String> {
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

pub(crate) fn expr_to_string(expr: &Expr) -> Result<String> {
    Ok(expr_to_string_nullable(expr)?.unwrap_or_default())
}

/// Like `expr_to_string` but distinguishes a SQL `NULL` literal (returned as
/// `Ok(None)`) from an empty string. The validator-side caller (PRD 00122)
/// uses this to detect bare-NULL writes that would otherwise collapse to "".
pub(crate) fn expr_to_string_nullable(expr: &Expr) -> Result<Option<String>> {
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
pub(crate) fn sqlite_value_to_string_nullable(
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
pub(crate) fn eval_expr(conn: &rusqlite::Connection, expr: &Expr) -> Result<String> {
    Ok(eval_expr_nullable(conn, expr)?.unwrap_or_default())
}

/// Like `eval_expr` but returns `Ok(None)` for SQL NULL results — including
/// expression-synthesized NULL from `COALESCE(NULL, NULL)`, `IFNULL(NULL,
/// NULL)`, `NULLIF(x, x)`, subqueries, etc. The PRD 00122 validator uses
/// this to detect NULL writes against `NOT NULL` columns even when the user
/// wraps NULL in an expression that round-trips through SQLite.
pub(crate) fn eval_expr_nullable(
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

pub(crate) fn eval_values(conn: &rusqlite::Connection, exprs: &[Expr]) -> Result<Vec<String>> {
    exprs.iter().map(|e| eval_expr(conn, e)).collect()
}

/// Like `eval_values` but returns `Ok(None)` for each NULL-producing expression.
pub(crate) fn eval_values_nullable(
    conn: &rusqlite::Connection,
    exprs: &[Expr],
) -> Result<Vec<Option<String>>> {
    exprs.iter().map(|e| eval_expr_nullable(conn, e)).collect()
}
