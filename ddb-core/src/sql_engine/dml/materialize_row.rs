use std::collections::BTreeMap;

use rusqlite::params;

use crate::error::{DoogatError, Result};
use crate::indexer::materialize::{is_core_column, normalize_bool_str};
use crate::types::TableSchema;

use crate::sql_engine::SqlEngine;

impl<'a> SqlEngine<'a> {
    pub(in crate::sql_engine) fn insert_materialized_row(
        &mut self,
        schema: &TableSchema,
        id: &str,
        col_values: &BTreeMap<String, String>,
    ) -> Result<()> {
        let (title, date, updated_at): (Option<String>, Option<String>, Option<String>) = self
            .index
            .sql_conn()
            .query_row(
                "SELECT title, date, updated_at FROM doogats WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or((None, None, None));

        let mut vals: Vec<Option<String>> = vec![Some(id.to_string()), title, date, updated_at];
        let (col_names, placeholders) = build_insert_columns(schema, col_values, &mut vals);

        let sql = format!(
            "INSERT INTO \"{}\" ({}) VALUES ({})",
            schema.table_name,
            col_names.join(", "),
            placeholders.join(", ")
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = vals
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        self.index
            .sql_conn()
            .execute(&sql, params.as_slice())
            .map_err(|e| {
                classify_materialized_insert_error(e, self.index.sql_conn(), schema, col_values)
            })?;
        Ok(())
    }

    pub(super) fn update_materialized_row(
        &mut self,
        schema: &TableSchema,
        id: &str,
        updates: &BTreeMap<String, String>,
    ) -> Result<()> {
        let mut set_clauses = Vec::new();
        let mut vals: Vec<String> = Vec::new();

        self.append_core_column_sets(id, &mut set_clauses, &mut vals);
        append_update_set_clauses(schema, updates, &mut set_clauses, &mut vals);

        if set_clauses.is_empty() {
            return Ok(());
        }

        vals.push(id.to_string());
        let sql = format!(
            "UPDATE \"{}\" SET {} WHERE id = ?{}",
            schema.table_name,
            set_clauses.join(", "),
            vals.len()
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = vals
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        self.index
            .sql_conn()
            .execute(&sql, params.as_slice())
            .map_err(|e| DoogatError::SqlEngine(e.to_string()))?;
        Ok(())
    }

    fn append_core_column_sets(
        &self,
        id: &str,
        set_clauses: &mut Vec<String>,
        vals: &mut Vec<String>,
    ) {
        let row = self.index.sql_conn().query_row(
            "SELECT title, date, updated_at FROM doogats WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        );
        let (title, date, updated_at) = match row {
            Ok(r) => r,
            Err(_) => return,
        };
        for (col_name, value) in [("title", title), ("date", date), ("updated_at", updated_at)] {
            if let Some(v) = value {
                vals.push(v);
                set_clauses.push(format!("{col_name} = ?{}", vals.len()));
            }
        }
    }
}

/// PRD 00129 §3a + §6: convert a SQLite error from
/// `insert_materialized_row` into the appropriate structured DoogatError.
/// Today the relevant case is the typedef `UNIQUE(...)` index hitting a
/// duplicate tuple; SQLite emits a `SqliteFailure` with code
/// `ConstraintUnique` and a message of the form
/// `UNIQUE constraint failed: <table>.<col>[, <table>.<col>]...`. We
/// extract the column names from the message (so the GraphQL extensions
/// reflect what actually conflicted, not what the typedef declared in
/// case multiple `UNIQUE(...)` groups exist on the same table) and look
/// up the offending values from `col_values`. Anything else falls through
/// to the legacy `DoogatError::SqlEngine(message)`.
fn classify_materialized_insert_error(
    err: rusqlite::Error,
    conn: &rusqlite::Connection,
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
) -> DoogatError {
    use rusqlite::ErrorCode;
    if let rusqlite::Error::SqliteFailure(ref ffi_err, ref msg) = err {
        if ffi_err.code == ErrorCode::ConstraintViolation {
            if let Some(detail) = msg.as_deref() {
                if is_singleton_lock_failure(detail, &schema.table_name) {
                    if let Some(existing_id) =
                        lookup_singleton_existing_id(conn, &schema.table_name)
                    {
                        return DoogatError::singleton_violation(
                            schema.table_name.clone(),
                            existing_id,
                        );
                    }
                }
                if let Some(cols) = parse_unique_failure_columns(detail, &schema.table_name) {
                    let values: Vec<String> = cols
                        .iter()
                        .map(|c| col_values.get(c).cloned().unwrap_or_default())
                        .collect();
                    return DoogatError::unique_violation(schema.table_name.clone(), cols, values);
                }
            }
        }
    }
    DoogatError::SqlEngine(err.to_string())
}

fn lookup_singleton_existing_id(conn: &rusqlite::Connection, table_name: &str) -> Option<String> {
    // Match the Layer 1 ORDER BY in service/validation.rs::check_singleton_constraint
    // so all three enforcement layers report the same `existing_id` for the
    // same set of rows. See singleton_layers.rs:127-141 for the invariant.
    conn.query_row(
        &format!("SELECT id FROM \"{table_name}\" ORDER BY id ASC LIMIT 1"),
        [],
        |row| row.get(0),
    )
    .ok()
}

fn is_singleton_lock_failure(detail: &str, table_name: &str) -> bool {
    // SQLite's UNIQUE-failure message for an expression index always quotes
    // the index name with single quotes, e.g. `UNIQUE constraint failed:
    // index 'app_config_singleton_lock'`. Earlier code also matched double-
    // quoted and backtick variants; SQLite never emits those.
    let prefix = "UNIQUE constraint failed: ";
    let rest = match detail.strip_prefix(prefix) {
        Some(rest) => rest,
        None => return false,
    };
    let index_name = format!("{table_name}_singleton_lock");
    rest == format!("index '{index_name}'")
}

/// Pull the conflicting column names out of SQLite's
/// `"UNIQUE constraint failed: t.col1, t.col2"` message, scoped to the
/// expected table. Returns `None` when the message isn't a UNIQUE
/// failure or doesn't reference the expected table — in that case the
/// caller falls back to the legacy generic error.
fn parse_unique_failure_columns(detail: &str, table_name: &str) -> Option<Vec<String>> {
    let prefix = "UNIQUE constraint failed: ";
    let rest = detail.strip_prefix(prefix)?;
    let qualified_prefix = format!("{table_name}.");
    let cols: Vec<String> = rest
        .split(',')
        .map(str::trim)
        .filter_map(|tok| tok.strip_prefix(&qualified_prefix).map(str::to_string))
        .collect();
    if cols.is_empty() {
        None
    } else {
        Some(cols)
    }
}

/// Build column names and placeholders for INSERT INTO the materialized table.
/// Appends non-core column values to `vals` and returns parallel column/placeholder vecs.
fn build_insert_columns(
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
    vals: &mut Vec<Option<String>>,
) -> (Vec<String>, Vec<String>) {
    let mut col_names = vec![
        "id".to_string(),
        "title".to_string(),
        "date".to_string(),
        "updated_at".to_string(),
    ];
    let mut placeholders = vec![
        "?1".to_string(),
        "?2".to_string(),
        "?3".to_string(),
        "?4".to_string(),
    ];
    let mut param_idx = 5;
    for col in &schema.columns {
        if is_core_column(&col.name) {
            continue;
        }
        col_names.push(format!("\"{}\"", col.name));
        placeholders.push(format!("?{param_idx}"));
        param_idx += 1;
        let val = col_values.get(&col.name).cloned().unwrap_or_default();
        let val = if val.is_empty() {
            None
        } else if col.data_type.eq_ignore_ascii_case("BOOLEAN") {
            Some(normalize_bool_str(&val))
        } else {
            Some(val)
        };
        vals.push(val);
    }
    (col_names, placeholders)
}

/// Append SET clauses for non-core columns in an UPDATE statement.
fn append_update_set_clauses(
    schema: &TableSchema,
    updates: &BTreeMap<String, String>,
    set_clauses: &mut Vec<String>,
    vals: &mut Vec<String>,
) {
    let valid_cols: Vec<&String> = schema
        .columns
        .iter()
        .filter(|c| !is_core_column(&c.name))
        .map(|c| &c.name)
        .collect();

    for (col, val) in updates {
        if !valid_cols.contains(&col) {
            continue;
        }
        let is_bool = schema
            .columns
            .iter()
            .any(|c| &c.name == col && c.data_type.eq_ignore_ascii_case("BOOLEAN"));
        let normalized = if is_bool {
            normalize_bool_str(val)
        } else {
            val.clone()
        };
        vals.push(normalized);
        set_clauses.push(format!("\"{}\" = ?{}", col, vals.len()));
    }
}
