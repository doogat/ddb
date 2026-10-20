use crate::error::{codes, DoogatError, Result};
use crate::sql_engine::{SqlEngine, SqlResult};

/// Split an `ADD/DROP UNIQUE (...)` column list into trimmed, unquoted,
/// lowercase column names.
pub(in crate::sql_engine) fn parse_quoted_column_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|c| c.trim().trim_matches('"').to_lowercase())
        .collect()
}

fn sorted(columns: &[String]) -> Vec<String> {
    let mut out = columns.to_vec();
    out.sort();
    out
}

impl<'a> SqlEngine<'a> {
    /// Column names currently present on the live materialized table, or an
    /// empty vec if the table or pragma query fails.
    fn live_table_columns(&self, table_name: &str) -> Vec<String> {
        self.index
            .sql_conn()
            .prepare(&format!("PRAGMA table_info(\"{table_name}\")"))
            .and_then(|mut stmt| {
                stmt.query_map([], |row| row.get::<_, String>(1))
                    .map(|rows| rows.flatten().collect::<Vec<_>>())
            })
            .unwrap_or_default()
    }

    pub(in crate::sql_engine) fn handle_add_unique_constraint(
        &mut self,
        table_name: &str,
        columns: &[String],
    ) -> Result<SqlResult> {
        let mut schema = self.load_schema(table_name)?;
        let cols = columns.join(", ");
        for col in columns {
            if !schema.columns.iter().any(|c| c.name.to_lowercase() == *col) {
                return Err(DoogatError::SqlEngine(format!("column not found: {col}")));
            }
        }
        let key = sorted(columns);
        let existing = schema.unique_together.get_or_insert_with(Vec::new);
        if existing.iter().any(|c| sorted(c) == key) {
            return Ok(SqlResult::Ok(format!(
                "unique constraint ({cols}) already present on {table_name}"
            )));
        }
        let quoted: Vec<String> = columns.iter().map(|c| format!("\"{c}\"")).collect();
        let group_by = quoted.join(", ");
        let not_null = quoted
            .iter()
            .map(|c| format!("{c} IS NOT NULL"))
            .collect::<Vec<_>>()
            .join(" AND ");
        // A column added earlier in the same ALTER sequence may still be
        // buffered: `rematerialize_or_defer` (ddl/alter.rs) pushes the table
        // onto the pending-rematerialize list inside a transaction rather
        // than rebuilding it immediately, so the live materialized table may
        // not have the column yet. Querying it would fail with "no such
        // column"; a column with no rows yet cannot hold a duplicate, so
        // skip the check for it.
        let live_columns = self.live_table_columns(table_name);
        let pending_rematerialize = columns
            .iter()
            .any(|c| !live_columns.iter().any(|lc| lc.eq_ignore_ascii_case(c)));
        let dup_count: i64 = if pending_rematerialize {
            0
        } else {
            self.index.sql_conn().query_row(
                &format!(
                    "SELECT COUNT(*) FROM (SELECT 1 FROM \"{table_name}\" WHERE {not_null} GROUP BY {group_by} HAVING COUNT(*) > 1)"
                ),
                [],
                |row| row.get(0),
            )?
        };
        if dup_count > 0 {
            return Err(DoogatError::Structured {
                code: codes::UNIQUE_VIOLATION,
                message: format!(
                    "cannot add unique constraint ({cols}) on {table_name}: {dup_count} duplicate group(s) exist; remove duplicates and retry"
                ),
                context: vec![],
            });
        }
        existing.push(columns.to_vec());
        self.update_typedef(table_name, &schema)?;
        self.rematerialize_or_defer(table_name)?;
        Ok(SqlResult::Ok(format!(
            "unique constraint ({cols}) added on {table_name}"
        )))
    }

    pub(in crate::sql_engine) fn handle_drop_unique_constraint(
        &mut self,
        table_name: &str,
        columns: &[String],
    ) -> Result<SqlResult> {
        let mut schema = self.load_schema(table_name)?;
        let cols = columns.join(", ");
        let key = sorted(columns);
        let mut constraints = schema.unique_together.take().unwrap_or_default();
        let before = constraints.len();
        constraints.retain(|c| sorted(c) != key);
        if constraints.len() == before {
            return Ok(SqlResult::Ok(format!(
                "unique constraint ({cols}) already absent on {table_name}"
            )));
        }
        schema.unique_together = (!constraints.is_empty()).then_some(constraints);
        self.update_typedef(table_name, &schema)?;
        self.rematerialize_or_defer(table_name)?;
        Ok(SqlResult::Ok(format!(
            "unique constraint ({cols}) dropped on {table_name}"
        )))
    }
}
