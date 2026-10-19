use std::collections::{BTreeMap, BTreeSet};

use rusqlite::params;
use sqlparser::ast::{AssignmentTarget, Expr, FromTable, Statement};

use crate::error::{DoogatError, Result};
use crate::parser;
use crate::types::TableSchema;

use crate::sql_engine::builders::{apply_updates_to_doogat, recompute_template_title};
use crate::sql_engine::helpers::{
    expr_to_string, extract_from_table, extract_junction_where, extract_where_id, is_literal_expr,
    sqlite_value_to_string_nullable, unquote_identifier, value_to_sql,
};
use crate::sql_engine::{PendingWrite, SqlEngine, SqlResult};

use super::is_null_literal;

/// Partitioned UPDATE assignments: literal values and deferred SQL expressions.
type PartitionedAssignments = (BTreeMap<String, String>, Vec<(String, String)>);

/// Prepared bulk-update output: file contents and per-row update maps.
type BulkUpdateFiles = (Vec<(String, String)>, Vec<BTreeMap<String, String>>);

impl<'a> SqlEngine<'a> {
    pub(in crate::sql_engine) fn handle_update(
        &mut self,
        table: &sqlparser::ast::TableWithJoins,
        assignments: &[sqlparser::ast::Assignment],
        selection: &Option<Expr>,
    ) -> Result<SqlResult> {
        let table_name = unquote_identifier(&table.relation.to_string());
        let schema = self.load_schema(&table_name)?;

        let (mut updates, deferred) = Self::partition_assignments(assignments)?;
        Self::validate_update_allowed_values(&schema, &updates)?;

        // Pre-validate the literal SET assignments. Catches:
        //   - unknown columns (in literal or deferred SET targets)
        //   - explicit `SET col = NULL` on a NOT NULL column
        //   - type/length mismatches on literal values
        // Deferred-expression results are re-validated per row after eval in
        // `apply_single_row_update` and `prepare_bulk_update_files`.
        let (update_col_names, update_null_cols) = Self::collect_update_metadata(assignments);
        Self::validate_row_against_schema(
            &schema,
            &table_name,
            &update_col_names,
            &updates,
            &update_null_cols,
            false,
        )?;

        if let Ok(doogat_id) = extract_where_id(selection) {
            // `WHERE id = 'X'` fast path: if no row with that id exists in
            // the target table, fall through to `Affected(0)` to match
            // standard SQL no-match semantics (and the behavior of the
            // compound/IN bulk path). The check is scoped to the target
            // table so an id that exists under a different type doesn't
            // wrongly get mutated as this type. See #5.
            if !self.row_exists_in_table(&table_name, &doogat_id)? {
                return Ok(SqlResult::Affected(0));
            }
            return self.apply_single_row_update(
                &table_name,
                &schema,
                &doogat_id,
                &deferred,
                &mut updates,
            );
        }

        self.update_bulk_rows(&table_name, &schema, selection, &updates, &deferred)
    }

    /// Return whether a row with the given id exists in the materialized
    /// type table named `table_name`.
    ///
    /// Used by the `WHERE id = 'X'` fast paths in `handle_update` and
    /// `handle_delete` to distinguish "no such row in this table" (return
    /// `Affected(0)`) from "row exists, proceed with mutation". The caller
    /// must have already validated that `table_name` resolves to a real
    /// type table via `load_schema`. See #5.
    fn row_exists_in_table(&self, table_name: &str, doogat_id: &str) -> Result<bool> {
        let sql = format!("SELECT COUNT(*) > 0 FROM \"{table_name}\" WHERE id = ?1");
        self.index
            .sql_conn()
            .query_row(&sql, params![doogat_id], |row| row.get::<_, bool>(0))
            .map_err(|e| DoogatError::SqlEngine(format!("existence check failed: {e}")))
    }

