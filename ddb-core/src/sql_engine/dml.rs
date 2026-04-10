use rusqlite::params;
use sqlparser::ast::{AssignmentTarget, Expr, FromTable, SetExpr, Statement};
use std::collections::BTreeMap;

use crate::error::{DoogatError, Result};
use crate::indexer::materialize::{is_core_column, normalize_bool_str};
use crate::parser;
use crate::types::{DoogatId, TableSchema};

use super::builders::{apply_updates_to_doogat, build_data_doogat};
use super::helpers::{
    eval_values, expr_to_string, extract_from_table, extract_junction_where, extract_where_id,
    is_literal_expr, sqlite_value_to_string, unquote_identifier, value_to_sql,
};
use super::{PendingDelete, PendingWrite, SqlEngine, SqlResult};

/// Filtered rows paired with a parallel vec of existing IDs for conflict-skipped slots.
type ConflictFilterResult = (Vec<Vec<String>>, Vec<Option<String>>);

/// Partitioned UPDATE assignments: literal values and deferred SQL expressions.
type PartitionedAssignments = (BTreeMap<String, String>, Vec<(String, String)>);

/// Prepared bulk-update output: file contents and per-row update maps.
type BulkUpdateFiles = (Vec<(String, String)>, Vec<BTreeMap<String, String>>);

impl<'a> SqlEngine<'a> {
    pub(super) fn handle_insert(
        &mut self,
        ins: &sqlparser::ast::Insert,
    ) -> Result<SqlResult> {
        self.reject_insert_variants(ins)?;
        let on_conflict_ignore = self.parse_on_conflict(ins)?;

        let table_name = unquote_identifier(&ins.table.to_string());

        if let Some((type_name, col_name)) = self.resolve_junction_table(&table_name)? {
            return self.handle_junction_insert(ins, &type_name, &col_name);
        }

        let schema = self.load_schema(&table_name)?;
        let col_names: Vec<String> = ins.columns.iter().map(|c| c.value.to_lowercase()).collect();
        let rows = self.extract_insert_rows(ins)?;

        let (rows, on_conflict_existing) = if on_conflict_ignore {
            self.filter_conflict_rows(rows, &schema, &col_names)?
        } else {
            (rows, vec![])
        };

        let ids = self.unique_ids(rows.len())?;
        let ref_folder_types = self.ref_folder_types(&schema);
        let mut created_ids = Vec::with_capacity(rows.len());
        let mut files: Vec<(String, String)> = Vec::with_capacity(rows.len());
        let mut next_counters = self.precompute_insert_next_counters(&schema);

        for (row_values, id) in rows.iter().zip(ids.into_iter()) {
            if col_names.len() != row_values.len() {
                return Err(DoogatError::SqlEngine(
                    "column count doesn't match value count".into(),
                ));
            }
            let mut col_values: BTreeMap<String, String> = col_names
                .iter()
                .zip(row_values.iter())
                .map(|(n, v)| (n.clone(), v.clone()))
                .collect();
            self.fill_defaults_and_validate(&schema, &mut col_values, &mut next_counters)?;
            let (path, content) =
                self.build_and_index_row(&schema, &table_name, &id, &col_values, &ref_folder_types)?;
            self.buffer_or_collect_write(path, content, &mut files);
            created_ids.push(id.0.clone());
        }

        self.commit_insert_files(&files, &table_name, created_ids.len())?;
        self.merge_insert_results(
            on_conflict_ignore,
            on_conflict_existing,
            created_ids,
        )
    }

    fn reject_insert_variants(&self, ins: &sqlparser::ast::Insert) -> Result<()> {
        if ins.replace_into {
            return Err(DoogatError::SqlEngine(
                "REPLACE INTO not supported: bypasses git storage; use explicit DELETE + INSERT instead".into(),
            ));
        }
        if ins.or.is_some() {
            return Err(DoogatError::SqlEngine(
                "INSERT OR REPLACE/UPSERT not supported: bypasses git storage; use explicit INSERT + UPDATE instead".into(),
            ));
        }
        Ok(())
    }

