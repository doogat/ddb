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

    /// Resolve `desired_col` to the name it must be queried under on the
    /// LIVE materialized table right now: itself when already live, or its
    /// pre-rename name when a `RenameColumn` buffered earlier in this same
    /// transaction renamed it here (`rematerialize_or_defer` defers the
    /// rebuild to commit, so the live table still carries the old name) --
    /// `None` only when the column is genuinely new and holds no live data
    /// under any name (PRD 00184 task 8; task 6 is the `None` case alone).
    fn live_probe_column(
        &self,
        table_name: &str,
        desired_col: &str,
        live_columns: &[String],
    ) -> Option<String> {
        if live_columns.iter().any(|lc| lc.eq_ignore_ascii_case(desired_col)) {
            return Some(desired_col.to_string());
        }
        let old_name = self.txn.as_ref()?.column_renames.iter().find_map(|(t, old, new)| {
            (t.eq_ignore_ascii_case(table_name) && new.eq_ignore_ascii_case(desired_col))
                .then(|| old.clone())
        })?;
        live_columns
            .iter()
            .any(|lc| lc.eq_ignore_ascii_case(&old_name))
            .then_some(old_name)
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
        // A column added earlier in the same ALTER sequence may still be
        // buffered: `rematerialize_or_defer` (ddl/alter.rs) pushes the table
        // onto the pending-rematerialize list inside a transaction rather
        // than rebuilding it immediately, so the live materialized table may
        // not have the column yet under this name. A genuinely new column
        // (task 6) holds no live data under any name and is skipped. A
        // RENAMED column (task 8) still holds its pre-existing data, live
        // under its OLD name -- `live_probe_column` resolves it there so the
        // duplicate check still runs against real data instead of treating
        // the rename as data-free. Skipping is all-or-nothing for a
        // composite key: a `unique_together` mixing a resolvable column with
        // a genuinely new, unresolvable one skips the dup check for the
        // whole tuple, so an existing duplicate on the resolvable column
        // alone is caught only later, by the real UNIQUE INDEX at
        // commit-time rematerialize (a raw SQLite error there, not this
        // handler's friendly message) -- deliberately deferred, since that
        // path cannot lose or persist duplicate data silently.
        let live_columns = self.live_table_columns(table_name);
        let probe_columns: Option<Vec<String>> = columns
            .iter()
            .map(|c| self.live_probe_column(table_name, c, &live_columns))
            .collect();
        let dup_count: i64 = match probe_columns {
            None => 0,
            Some(probe_columns) => {
                let probe_quoted: Vec<String> =
                    probe_columns.iter().map(|c| format!("\"{c}\"")).collect();
                let group_by = probe_quoted.join(", ");
                let not_null = probe_quoted
                    .iter()
                    .map(|c| format!("{c} IS NOT NULL"))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                self.index.sql_conn().query_row(
                    &format!(
                        "SELECT COUNT(*) FROM (SELECT 1 FROM \"{table_name}\" WHERE {not_null} GROUP BY {group_by} HAVING COUNT(*) > 1)"
                    ),
                    [],
                    |row| row.get(0),
                )?
            }
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
