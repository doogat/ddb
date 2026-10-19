use std::collections::{BTreeMap, BTreeSet};

use crate::error::{DoogatError, Result};
use crate::types::TableSchema;

use crate::sql_engine::SqlEngine;

use super::{is_strict_check_type, type_check_value, NullColsPerRow, RESERVED_COLUMNS};

/// Filtered rows, parallel null-column sets, and a vec of existing IDs for
/// conflict-skipped slots.
type ConflictFilterResult = (Vec<Vec<String>>, NullColsPerRow, Vec<Option<String>>);

impl<'a> SqlEngine<'a> {
    /// Validate a row's column names and values against the table schema.
    ///
    /// Runs six constraint checks that the SQL parser accepts but does not
    /// enforce by default (issue #7):
    ///
    /// 1. **Unknown columns** — names not present in `schema.columns` and not
    ///    in the reserved set are rejected. Reserved: `id`, `title`, `type`,
    ///    `date`, `created_at`, `updated_at`, `tags`.
    /// 2. **NOT NULL** — for INSERT, a `required` column missing from
    ///    `col_values` (no default applied) or present in `null_cols` is
    ///    rejected. For UPDATE, only an explicit `SET col = NULL` (column in
    ///    `null_cols`) on a required column is rejected; UPDATEs that leave a
    ///    column untouched are fine.
    /// 3. **INTEGER** — values must parse as `i64`.
    /// 4. **REAL/FLOAT/DOUBLE** — values must parse as `f64`.
    /// 5. **BOOLEAN** — values must be one of `0`, `1`, `true`, `false`,
    ///    `TRUE`, `FALSE`.
    /// 6. **VARCHAR(N) / CHAR(N) length** — character count must not exceed
    ///    the declared length. Bare `VARCHAR`/`CHAR` (no length) is unbounded.
    ///
    /// `allowed_values` (ENUM) and `REFERENCES` are validated separately in
    /// `super::typed_insert::prepare_typed_insert_validate`. This helper is
    /// additive.
    ///
    /// On failure returns `DoogatError::Validation` with one of the exact
    /// error strings documented in `docs/src/technical/sql-engine.md`.
    pub(in crate::sql_engine) fn validate_row_against_schema(
        schema: &TableSchema,
        table_name: &str,
        col_names: &[String],
        col_values: &BTreeMap<String, String>,
        null_cols: &BTreeSet<String>,
        is_insert: bool,
    ) -> Result<()> {
        Self::check_unknown_columns(schema, table_name, col_names)?;
        Self::check_not_null(schema, table_name, col_values, null_cols, is_insert)?;
        Self::check_column_types(schema, table_name, col_values, null_cols)?;
        Ok(())
    }

    fn check_unknown_columns(
        schema: &TableSchema,
        table_name: &str,
        col_names: &[String],
    ) -> Result<()> {
        let schema_cols: BTreeSet<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
        for name in col_names {
            if schema_cols.contains(name.as_str()) {
                continue;
            }
            if RESERVED_COLUMNS.contains(&name.as_str()) {
                continue;
            }
            return Err(DoogatError::unknown_field(table_name, name));
        }
        Ok(())
    }

    fn check_not_null(
        schema: &TableSchema,
        table_name: &str,
        col_values: &BTreeMap<String, String>,
        null_cols: &BTreeSet<String>,
        is_insert: bool,
    ) -> Result<()> {
        for col in &schema.columns {
            if !col.required {
                continue;
            }
            if null_cols.contains(&col.name) {
                return Err(DoogatError::not_null_violation(table_name, &col.name));
            }
            if is_insert && !col_values.contains_key(&col.name) {
                // Title is special: when the typedef declares a
                // `title_template`, `resolve_insert_title` (priority 2) will
                // synthesize a title from the other columns at write time,
                // so an absent `title` column is fine. Without this
                // exemption, the validator would reject INSERTs that the PRD
                // explicitly says must keep working — see PRD 00122 design
                // section 5 ("title_template feature stays intact"). An
                // explicit `INSERT INTO t (title, ...) VALUES (NULL, ...)`
                // is still rejected via the `null_cols` branch above.
                if col.name == "title" && schema.title_template.is_some() {
                    continue;
                }
                return Err(DoogatError::not_null_violation(table_name, &col.name));
            }
        }
        Ok(())
    }