    fn parse_on_conflict(&self, ins: &sqlparser::ast::Insert) -> Result<bool> {
        let on_conflict = match ins.on {
            Some(ref oc) => oc,
            None => return Ok(false),
        };
        use sqlparser::ast::{OnConflictAction, OnInsert};
        match on_conflict {
            OnInsert::OnConflict(oc) => match oc.action {
                OnConflictAction::DoNothing => Ok(true),
                _ => Err(DoogatError::SqlEngine(
                    "ON CONFLICT DO UPDATE is not supported; only DO NOTHING is allowed"
                        .into(),
                )),
            },
            _ => Err(DoogatError::SqlEngine(
                "INSERT OR REPLACE/UPSERT not supported: bypasses git storage"
                    .into(),
            )),
        }
    }

    fn extract_insert_rows(
        &self,
        ins: &sqlparser::ast::Insert,
    ) -> Result<Vec<Vec<String>>> {
        let query = ins.source.as_ref().ok_or_else(|| {
            DoogatError::SqlEngine("missing VALUES clause".into())
        })?;
        match query.body.as_ref() {
            SetExpr::Values(v) => {
                let mut rows = Vec::with_capacity(v.rows.len());
                for row in &v.rows {
                    rows.push(eval_values(self.index.sql_conn(), row)?);
                }
                Ok(rows)
            }
            _ => Err(DoogatError::SqlEngine(
                "only VALUES clause supported".into(),
            )),
        }
    }

    fn build_and_index_row(
        &mut self,
        schema: &TableSchema,
        table_name: &str,
        id: &DoogatId,
        col_values: &BTreeMap<String, String>,
        ref_folder_types: &std::collections::HashSet<String>,
    ) -> Result<(String, String)> {
        let doogat = build_data_doogat(id, schema, col_values, ref_folder_types);
        let content = parser::serialize(&doogat);
        let path = if table_name == "doogats" {
            format!("ddb/{}.md", id.0)
        } else {
            crate::git_ops::doogat_path(&id.0, Some(table_name), schema.folder)
        };
        let parsed = parser::parse(&content, &path)?;

        // Write the `doogats` index row and the materialized typed-table row
        // atomically. If `insert_materialized_row` fails (e.g. UNIQUE
        // constraint violation), rolling back the savepoint removes the index
        // row so no ghost entry is left behind. Without this, a client that
        // retries a failing INSERT would brick every subsequent mutation that
        // touches the `doogats` index. See
        // https://github.com/doogat/ddb/issues/4.
        self.index
            .sql_conn()
            .execute("SAVEPOINT insert_row", [])
            .map_err(|e| DoogatError::SqlEngine(e.to_string()))?;

        let write_result = self
            .index
            .index_doogat(&parsed)
            .and_then(|()| self.insert_materialized_row(schema, &id.0, col_values));

        if let Err(e) = write_result {
            // Best-effort rollback. If these fail the savepoint stack is
            // already in trouble; propagate the original error either way.
            if let Err(rb_err) = self
                .index
                .sql_conn()
                .execute("ROLLBACK TO insert_row", [])
            {
                tracing::warn!(error = %rb_err, "failed to rollback insert_row savepoint");
            }
            if let Err(rl_err) = self.index.sql_conn().execute("RELEASE insert_row", []) {
                tracing::warn!(error = %rl_err, "failed to release insert_row savepoint");
            }
            return Err(e);
        }

        self.index
            .sql_conn()
            .execute("RELEASE insert_row", [])
            .map_err(|e| DoogatError::SqlEngine(e.to_string()))?;

        Ok((path, content))
    }

