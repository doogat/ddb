use sqlparser::ast::{SetExpr, Statement};

use crate::error::{DoogatError, Result};

// --- SQL identifier helpers ---

/// Strip surrounding double-quotes from a SQL identifier.
/// sqlparser preserves quotes in `to_string()` for identifiers like `"meeting-minutes"`.
pub(crate) fn unquote_identifier(s: &str) -> String {
    s.trim_matches('"').to_lowercase()
}

/// Extract the primary table name from a statement's FROM clause.
/// Returns the first plain table relation found, or None for subqueries/joins/CTEs.
pub(crate) fn extract_from_table(stmt: &Statement) -> Option<String> {
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
pub(crate) fn is_reserved_table(name: &str) -> bool {
    name == "doogats" || name.starts_with("_ddb_") || name.starts_with("sqlite_")
}

/// Validate a typedef rename target name. Rejects empty strings, non-identifier
/// shapes (leading digit, hyphen, dot, whitespace), and reserved internal names
/// (`doogats`, `_typedef`, `_ddb_*`, `sqlite_*`).
///
/// Mirrors `is_valid_graphql_name` from `ddb-server/src/schema/base_types.rs`
/// — keep both in sync. Duplicated here to keep `ddb-core` independent of the
/// server crate.
pub(crate) fn validate_rename_target_name(new_name: &str) -> Result<()> {
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
