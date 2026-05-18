//! Shared helper for typed INSERT data shaping.
//!
//! Owns the three pieces every typed-create entry point needs:
//! 1. **Default resolution** — `NEXT`, `NEXT(partition)`, static defaults.
//! 2. **Per-column validation** — `allowed_values` (ENUM) and FK existence
//!    against the typedef's declared target table (not the generic `doogats`
//!    index).
//! 3. **Zone-aware `ParsedDoogat` construction** — REFERENCES values land in
//!    the reference zone, body-zone columns in body sections, frontmatter-zone
//!    columns in `extra`.
//!
//! Both the SQL `INSERT` path (`SqlEngine::handle_insert`) and the service
//! `batch_create` / `create_doogat_with_extra` paths route through this
//! helper. Before this module landed, the service paths dumped every typed
//! field into frontmatter and validated FKs against the generic doogats
//! index, producing four user-visible defects (see PRD 00133
//! unify-typed-write-paths-v1).

use std::collections::{btree_map::Entry, BTreeMap};

use crate::error::{DoogatError, Result};
use crate::types::{ColumnDef, TableSchema};

use super::helpers::is_safe_sql_identifier;

/// Per-typedef counters shared across rows in the same logical batch.
///
/// Keyed by column name (the helper is scoped to a single schema per call, so
/// the table name is implicit). Callers that need cross-table batch awareness
/// hold a separate `TypedInsertCounters` per table and copy entries in/out
/// as they invoke the helper for each input.
#[derive(Default)]
pub(crate) struct TypedInsertCounters {
    /// Per-column counter for `DEFAULT NEXT` (no partition).
    pub(crate) bare: BTreeMap<String, i64>,
    /// Per-(column, partition_value) counter for `DEFAULT NEXT(partition)`.
    /// Lazily seeded from SQLite on first use of each partition value.
    pub(crate) partitioned: BTreeMap<(String, String), i64>,
}

/// Fill missing column defaults and validate per-column constraints.
///
/// On success, `col_values` has every column with a `default_value` filled in
/// (NEXT counters incremented in `counters`), and every populated column has
/// passed `allowed_values` and FK existence checks.
pub(crate) fn prepare_typed_insert_validate(
    schema: &TableSchema,
    col_values: &mut BTreeMap<String, String>,
    counters: &mut TypedInsertCounters,
    conn: &rusqlite::Connection,
) -> Result<()> {
    fill_column_defaults(schema, col_values, counters, conn)?;
    validate_allowed_values(schema, col_values)?;
    validate_fk_against_target_table(schema, col_values, conn)?;
    Ok(())
}

fn fill_column_defaults(
    schema: &TableSchema,
    col_values: &mut BTreeMap<String, String>,
    counters: &mut TypedInsertCounters,
    conn: &rusqlite::Connection,
) -> Result<()> {
    for col_def in &schema.columns {
        if col_values.contains_key(&col_def.name) {
            continue;
        }
        let default = match col_def.default_value {
            Some(ref d) => d,
            None => continue,
        };
        let value = resolve_default(default, col_def, schema, col_values, counters, conn)?;
        col_values.insert(col_def.name.clone(), value);
    }
    Ok(())
}

fn resolve_default(
    default: &str,
    col_def: &ColumnDef,
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
    counters: &mut TypedInsertCounters,
    conn: &rusqlite::Connection,
) -> Result<String> {
    if default == "NEXT" {
        let counter = match counters.bare.entry(col_def.name.clone()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let seed = seed_bare_next(conn, &schema.table_name, &col_def.name)?;
                entry.insert(seed)
            }
        };
        *counter += 1;
        return Ok(counter.to_string());
    }
    if let Some(partition_col) = parse_next_partition(default) {
        let partition_val = col_values.get(partition_col).cloned().unwrap_or_default();
        let key = (col_def.name.clone(), partition_val.clone());
        let counter = match counters.partitioned.entry(key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let seed = seed_partitioned_next(
                    conn,
                    &schema.table_name,
                    &col_def.name,
                    partition_col,
                    &partition_val,
                )?;
                entry.insert(seed)
            }
        };
        *counter += 1;
        return Ok(counter.to_string());
    }
    Ok(default.to_owned())
}