    fn buffer_or_collect_write(
        &mut self,
        path: String,
        content: String,
        files: &mut Vec<(String, String)>,
    ) {
        if let Some(ref mut buf) = self.txn {
            buf.writes.push(PendingWrite { path, content });
        } else {
            files.push((path, content));
        }
    }

    fn commit_insert_files(
        &mut self,
        files: &[(String, String)],
        table_name: &str,
        count: usize,
    ) -> Result<()> {
        if self.txn.is_some() || files.is_empty() {
            return Ok(());
        }
        let file_refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        self.repo.commit_files(
            &file_refs,
            &format!("insert {count} row(s) into {table_name}"),
        )?;
        Ok(())
    }

    fn merge_insert_results(
        &self,
        on_conflict_ignore: bool,
        on_conflict_existing: Vec<Option<String>>,
        created_ids: Vec<String>,
    ) -> Result<SqlResult> {
        if !on_conflict_ignore || on_conflict_existing.is_empty() {
            return Ok(SqlResult::Ok(created_ids.join(",")));
        }
        let mut created_iter = created_ids.into_iter();
        let all_ids: Vec<String> = on_conflict_existing
            .into_iter()
            .map(|slot| match slot {
                Some(id) => id,
                None => created_iter.next().unwrap_or_default(),
            })
            .collect();
        Ok(SqlResult::Ok(all_ids.join(",")))
    }