    /// Walk an UPDATE's raw assignments to extract:
    /// - the ordered list of column names in the SET clause (for the
    ///   unknown-column check), and
    /// - the set of column names whose RHS is a SQL `NULL` literal (so the
    ///   validator can flag `SET col = NULL` on a NOT NULL column).
    ///
    /// We do this from the raw AST because `partition_assignments` collapses
    /// `Expr::Value(Null)` to "" via `expr_to_string`, losing the distinction.
    fn collect_update_metadata(
        assignments: &[sqlparser::ast::Assignment],
    ) -> (Vec<String>, BTreeSet<String>) {
        let mut col_names = Vec::with_capacity(assignments.len());
        let mut nulls = BTreeSet::new();
        for assignment in assignments {
            let col_name = match &assignment.target {
                AssignmentTarget::ColumnName(name) => name.to_string().to_lowercase(),
                AssignmentTarget::Tuple(names) => names
                    .iter()
                    .map(|n| n.to_string().to_lowercase())
                    .collect::<Vec<_>>()
                    .join("."),
            };
            if is_null_literal(&assignment.value) {
                nulls.insert(col_name.clone());
            }
            col_names.push(col_name);
        }
        (col_names, nulls)
    }

    fn partition_assignments(
        assignments: &[sqlparser::ast::Assignment],
    ) -> Result<PartitionedAssignments> {
        let mut updates: BTreeMap<String, String> = BTreeMap::new();
        let mut deferred: Vec<(String, String)> = Vec::new();
        for assignment in assignments {
            let col_name = match &assignment.target {
                AssignmentTarget::ColumnName(name) => name.to_string().to_lowercase(),
                AssignmentTarget::Tuple(names) => names
                    .iter()
                    .map(|n| n.to_string().to_lowercase())
                    .collect::<Vec<_>>()
                    .join("."),
            };
            if is_literal_expr(&assignment.value) {
                updates.insert(col_name, expr_to_string(&assignment.value)?);
            } else {
                deferred.push((col_name, value_to_sql(&assignment.value)?));
            }
        }
        Ok((updates, deferred))
    }

    fn update_bulk_rows(
        &mut self,
        table_name: &str,
        schema: &TableSchema,
        selection: &Option<Expr>,
        updates: &BTreeMap<String, String>,
        deferred: &[(String, String)],
    ) -> Result<SqlResult> {
        let matches = self.resolve_matching_ids(table_name, selection)?;
        if matches.is_empty() {
            return Ok(SqlResult::Affected(0));
        }

        let (files, per_row_updates) =
            self.prepare_bulk_update_files(table_name, schema, &matches, updates, deferred)?;

        self.commit_or_buffer_writes(&files, &format!("bulk update {table_name}"))?;

        for ((id, path), row_updates) in matches.iter().zip(per_row_updates.iter()) {
            let content = self.read_content(path)?;
            let reparsed = parser::parse(&content, path)?;
            self.update_indexes_atomically(schema, id, &reparsed, row_updates)?;
        }

        Ok(SqlResult::Affected(matches.len()))
    }

    fn prepare_bulk_update_files(
        &mut self,
        table_name: &str,
        schema: &TableSchema,
        matches: &[(String, String)],
        updates: &BTreeMap<String, String>,
        deferred: &[(String, String)],
    ) -> Result<BulkUpdateFiles> {
        let mut files = Vec::with_capacity(matches.len());
        let mut per_row_updates = Vec::with_capacity(matches.len());
        for (id, path) in matches {
            let mut row_updates = updates.clone();
            if !deferred.is_empty() {
                let mut eval_nulls = BTreeSet::new();
                Self::eval_deferred_expressions(
                    self.index.sql_conn(),
                    deferred,
                    table_name,
                    id,
                    &mut row_updates,
                    &mut eval_nulls,
                )?;
                Self::validate_update_allowed_values(schema, &row_updates)?;
                // Type/length + NOT NULL re-check for deferred-expression
                // results. eval_nulls catches expression-synthesized NULL
                // (PRD 00122 blind review C1).
                let merged_names: Vec<String> = row_updates.keys().cloned().collect();
                Self::validate_row_against_schema(
                    schema,
                    table_name,
                    &merged_names,
                    &row_updates,
                    &eval_nulls,
                    false,
                )?;
            }
            let content = self.read_content(path)?;
            let mut parsed = parser::parse(&content, path)?;
            apply_updates_to_doogat(&mut parsed, schema, &row_updates);
            if let Some(new_title) = recompute_template_title(
                self.index.sql_conn(),
                schema,
                table_name,
                id,
                &row_updates,
            )? {
                parsed.meta.title = Some(new_title);
            }
            files.push((path.clone(), parser::serialize(&parsed)));
            per_row_updates.push(row_updates);
        }
        Ok((files, per_row_updates))
    }