    fn check_column_types(
        schema: &TableSchema,
        table_name: &str,
        col_values: &BTreeMap<String, String>,
        null_cols: &BTreeSet<String>,
    ) -> Result<()> {
        for col in &schema.columns {
            if null_cols.contains(&col.name) {
                continue;
            }
            let val = match col_values.get(&col.name) {
                Some(v) => v,
                None => continue,
            };
            // Skip empty string for non-numeric/non-bool types so e.g.
            // `INSERT (description) VALUES ('')` still passes for TEXT.
            // Numeric and BOOLEAN types still validate empty strings: those
            // would have come from the user typing `''` literally and are
            // invalid. Expression-synthesized NULL is already handled via
            // `null_cols` above (PRD 00122 blind review C1).
            if val.is_empty() && !is_strict_check_type(&col.data_type) {
                continue;
            }
            type_check_value(&col.data_type, table_name, &col.name, val)?;
        }
        Ok(())
    }

    /// Filter out rows that match existing unique_together constraints when
    /// ON CONFLICT DO NOTHING is active. Filters `rows` and `null_cols_per_row`
    /// in lockstep so the parallel-index invariant downstream stays intact,
    /// and returns the existing IDs for skipped slots.
    pub(super) fn filter_conflict_rows(
        &self,
        rows: Vec<Vec<String>>,
        null_cols_per_row: NullColsPerRow,
        schema: &TableSchema,
        col_names: &[String],
    ) -> Result<ConflictFilterResult> {
        let constraints = match schema.unique_together {
            Some(ref c) => c,
            None => return Ok((rows, null_cols_per_row, vec![])),
        };

        let mut existing: Vec<Option<String>> = vec![None; rows.len()];
        let mut filtered = Vec::with_capacity(rows.len());
        let mut filtered_nulls = Vec::with_capacity(rows.len());

        for (row_idx, (row_values, nulls)) in rows.into_iter().zip(null_cols_per_row).enumerate() {
            if let Some(id) = self.find_conflict_match(schema, constraints, col_names, &row_values)
            {
                existing[row_idx] = Some(id);
            } else {
                filtered.push(row_values);
                filtered_nulls.push(nulls);
            }
        }

        Ok((filtered, filtered_nulls, existing))
    }

    /// Check one row against all unique_together constraint groups, returning
    /// the existing doogat ID if any group matches.
    fn find_conflict_match(
        &self,
        schema: &TableSchema,
        constraints: &[Vec<String>],
        col_names: &[String],
        row_values: &[String],
    ) -> Option<String> {
        for constraint_cols in constraints {
            let where_clause: String = constraint_cols
                .iter()
                .map(|c| format!("\"{}\" = ?", c))
                .collect::<Vec<_>>()
                .join(" AND ");
            let sql = format!(
                "SELECT id FROM \"{}\" WHERE {}",
                schema.table_name, where_clause
            );
            let bind_vals: Vec<String> = constraint_cols
                .iter()
                .filter_map(|col| {
                    col_names
                        .iter()
                        .position(|n| n == col)
                        .and_then(|pos| row_values.get(pos))
                        .cloned()
                })
                .collect();
            if bind_vals.len() != constraint_cols.len() {
                continue;
            }
            let existing_id: Option<String> = self
                .index
                .sql_conn()
                .query_row(&sql, rusqlite::params_from_iter(bind_vals), |row| {
                    row.get(0)
                })
                .ok();
            if existing_id.is_some() {
                return existing_id;
            }
        }
        None
    }
}