    pub(super) fn handle_update(
        &mut self,
        table: &sqlparser::ast::TableWithJoins,
        assignments: &[sqlparser::ast::Assignment],
        selection: &Option<Expr>,
    ) -> Result<SqlResult> {
        let table_name = unquote_identifier(&table.relation.to_string());
        let schema = self.load_schema(&table_name)?;

        let (mut updates, deferred) = Self::partition_assignments(assignments)?;
        Self::validate_update_allowed_values(&schema, &updates)?;

        if let Ok(doogat_id) = extract_where_id(selection) {
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
            self.index.index_doogat(&reparsed)?;
            self.update_materialized_row(schema, id, row_updates)?;
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
                Self::eval_deferred_expressions(
                    self.index.sql_conn(), deferred, table_name, id, &mut row_updates,
                )?;
                Self::validate_update_allowed_values(schema, &row_updates)?;
            }
            let content = self.read_content(path)?;
            let mut parsed = parser::parse(&content, path)?;
            apply_updates_to_doogat(&mut parsed, schema, &row_updates);
            files.push((path.clone(), parser::serialize(&parsed)));
            per_row_updates.push(row_updates);
        }
        Ok((files, per_row_updates))
    }

    fn commit_or_buffer_writes(
        &mut self,
        files: &[(String, String)],
        message: &str,
    ) -> Result<()> {
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

    pub(super) fn handle_delete(
        &mut self,
        del: &sqlparser::ast::Delete,
    ) -> Result<SqlResult> {
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
            return self.delete_single_row(&table_name, &doogat_id);
        }

        self.delete_bulk_rows(&table_name, &del.selection)
    }

    fn delete_single_row(
        &mut self,
        table_name: &str,
        doogat_id: &str,
    ) -> Result<SqlResult> {
        let path = self.index.resolve_path(doogat_id)?;
        self.index.remove_doogat(doogat_id)?;
        self.index.sql_conn().execute(
            &format!("DELETE FROM \"{}\" WHERE id = ?1", table_name),
            params![doogat_id],
        )?;
        self.cascade_junction_cleanup(table_name, doogat_id)?;
        let ref_edits = self.cascade_remove_dangling_references(doogat_id, &path)?;
        if let Some(ref mut buf) = self.txn {
            buf.deletes.push(PendingDelete {
                path: path.clone(),
                doogat_id: doogat_id.to_string(),
            });
            buf.writes.extend(ref_edits);
        } else {
            let writes: Vec<(&str, &str)> = ref_edits
                .iter()
                .map(|w| (w.path.as_str(), w.content.as_str()))
                .collect();
            self.repo.commit_batch(
                &writes,
                &[&path],
                &format!("delete from {table_name} {doogat_id}"),
            )?;
        }
        Ok(SqlResult::Affected(1))
    }

    fn delete_bulk_rows(
        &mut self,
        table_name: &str,
        selection: &Option<Expr>,
    ) -> Result<SqlResult> {
        let matches = self.resolve_matching_ids(table_name, selection)?;
        if matches.is_empty() {
            return Ok(SqlResult::Affected(0));
        }

        let mut all_ref_edits: Vec<PendingWrite> = Vec::new();
        for (id, path) in &matches {
            self.index.remove_doogat(id)?;
            self.index.sql_conn().execute(
                &format!("DELETE FROM \"{}\" WHERE id = ?1", table_name),
                params![id],
            )?;
            self.cascade_junction_cleanup(table_name, id)?;
            all_ref_edits.extend(self.cascade_remove_dangling_references(id, path)?);
        }

        if let Some(ref mut buf) = self.txn {
            for (id, path) in &matches {
                buf.deletes.push(PendingDelete {
                    path: path.clone(),
                    doogat_id: id.clone(),
                });
            }
            buf.writes.extend(all_ref_edits);
        } else {
            let delete_paths: Vec<&str> = matches.iter().map(|(_, p)| p.as_str()).collect();
            let writes: Vec<(&str, &str)> = all_ref_edits
                .iter()
                .map(|w| (w.path.as_str(), w.content.as_str()))
                .collect();
            self.repo.commit_batch(
                &writes,
                &delete_paths,
                &format!("bulk delete from {table_name}"),
            )?;
        }

        Ok(SqlResult::Affected(matches.len()))
    }

    /// Resolve doogat ids and paths matching a WHERE clause via SQLite.
    /// When `selection` is None, returns all rows of the table.
    pub(super) fn resolve_matching_ids(
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
    pub(super) fn coerce_boolean_columns(
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

    /// Evaluate deferred SQL expressions (COALESCE, IFNULL, etc.) for a specific row.
    fn eval_deferred_expressions(
        conn: &rusqlite::Connection,
        deferred: &[(String, String)],
        table_name: &str,
        doogat_id: &str,
        updates: &mut BTreeMap<String, String>,
    ) -> Result<()> {
        for (col, sql) in deferred {
            let eval_sql = format!("SELECT {sql} FROM \"{table_name}\" WHERE id = ?1");
            let result: rusqlite::types::Value = conn
                .query_row(&eval_sql, rusqlite::params![doogat_id], |row| row.get(0))
                .map_err(|e| DoogatError::SqlEngine(format!("expression eval failed: {e}")))?;
            updates.insert(col.clone(), sqlite_value_to_string(result)?);
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
            Self::eval_deferred_expressions(
                self.index.sql_conn(),
                deferred,
                table_name,
                doogat_id,
                updates,
            )?;
            Self::validate_update_allowed_values(schema, updates)?;
        }
        let path = self.index.resolve_path(doogat_id)?;
        let content = self.read_content(&path)?;
        let mut parsed = parser::parse(&content, &path)?;
        apply_updates_to_doogat(&mut parsed, schema, updates);
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
        self.index.index_doogat(&reparsed)?;
        self.update_materialized_row(schema, doogat_id, updates)?;
        Ok(SqlResult::Affected(1))
    }

    /// Pre-compute bare NEXT counters for auto-increment columns.
    fn precompute_insert_next_counters(
        &self,
        schema: &TableSchema,
    ) -> BTreeMap<String, i64> {
        let mut counters = BTreeMap::new();
        for col_def in &schema.columns {
            if col_def.default_value.as_deref() != Some("NEXT") {
                continue;
            }
            let max_val: i64 = self
                .index
                .sql_conn()
                .query_row(
                    &format!(
                        "SELECT COALESCE(MAX(\"{}\"), 0) FROM \"{}\"",
                        col_def.name, schema.table_name
                    ),
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            counters.insert(col_def.name.clone(), max_val);
        }
        counters
    }

    /// Fill default values and validate constraints for a single INSERT row.
    fn fill_defaults_and_validate(
        &self,
        schema: &TableSchema,
        col_values: &mut BTreeMap<String, String>,
        next_counters: &mut BTreeMap<String, i64>,
    ) -> Result<()> {
        self.fill_column_defaults(schema, col_values, next_counters)?;
        self.validate_insert_constraints(schema, col_values)
    }

    fn fill_column_defaults(
        &self,
        schema: &TableSchema,
        col_values: &mut BTreeMap<String, String>,
        next_counters: &mut BTreeMap<String, i64>,
    ) -> Result<()> {
        for col_def in &schema.columns {
            if col_values.contains_key(&col_def.name) {
                continue;
            }
            let default = match col_def.default_value {
                Some(ref d) => d,
                None => continue,
            };
            let value = self.resolve_insert_default(default, col_def, schema, col_values, next_counters);
            col_values.insert(col_def.name.clone(), value);
        }
        Ok(())
    }

    /// Resolve a single column default for an INSERT row.
    fn resolve_insert_default(
        &self,
        default: &str,
        col_def: &crate::types::ColumnDef,
        schema: &TableSchema,
        col_values: &BTreeMap<String, String>,
        next_counters: &mut BTreeMap<String, i64>,
    ) -> String {
        if default == "NEXT" {
            let counter = next_counters
                .get_mut(&col_def.name)
                .expect("key pre-populated above");
            *counter += 1;
            return counter.to_string();
        }
        if default.starts_with("NEXT(") && default.ends_with(')') {
            let partition_col = &default[5..default.len() - 1];
            let partition_val = col_values.get(partition_col).cloned().unwrap_or_default();
            let max_val: i64 = self
                .index
                .sql_conn()
                .query_row(
                    &format!(
                        "SELECT COALESCE(MAX(\"{}\"), 0) FROM \"{}\" WHERE \"{}\" = ?1",
                        col_def.name, schema.table_name, partition_col
                    ),
                    params![partition_val],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            return (max_val + 1).to_string();
        }
        default.to_owned()
    }

    fn validate_insert_constraints(
        &self,
        schema: &TableSchema,
        col_values: &BTreeMap<String, String>,
    ) -> Result<()> {
        for col_def in &schema.columns {
            let allowed = match col_def.allowed_values {
                Some(ref a) => a,
                None => continue,
            };
            let val = match col_values.get(&col_def.name) {
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

        for col_def in &schema.columns {
            if col_def.references.is_none() {
                continue;
            }
            let ref_id = match col_values.get(&col_def.name) {
                Some(v) if !v.is_empty() => v,
                _ => continue,
            };
            let exists: bool = self
                .index
                .sql_conn()
                .query_row(
                    "SELECT COUNT(*) > 0 FROM doogats WHERE id = ?1",
                    params![ref_id],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if !exists {
                return Err(DoogatError::SqlEngine(format!(
                    "referenced doogat not found: {}",
                    ref_id
                )));
            }
        }

        Ok(())
    }

    /// Filter out rows that match existing unique_together constraints when
    /// ON CONFLICT DO NOTHING is active. Returns filtered rows and a parallel
    /// vec mapping original row indices to existing IDs (Some) or new rows (None).
    fn filter_conflict_rows(
        &self,
        rows: Vec<Vec<String>>,
        schema: &TableSchema,
        col_names: &[String],
    ) -> Result<ConflictFilterResult> {
        let constraints = match schema.unique_together {
            Some(ref c) => c,
            None => return Ok((rows, vec![])),
        };

        let mut existing: Vec<Option<String>> = vec![None; rows.len()];
        let mut filtered = Vec::with_capacity(rows.len());

        for (row_idx, row_values) in rows.into_iter().enumerate() {
            if let Some(id) = self.find_conflict_match(schema, constraints, col_names, &row_values)
            {
                existing[row_idx] = Some(id);
            } else {
                filtered.push(row_values);
            }
        }

        Ok((filtered, existing))
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

    fn cascade_junction_cleanup(&mut self, target_type: &str, deleted_id: &str) -> Result<()> {
        let conn = self.index.sql_conn();
        let mut stmt = conn
            .prepare("SELECT title FROM doogats WHERE type = '_typedef'")
            .map_err(|e| DoogatError::SqlEngine(format!("cascade junction query: {e}")))?;
        let type_names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| DoogatError::SqlEngine(format!("cascade junction query: {e}")))?
            .filter_map(|r| {
                r.map_err(|e| tracing::warn!(error = %e, "cascade junction: failed to read typedef row"))
                    .ok()
            })
            .collect();
        drop(stmt);

        for table_name in &type_names {
            let schema = match self.load_schema(table_name) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(type_name = %table_name, error = %e, "cascade junction: failed to load schema");
                    continue;
                }
            };
            for col in &schema.columns {
                if col.references.as_deref() == Some(target_type) {
                    let jt = format!("{table_name}_{}", col.name);
                    let col_id = format!("{}_id", col.name);
                    self.index.sql_conn().execute(
                        &format!("DELETE FROM \"{jt}\" WHERE \"{col_id}\" = ?1"),
                        params![deleted_id],
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Remove wikilinks to `deleted_id` from the reference sections of all
    /// doogats that link to it.  Returns the edited files; caller is
    /// responsible for committing or buffering them.
    fn cascade_remove_dangling_references(
        &mut self,
        deleted_id: &str,
        deleted_path: &str,
    ) -> Result<Vec<PendingWrite>> {
        let sources = self.index.backlinks_by_target(deleted_id, deleted_path)?;
        let mut edits = Vec::new();
        for (source_id, source_path) in &sources {
            if let Some(write) =
                self.strip_dangling_ref(source_id, source_path, deleted_id, deleted_path)?
            {
                edits.push(write);
            }
        }
        Ok(edits)
    }

    fn strip_dangling_ref(
        &mut self,
        source_id: &str,
        source_path: &str,
        deleted_id: &str,
        deleted_path: &str,
    ) -> Result<Option<PendingWrite>> {
        let content = self.read_content(source_path)?;
        let mut parsed = parser::parse(&content, source_path)?;

        let old_section = parsed.reference_section.clone();
        let new_lines: Vec<&str> = old_section
            .lines()
            .filter(|line| {
                !line.contains(&format!("[[{deleted_id}]]"))
                    && !line.contains(&format!("[[{deleted_path}]]"))
            })
            .collect();
        let new_section = if new_lines.is_empty() {
            String::new()
        } else {
            format!("{}\n", new_lines.join("\n"))
        };

        if new_section == old_section {
            return Ok(None);
        }
        parsed.reference_section = new_section;
        let new_content = parser::serialize(&parsed);

        let re_parsed = parser::parse(&new_content, source_path)?;
        self.index.index_doogat(&re_parsed)?;

        if let Some(ref stype) = re_parsed.meta.doogat_type {
            if let Ok(schema) = self.load_schema(stype) {
                self.index
                    .materialize_single(&schema, source_id, &re_parsed)?;
            }
        }

        Ok(Some(PendingWrite {
            path: source_path.to_string(),
            content: new_content,
        }))
    }

    fn insert_materialized_row(
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

        let mut vals: Vec<Option<String>> =
            vec![Some(id.to_string()), title, date, updated_at];
        let (col_names, placeholders) =
            build_insert_columns(schema, col_values, &mut vals);

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
            .map_err(|e| DoogatError::SqlEngine(e.to_string()))?;
        Ok(())
    }

    fn update_materialized_row(
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