    fn commit_or_buffer_writes(&mut self, files: &[(String, String)], message: &str) -> Result<()> {
        if let Some(ref mut buf) = self.txn {
            for (path, content) in files {
                buf.writes.push(PendingWrite {
                    path: path.clone(),
                    content: content.clone(),
                });
            }
        } else {
            let file_refs: Vec<(&str, &str)> = files
                .iter()
                .map(|(p, c)| (p.as_str(), c.as_str()))
                .collect();
            self.repo.commit_files(&file_refs, message)?;
        }
        Ok(())
    }

    pub(in crate::sql_engine) fn handle_delete(&mut self, del: &sqlparser::ast::Delete) -> Result<SqlResult> {
        let from_tables = match &del.from {
            FromTable::WithFromKeyword(tables) | FromTable::WithoutKeyword(tables) => tables,
        };
        let table_name = from_tables
            .first()
            .map(|f| unquote_identifier(&f.relation.to_string()))
            .ok_or_else(|| DoogatError::SqlEngine("missing table in DELETE".into()))?;

        if let Some((type_name, col_name)) = self.resolve_junction_table(&table_name)? {
            let type_id_col = format!("{type_name}_id");
            let ref_id_col = format!("{col_name}_id");
            let (parent_id, target_id) =
                extract_junction_where(&del.selection, &type_id_col, &ref_id_col)?;
            return self.handle_junction_delete(&type_name, &col_name, &parent_id, &target_id);
        }

        let _schema = self.load_schema(&table_name)?;

        if let Ok(doogat_id) = extract_where_id(&del.selection) {
            // `WHERE id = 'X'` fast path: if no row with that id exists in
            // the target table, fall through to `Affected(0)` to match
            // standard SQL no-match semantics. See #5 and `handle_update`
            // for rationale.
            if !self.row_exists_in_table(&table_name, &doogat_id)? {
                return Ok(SqlResult::Affected(0));
            }
            return self.delete_single_row(&table_name, &doogat_id);
        }

        self.delete_bulk_rows(&table_name, &del.selection)
    }

