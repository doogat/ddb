use std::collections::BTreeSet;

use sqlparser::ast::Expr;

use crate::error::{DoogatError, Result};

mod insert;
mod materialize_row;
mod update;
mod validate;

/// Column names that are not declared in `schema.columns` but are accepted by
/// the SQL write path because the doogat pipeline owns them. Listed in
/// `validate_row_against_schema`'s unknown-column check.
const RESERVED_COLUMNS: &[&str] = &[
    "id",
    "title",
    "type",
    "date",
    "created_at",
    "updated_at",
    "tags",
];

/// Type-check a single value against its declared `data_type`. Returns
/// `DoogatError::Validation` on mismatch using the exact strings documented
/// in `docs/src/technical/sql-engine.md`.
fn type_check_value(data_type: &str, table_name: &str, col_name: &str, val: &str) -> Result<()> {
    let dt = data_type.to_uppercase();
    if dt == "INTEGER" {
        if val.parse::<i64>().is_err() {
            return Err(DoogatError::Validation(format!(
                "type mismatch for {table_name}.{col_name}: expected INTEGER, got '{val}'"
            )));
        }
        return Ok(());
    }
    if matches!(dt.as_str(), "REAL" | "FLOAT" | "DOUBLE") {
        if val.parse::<f64>().is_err() {
            return Err(DoogatError::Validation(format!(
                "type mismatch for {table_name}.{col_name}: expected REAL, got '{val}'"
            )));
        }
        return Ok(());
    }
    if dt == "BOOLEAN" {
        if !matches!(val, "0" | "1" | "true" | "false" | "TRUE" | "FALSE") {
            return Err(DoogatError::Validation(format!(
                "type mismatch for {table_name}.{col_name}: expected BOOLEAN, got '{val}'"
            )));
        }
        return Ok(());
    }
    if let Some(limit) = parse_varchar_length(&dt) {
        let chars = val.chars().count();
        if chars > limit {
            return Err(DoogatError::Validation(format!(
                "value too long for {table_name}.{col_name}: {chars} chars exceeds limit {limit}"
            )));
        }
    }
    Ok(())
}

/// Extract the `N` from `VARCHAR(N)` or `CHAR(N)`. Returns `None` for bare
/// `VARCHAR`/`CHAR` (no length cap) and for unparseable inputs.
fn parse_varchar_length(upper_data_type: &str) -> Option<usize> {
    let inner = upper_data_type
        .strip_prefix("VARCHAR(")
        .or_else(|| upper_data_type.strip_prefix("CHAR("))?
        .strip_suffix(')')?;
    inner.parse::<usize>().ok()
}

/// Returns true for column types where an empty string is never a legal
/// value (numeric and boolean). For text-like types (`TEXT`, `VARCHAR`,
/// `CHAR`, `BLOB`), empty strings are accepted because users may legitimately
/// `INSERT (description) VALUES ('')`.
fn is_strict_check_type(data_type: &str) -> bool {
    let upper = data_type.to_uppercase();
    matches!(
        upper.as_str(),
        "INTEGER" | "REAL" | "FLOAT" | "DOUBLE" | "BOOLEAN"
    )
}

/// Returns true when the expression is a bare SQL `NULL` literal. Used by
/// the UPDATE-side `collect_update_metadata` because it walks the raw AST
/// and stringifies before SQLite would otherwise see the value. INSERT-side
/// NULL detection is handled by `eval_values_nullable` which catches both
/// literal and expression-synthesized NULL.
fn is_null_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Value(v) if matches!(v.value, sqlparser::ast::Value::Null)
    )
}

/// Per-row null column sets, parallel to the row vector. Each entry lists
/// the column names whose INSERT VALUES expression evaluated to SQL NULL,
/// captured before the row builder collapses NULL to "".
type NullColsPerRow = Vec<BTreeSet<String>>;