fn parse_next_partition(default: &str) -> Option<&str> {
    default
        .strip_prefix("NEXT(")
        .and_then(|s| s.strip_suffix(')'))
}

fn seed_bare_next(conn: &rusqlite::Connection, table: &str, col: &str) -> Result<i64> {
    // These identifiers are formatted into SQL, so keep the validation next to
    // the query even though typedef schemas are validated earlier.
    if !is_safe_sql_identifier(table) || !is_safe_sql_identifier(col) {
        return Ok(0);
    }
    conn.query_row(
        &format!("SELECT COALESCE(MAX(\"{col}\"), 0) FROM \"{table}\""),
        [],
        |row| row.get(0),
    )
    .map_err(|e| DoogatError::Sql(e.to_string()))
}

fn seed_partitioned_next(
    conn: &rusqlite::Connection,
    table: &str,
    col: &str,
    partition_col: &str,
    partition_val: &str,
) -> Result<i64> {
    if !is_safe_sql_identifier(table)
        || !is_safe_sql_identifier(col)
        || !is_safe_sql_identifier(partition_col)
    {
        return Ok(0);
    }
    conn.query_row(
        &format!(
            "SELECT COALESCE(MAX(\"{col}\"), 0) FROM \"{table}\" WHERE \"{partition_col}\" = ?1"
        ),
        rusqlite::params![partition_val],
        |row| row.get(0),
    )
    .map_err(|e| DoogatError::Sql(e.to_string()))
}

fn validate_allowed_values(
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
) -> Result<()> {
    for col in &schema.columns {
        let allowed = match col.allowed_values {
            Some(ref a) => a,
            None => continue,
        };
        let val = match col_values.get(&col.name) {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };
        if !allowed.contains(val) {
            return Err(DoogatError::Validation(format!(
                "column '{}': value '{}' not in allowed values {:?}",
                col.name, val, allowed
            )));
        }
    }
    Ok(())
}

/// Validate that every populated REFERENCES column points at an existing row
/// in its declared target type's materialized table. Queries the target
/// table directly (e.g. `SELECT 1 FROM "category" WHERE id = ?`) instead of
/// the generic `doogats` index — that's the bug PRD 00133 fixes: the old
/// service path queried `doogats`, so a `link.category` REFERENCES could
/// validate against any row of any type.
fn validate_fk_against_target_table(
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
    conn: &rusqlite::Connection,
) -> Result<()> {
    for col in &schema.columns {
        let target_table = match col.references {
            Some(ref t) => t,
            None => continue,
        };
        let ref_id = match col_values.get(&col.name) {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };
        let exists = check_row_exists(conn, target_table, ref_id)?;
        if !exists {
            return Err(DoogatError::dangling_reference(
                schema.table_name.clone(),
                col.name.clone(),
                target_table.clone(),
                ref_id.clone(),
            ));
        }
    }
    Ok(())
}

fn check_row_exists(conn: &rusqlite::Connection, table: &str, id: &str) -> Result<bool> {
    if !is_safe_sql_identifier(table) {
        return Ok(false);
    }
    let table_present: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            rusqlite::params![table],
            |row| row.get(0),
        )
        .map_err(|e| DoogatError::Sql(e.to_string()))?;
    if !table_present {
        return Ok(false);
    }
    conn.query_row(
        &format!("SELECT COUNT(*) > 0 FROM \"{table}\" WHERE id = ?1"),
        rusqlite::params![id],
        |row| row.get(0),
    )
    .map_err(|e| DoogatError::Sql(e.to_string()))
}