    /// Resolve doogat ids and paths matching a WHERE clause via SQLite.
    /// When `selection` is None, returns all rows of the table.
    pub(in crate::sql_engine) fn resolve_matching_ids(
        &mut self,
        table_name: &str,
        selection: &Option<Expr>,
    ) -> Result<Vec<(String, String)>> {
        let (sql, where_clause) = match selection {
            Some(expr) => {
                let clause = format!("{expr}");
                (
                    format!("SELECT id FROM \"{table_name}\" WHERE {clause}"),
                    Some(clause),
                )
            }
            None => (format!("SELECT id FROM \"{table_name}\""), None),
        };

        let mut stmt = self.index.sql_conn().prepare(&sql).map_err(|e| {
            DoogatError::SqlEngine(format!(
                "invalid WHERE clause{}: {e}",
                where_clause
                    .as_deref()
                    .map(|c| format!(" ({c})"))
                    .unwrap_or_default()
            ))
        })?;
        let ids: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| DoogatError::SqlEngine(format!("query failed: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        let mut result = Vec::with_capacity(ids.len());
        for id in ids {
            let path = self.index.resolve_path(&id)?;
            result.push((id, path));
        }
        Ok(result)
    }

    /// Coerce BOOLEAN columns in SELECT results from "1"/"0" to "true"/"false".
    /// Also returns column type metadata when a schema is available.
    pub(in crate::sql_engine) fn coerce_boolean_columns(
        &mut self,
        stmt: &Statement,
        columns: &[String],
        mut rows: Vec<Vec<String>>,
    ) -> (Vec<Vec<String>>, Option<Vec<String>>) {
        let table_name = match extract_from_table(stmt) {
            Some(t) => t,
            None => return (rows, None),
        };
        let schema = match self.load_schema(&table_name) {
            Ok(s) => s,
            Err(_) => return (rows, None),
        };

        // Build column type list and boolean indices
        let mut col_types = Vec::with_capacity(columns.len());
        let mut bool_indices = Vec::new();
        for (i, col_name) in columns.iter().enumerate() {
            let schema_col = schema
                .columns
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(col_name));
            let dtype = schema_col
                .map(|c| c.data_type.clone())
                .unwrap_or_else(|| "TEXT".to_string());
            if dtype.eq_ignore_ascii_case("BOOLEAN") {
                bool_indices.push(i);
            }
            col_types.push(dtype);
        }

        for row in &mut rows {
            for &idx in &bool_indices {
                if idx < row.len() {
                    match row[idx].as_str() {
                        "1" => row[idx] = "true".to_string(),
                        "0" => row[idx] = "false".to_string(),
                        _ => {}
                    }
                }
            }
        }
        (rows, Some(col_types))
    }

    /// Validate allowed_values constraints for UPDATE assignments.
    fn validate_update_allowed_values(
        schema: &TableSchema,
        updates: &BTreeMap<String, String>,
    ) -> Result<()> {
        for col_def in &schema.columns {
            let allowed = match col_def.allowed_values {
                Some(ref a) => a,
                None => continue,
            };
            let val = match updates.get(&col_def.name) {
                Some(v) if !v.is_empty() => v,
                _ => continue,
            };
            if !allowed.contains(val) {
                return Err(DoogatError::Validation(format!(
                    "column '{}': value '{}' not in allowed values {:?}",
                    col_def.name, val, allowed
                )));
            }
        }
        Ok(())
    }

    /// Evaluate deferred SQL expressions (COALESCE, IFNULL, etc.) for a
    /// specific row, populating `updates` with the resolved string values
    /// and `eval_nulls` with the column names whose evaluation produced
    /// SQL NULL. Callers re-run the row validator using `eval_nulls` so an
    /// expression that synthesizes NULL on a NOT NULL column is rejected
    /// (PRD 00122 blind review C1).
    fn eval_deferred_expressions(
        conn: &rusqlite::Connection,
        deferred: &[(String, String)],
        table_name: &str,
        doogat_id: &str,
        updates: &mut BTreeMap<String, String>,
        eval_nulls: &mut BTreeSet<String>,
    ) -> Result<()> {
        for (col, sql) in deferred {
            let eval_sql = format!("SELECT {sql} FROM \"{table_name}\" WHERE id = ?1");
            let result: rusqlite::types::Value = conn
                .query_row(&eval_sql, rusqlite::params![doogat_id], |row| row.get(0))
                .map_err(|e| DoogatError::SqlEngine(format!("expression eval failed: {e}")))?;
            match sqlite_value_to_string_nullable(result)? {
                Some(s) => {
                    updates.insert(col.clone(), s);
                }
                None => {
                    eval_nulls.insert(col.clone());
                    updates.insert(col.clone(), String::new());
                }
            }
        }
        Ok(())
    }

    /// Apply an UPDATE to a single row (fast path when WHERE id = '...').
    fn apply_single_row_update(
        &mut self,
        table_name: &str,
        schema: &TableSchema,
        doogat_id: &str,
        deferred: &[(String, String)],
        updates: &mut BTreeMap<String, String>,
    ) -> Result<SqlResult> {
        if !deferred.is_empty() {
            let mut eval_nulls = BTreeSet::new();
            Self::eval_deferred_expressions(
                self.index.sql_conn(),
                deferred,
                table_name,
                doogat_id,
                updates,
                &mut eval_nulls,
            )?;
            Self::validate_update_allowed_values(schema, updates)?;
            // Re-validate so deferred-expression results get type/length
            // checked AND so an expression that synthesized NULL (e.g.
            // `SET title = COALESCE(NULL, NULL)`) is caught against a
            // NOT NULL column.
            let merged_names: Vec<String> = updates.keys().cloned().collect();
            Self::validate_row_against_schema(
                schema,
                table_name,
                &merged_names,
                updates,
                &eval_nulls,
                false,
            )?;
        }
        let path = self.index.resolve_path(doogat_id)?;
        let content = self.read_content(&path)?;
        let mut parsed = parser::parse(&content, &path)?;
        apply_updates_to_doogat(&mut parsed, schema, updates);
        if let Some(new_title) = recompute_template_title(
            self.index.sql_conn(),
            schema,
            table_name,
            doogat_id,
            updates,
        )? {
            parsed.meta.title = Some(new_title);
        }
        let new_content = parser::serialize(&parsed);
        if let Some(ref mut buf) = self.txn {
            buf.writes.push(PendingWrite {
                path: path.clone(),
                content: new_content.clone(),
            });
        } else {
            self.repo.commit_file(
                &path,
                &new_content,
                &format!("update {table_name} {doogat_id}"),
            )?;
        }
        let reparsed = parser::parse(&new_content, &path)?;
        self.update_indexes_atomically(schema, doogat_id, &reparsed, updates)?;
        Ok(SqlResult::Affected(1))
    }

    /// Apply the SQL-side index writes for a typed UPDATE inside a SAVEPOINT
    /// so the doogats/typed-table/junction trio stays consistent if any step
    /// fails. Mirrors the INSERT atomicity pattern in `build_and_index_row`
    /// (PRD 00134 cycle-1 review C1).
    ///
    /// The git write happens before this call (see `apply_single_row_update`
    /// and `update_bulk_rows`); a sync-side failure here is reconcilable on
    /// next index rebuild, but a half-applied SQL state is not, so the
    /// SAVEPOINT scope is exactly the three SQL writes:
    /// `index_doogat` → `update_materialized_row` → `sync_junction_tables_for_columns`.
    ///
    /// SAVEPOINT semantics are safe in both call modes: when there is no
    /// outer transaction (autocommit, the single-row WHERE-id path), SQLite
    /// implicitly begins one for the SAVEPOINT and commits it on RELEASE;
    /// when an outer transaction is already open (`update_bulk_rows` may be
    /// invoked while `self.txn` is `Some` via `commit_or_buffer_writes`), the
    /// SAVEPOINT nests inside it and RELEASE just merges the inner work into
    /// the outer transaction without auto-committing. Either way, ROLLBACK TO
    /// undoes only the three SQL writes scoped above.
    fn update_indexes_atomically(
        &mut self,
        schema: &TableSchema,
        doogat_id: &str,
        reparsed: &crate::types::ParsedDoogat,
        updates: &BTreeMap<String, String>,
    ) -> Result<()> {
        self.index
            .sql_conn()
            .execute("SAVEPOINT update_row", [])
            .map_err(|e| DoogatError::SqlEngine(e.to_string()))?;

        let changed_cols: Vec<&str> = updates.keys().map(String::as_str).collect();
        let write_result = self
            .index
            .index_doogat(reparsed)
            .and_then(|()| self.update_materialized_row(schema, doogat_id, updates))
            .and_then(|()| {
                self.index.sync_junction_tables_for_columns(
                    schema,
                    doogat_id,
                    reparsed,
                    &changed_cols,
                )
            });

        if let Err(e) = write_result {
            // Best-effort rollback. If these fail the savepoint stack is
            // already in trouble; propagate the original error either way.
            if let Err(rb_err) = self.index.sql_conn().execute("ROLLBACK TO update_row", []) {
                tracing::warn!(error = %rb_err, "failed to rollback update_row savepoint");
            }
            if let Err(rl_err) = self.index.sql_conn().execute("RELEASE update_row", []) {
                tracing::warn!(error = %rl_err, "failed to release update_row savepoint");
            }
            return Err(e);
        }

        self.index
            .sql_conn()
            .execute("RELEASE update_row", [])
            .map_err(|e| DoogatError::SqlEngine(e.to_string()))?;

        Ok(())
    }
}
