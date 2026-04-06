use rusqlite::params;
use sqlparser::ast::{AssignmentTarget, Expr, FromTable, SetExpr, Statement};
use std::collections::BTreeMap;

use crate::error::{DoogatError, Result};
use crate::indexer::materialize::{is_core_column, normalize_bool_str};
use crate::parser;
use crate::types::TableSchema;

use super::builders::{apply_updates_to_doogat, build_data_doogat};
use super::helpers::{
    eval_values, expr_to_string, extract_from_table, extract_junction_where, extract_where_id,
    is_literal_expr, sqlite_value_to_string, unquote_identifier, value_to_sql,
};
use super::{PendingDelete, PendingWrite, SqlEngine, SqlResult};

impl<'a> SqlEngine<'a> {
    pub(super) fn handle_insert(
        &mut self,
        ins: &sqlparser::ast::Insert,
    ) -> Result<SqlResult> {
        // Reject REPLACE/UPSERT variants that bypass git
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
        let on_conflict_ignore =
            if let Some(ref on_conflict) = ins.on {
                use sqlparser::ast::{OnConflictAction, OnInsert};
                match on_conflict {
                    OnInsert::OnConflict(oc) => match oc.action {
                        OnConflictAction::DoNothing => true,
                        _ => return Err(DoogatError::SqlEngine(
                            "ON CONFLICT DO UPDATE is not supported; only DO NOTHING is allowed"
                                .into(),
                        )),
                    },
                    _ => {
                        return Err(DoogatError::SqlEngine(
                            "INSERT OR REPLACE/UPSERT not supported: bypasses git storage".into(),
                        ))
                    }
                }
            } else {
                false
            };

        let table_name = unquote_identifier(&ins.table.to_string());

        // Check if this is a junction table INSERT
        if let Some((type_name, col_name)) = self.resolve_junction_table(&table_name)? {
            return self.handle_junction_insert(ins, &type_name, &col_name);
        }

        let schema = self.load_schema(&table_name)?;

        // Extract column names from INSERT
        let col_names: Vec<String> = ins.columns.iter().map(|c| c.value.to_lowercase()).collect();

        // Extract all row value sets
        let rows = match ins.source.as_ref() {
            Some(query) => match query.body.as_ref() {
                SetExpr::Values(v) => {
                    let mut rows = Vec::with_capacity(v.rows.len());
                    for row in &v.rows {
                        rows.push(eval_values(&self.index.conn, row)?);
                    }
                    rows
                }
                _ => {
                    return Err(DoogatError::SqlEngine(
                        "only VALUES clause supported".into(),
                    ))
                }
            },
            None => return Err(DoogatError::SqlEngine("missing VALUES clause".into())),
        };

        // When ON CONFLICT DO NOTHING, filter out rows that match an existing unique_together
        // constraint and capture the existing row IDs so we can return them alongside new IDs.
        let mut on_conflict_existing: Vec<Option<String>> = Vec::new();
        let rows: Vec<Vec<String>> = if on_conflict_ignore {
            if let Some(ref constraints) = schema.unique_together {
                on_conflict_existing = vec![None; rows.len()];
                let mut filtered = Vec::with_capacity(rows.len());
                'row: for (row_idx, row_values) in rows.into_iter().enumerate() {
                    for constraint_cols in constraints {
                        // Build WHERE clause for this constraint group
                        let where_clause: String = constraint_cols
                            .iter()
                            .map(|c| format!("\"{}\" = ?", c))
                            .collect::<Vec<_>>()
                            .join(" AND ");
                        let sql = format!(
                            "SELECT id FROM \"{}\" WHERE {}",
                            schema.table_name, where_clause
                        );
                        // Collect bind values for this constraint
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
                        if bind_vals.len() == constraint_cols.len() {
                            let existing_id: Option<String> = self
                                .index
                                .conn
                                .query_row(&sql, rusqlite::params_from_iter(bind_vals), |row| {
                                    row.get(0)
                                })
                                .ok();
                            if let Some(id) = existing_id {
                                on_conflict_existing[row_idx] = Some(id);
                                continue 'row;
                            }
                        }
                    }
                    filtered.push(row_values);
                }
                filtered
            } else {
                rows
            }
        } else {
            rows
        };

        // Generate all IDs upfront
        let ids = self.unique_ids(rows.len())?;

        // Collect which referenced types use folder storage (for path-qualified wikilinks)
        let ref_folder_types = self.ref_folder_types(&schema);

        let mut created_ids = Vec::with_capacity(rows.len());
        let mut files: Vec<(String, String)> = Vec::with_capacity(rows.len());

        // Pre-compute NEXT counters for auto-increment columns
        let mut next_counters: BTreeMap<String, i64> = BTreeMap::new();
        for col_def in &schema.columns {
            if let Some(ref dv) = col_def.default_value {
                if dv == "NEXT" {
                    let max_val: i64 = self
                        .index
                        .conn
                        .query_row(
                            &format!(
                                "SELECT COALESCE(MAX(\"{}\"), 0) FROM \"{}\"",
                                col_def.name, schema.table_name
                            ),
                            [],
                            |row| row.get(0),
                        )
                        .unwrap_or(0);
                    next_counters.insert(col_def.name.clone(), max_val);
                }
            }
        }

        for (row_values, id) in rows.iter().zip(ids.into_iter()) {
            if col_names.len() != row_values.len() {
                return Err(DoogatError::SqlEngine(
                    "column count doesn't match value count".into(),
                ));
            }

            // Build column->value map
            let mut col_values: BTreeMap<String, String> = BTreeMap::new();
            for (name, val) in col_names.iter().zip(row_values.iter()) {
                col_values.insert(name.clone(), val.clone());
            }

            // Fill default values for omitted columns
            for col_def in &schema.columns {
                if !col_values.contains_key(&col_def.name) {
                    if let Some(ref default) = col_def.default_value {
                        if default == "NEXT" {
                            let counter = next_counters.get_mut(&col_def.name).unwrap();
                            *counter += 1;
                            col_values.insert(col_def.name.clone(), counter.to_string());
                        } else if default.starts_with("NEXT(") && default.ends_with(')') {
                            let partition_col = &default[5..default.len() - 1];
                            let partition_val =
                                col_values.get(partition_col).cloned().unwrap_or_default();
                            let max_val: i64 = self
                                .index
                                .conn
                                .query_row(
                                    &format!(
                                        "SELECT COALESCE(MAX(\"{}\"), 0) FROM \"{}\" WHERE \"{}\" = ?1",
                                        col_def.name, schema.table_name, partition_col
                                    ),
                                    params![partition_val],
                                    |row| row.get(0),
                                )
                                .unwrap_or(0);
                            col_values.insert(col_def.name.clone(), (max_val + 1).to_string());
                        } else {
                            col_values.insert(col_def.name.clone(), default.clone());
                        }
                    }
                }
            }

            // Validate allowed_values constraints
            for col_def in &schema.columns {
                if let Some(ref allowed) = col_def.allowed_values {
                    if let Some(val) = col_values.get(&col_def.name) {
                        if !val.is_empty() && !allowed.contains(val) {
                            return Err(DoogatError::Validation(format!(
                                "column '{}': value '{}' not in allowed values {:?}",
                                col_def.name, val, allowed
                            )));
                        }
                    }
                }
            }

            // Validate FK references
            for col_def in &schema.columns {
                if let Some(ref _ref_table) = col_def.references {
                    if let Some(ref_id) = col_values.get(&col_def.name) {
                        if !ref_id.is_empty() {
                            let exists: bool = self
                                .index
                                .conn
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
                    }
                }
            }

            // Build doogat
            let doogat = build_data_doogat(&id, &schema, &col_values, &ref_folder_types);
            let content = parser::serialize(&doogat);
            let path = if table_name == "doogats" {
                format!("ddb/{}.md", id.0)
            } else {
                crate::git_ops::doogat_path(&id.0, Some(&table_name), schema.folder)
            };

            // Index the doogat
            let parsed = parser::parse(&content, &path)?;
            self.index.index_doogat(&parsed)?;

            // Insert into materialized table
            self.insert_materialized_row(&schema, &id.0, &col_values)?;

            if let Some(ref mut buf) = self.txn {
                buf.writes.push(PendingWrite { path, content });
            } else {
                files.push((path, content));
            }

            created_ids.push(id.0.clone());
        }

        // Commit all files in a single git commit (when not in transaction)
        if self.txn.is_none() && !files.is_empty() {
            let file_refs: Vec<(&str, &str)> = files
                .iter()
                .map(|(p, c)| (p.as_str(), c.as_str()))
                .collect();
            self.repo.commit_files(
                &file_refs,
                &format!("insert {} row(s) into {table_name}", created_ids.len()),
            )?;
        }

        if on_conflict_ignore && !on_conflict_existing.is_empty() {
            // Merge existing IDs (for skipped duplicates) with newly created IDs
            let mut created_iter = created_ids.into_iter();
            let all_ids: Vec<String> = on_conflict_existing
                .into_iter()
                .map(|slot| match slot {
                    Some(id) => id,
                    None => created_iter.next().unwrap_or_default(),
                })
                .collect();
            Ok(SqlResult::Ok(all_ids.join(",")))
        } else {
            Ok(SqlResult::Ok(created_ids.join(",")))
        }
    }

    pub(super) fn handle_update(
        &mut self,
        table: &sqlparser::ast::TableWithJoins,
        assignments: &[sqlparser::ast::Assignment],
        selection: &Option<Expr>,
    ) -> Result<SqlResult> {
        let table_name = unquote_identifier(&table.relation.to_string());
        let schema = self.load_schema(&table_name)?;

        // Build assignment map: literals evaluated now, complex expressions deferred
        let mut updates: BTreeMap<String, String> = BTreeMap::new();
        let mut deferred: Vec<(String, String)> = Vec::new(); // (col_name, sql_text)
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
                let val = expr_to_string(&assignment.value)?;
                updates.insert(col_name, val);
            } else {
                let sql = value_to_sql(&assignment.value)?;
                deferred.push((col_name, sql));
            }
        }

        let validate_allowed_values =
            |schema: &TableSchema, updates: &BTreeMap<String, String>| -> Result<()> {
                for col_def in &schema.columns {
                    if let Some(ref allowed) = col_def.allowed_values {
                        if let Some(val) = updates.get(&col_def.name) {
                            if !val.is_empty() && !allowed.contains(val) {
                                return Err(DoogatError::Validation(format!(
                                    "column '{}': value '{}' not in allowed values {:?}",
                                    col_def.name, val, allowed
                                )));
                            }
                        }
                    }
                }
                Ok(())
            };

        // Validate literal assignments early (fail fast before touching rows)
        validate_allowed_values(&schema, &updates)?;

        let eval_deferred = |conn: &rusqlite::Connection,
                             deferred: &[(String, String)],
                             table_name: &str,
                             doogat_id: &str,
                             updates: &mut BTreeMap<String, String>|
         -> Result<()> {
            for (col, sql) in deferred {
                let eval_sql = format!("SELECT {sql} FROM \"{table_name}\" WHERE id = ?1");
                let result: rusqlite::types::Value = conn
                    .query_row(&eval_sql, rusqlite::params![doogat_id], |row| row.get(0))
                    .map_err(|e| DoogatError::SqlEngine(format!("expression eval failed: {e}")))?;
                updates.insert(col.clone(), sqlite_value_to_string(result)?);
            }
            Ok(())
        };

        // Fast path: single-row WHERE id = '...'
        if let Ok(doogat_id) = extract_where_id(selection) {
            if !deferred.is_empty() {
                eval_deferred(
                    &self.index.conn,
                    &deferred,
                    &table_name,
                    &doogat_id,
                    &mut updates,
                )?;
                // Re-validate after deferred expressions are resolved
                validate_allowed_values(&schema, &updates)?;
            }
            let path = self.index.resolve_path(&doogat_id)?;
            let content = self.read_content(&path)?;
            let mut parsed = parser::parse(&content, &path)?;
            apply_updates_to_doogat(&mut parsed, &schema, &updates);
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
            self.update_materialized_row(&schema, &doogat_id, &updates)?;
            return Ok(SqlResult::Affected(1));
        }

        // Bulk path: resolve matching rows via SQLite
        let matches = self.resolve_matching_ids(&table_name, selection)?;
        if matches.is_empty() {
            return Ok(SqlResult::Affected(0));
        }

        let mut files: Vec<(String, String)> = Vec::with_capacity(matches.len());
        let mut per_row_updates: Vec<BTreeMap<String, String>> = Vec::with_capacity(matches.len());
        for (id, path) in &matches {
            let mut row_updates = updates.clone();
            if !deferred.is_empty() {
                eval_deferred(
                    &self.index.conn,
                    &deferred,
                    &table_name,
                    id,
                    &mut row_updates,
                )?;
                validate_allowed_values(&schema, &row_updates)?;
            }
            let content = self.read_content(path)?;
            let mut parsed = parser::parse(&content, path)?;
            apply_updates_to_doogat(&mut parsed, &schema, &row_updates);
            files.push((path.clone(), parser::serialize(&parsed)));
            per_row_updates.push(row_updates);
        }

        if let Some(ref mut buf) = self.txn {
            for (path, content) in &files {
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
            self.repo
                .commit_files(&file_refs, &format!("bulk update {table_name}"))?;
        }

        // Re-index and update materialized rows
        for ((id, path), row_updates) in matches.iter().zip(per_row_updates.iter()) {
            let content = self.read_content(path)?;
            let reparsed = parser::parse(&content, path)?;
            self.index.index_doogat(&reparsed)?;
            self.update_materialized_row(&schema, id, row_updates)?;
        }

        Ok(SqlResult::Affected(matches.len()))
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

        // Check if this is a junction table DELETE
        if let Some((type_name, col_name)) = self.resolve_junction_table(&table_name)? {
            let type_id_col = format!("{type_name}_id");
            let ref_id_col = format!("{col_name}_id");
            // Extract both IDs from WHERE clause: {type}_id = '...' AND {col}_id = '...'
            let (parent_id, target_id) =
                extract_junction_where(&del.selection, &type_id_col, &ref_id_col)?;
            return self.handle_junction_delete(&type_name, &col_name, &parent_id, &target_id);
        }

        let _schema = self.load_schema(&table_name)?;

        // Fast path: single-row WHERE id = '...'
        if let Ok(doogat_id) = extract_where_id(&del.selection) {
            let path = self.index.resolve_path(&doogat_id)?;
            self.index.remove_doogat(&doogat_id)?;
            self.index.conn.execute(
                &format!("DELETE FROM \"{}\" WHERE id = ?1", table_name),
                params![doogat_id],
            )?;
            self.cascade_junction_cleanup(&table_name, &doogat_id)?;
            let ref_edits = self.cascade_remove_dangling_references(&doogat_id, &path)?;
            if let Some(ref mut buf) = self.txn {
                buf.deletes.push(PendingDelete {
                    path: path.clone(),
                    doogat_id: doogat_id.clone(),
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
            return Ok(SqlResult::Affected(1));
        }

        // Bulk path: resolve matching rows via SQLite
        let matches = self.resolve_matching_ids(&table_name, &del.selection)?;
        if matches.is_empty() {
            return Ok(SqlResult::Affected(0));
        }

        let mut all_ref_edits: Vec<PendingWrite> = Vec::new();
        for (id, path) in &matches {
            self.index.remove_doogat(id)?;
            self.index.conn.execute(
                &format!("DELETE FROM \"{}\" WHERE id = ?1", table_name),
                params![id],
            )?;
            self.cascade_junction_cleanup(&table_name, id)?;
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

        let mut stmt = self.index.conn.prepare(&sql).map_err(|e| {
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

    fn cascade_junction_cleanup(&mut self, target_type: &str, deleted_id: &str) -> Result<()> {
        self.index
            .cascade_junction_cleanup(self.repo, target_type, deleted_id)
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
                continue;
            }
            parsed.reference_section = new_section;
            let new_content = parser::serialize(&parsed);

            // Re-index
            let re_parsed = parser::parse(&new_content, source_path)?;
            self.index.index_doogat(&re_parsed)?;

            // Rematerialize if typed
            if let Some(ref stype) = re_parsed.meta.doogat_type {
                if let Ok(schema) = self.load_schema(stype) {
                    self.index
                        .materialize_single(&schema, source_id, &re_parsed)?;
                }
            }

            edits.push(PendingWrite {
                path: source_path.clone(),
                content: new_content,
            });
        }
        Ok(edits)
    }

    fn insert_materialized_row(
        &mut self,
        schema: &TableSchema,
        id: &str,
        col_values: &BTreeMap<String, String>,
    ) -> Result<()> {
        // Fetch core fields from the doogats table (populated by index_doogat)
        let (title, date, updated_at): (Option<String>, Option<String>, Option<String>) = self
            .index
            .conn
            .query_row(
                "SELECT title, date, updated_at FROM doogats WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or((None, None, None));

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
        let mut vals: Vec<Option<String>> = vec![Some(id.to_string()), title, date, updated_at];

        let mut param_idx = 5;
        for col in &schema.columns {
            if is_core_column(&col.name) {
                continue;
            }
            col_names.push(format!("\"{}\"", col.name));
            placeholders.push(format!("?{}", param_idx));
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
        self.index.conn.execute(&sql, params.as_slice())?;
        Ok(())
    }

    fn update_materialized_row(
        &mut self,
        schema: &TableSchema,
        id: &str,
        updates: &BTreeMap<String, String>,
    ) -> Result<()> {
        let valid_cols: Vec<&String> = schema
            .columns
            .iter()
            .filter(|c| !is_core_column(&c.name))
            .map(|c| &c.name)
            .collect();
        let mut set_clauses = Vec::new();
        let mut vals: Vec<String> = Vec::new();

        // Refresh core columns from the doogats table
        if let Ok((title, date, updated_at)) = self.index.conn.query_row(
            "SELECT title, date, updated_at FROM doogats WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        ) {
            if let Some(t) = title {
                vals.push(t);
                set_clauses.push(format!("title = ?{}", vals.len()));
            }
            if let Some(d) = date {
                vals.push(d);
                set_clauses.push(format!("date = ?{}", vals.len()));
            }
            if let Some(u) = updated_at {
                vals.push(u);
                set_clauses.push(format!("updated_at = ?{}", vals.len()));
            }
        }

        for (col, val) in updates {
            if valid_cols.contains(&col) {
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
        self.index.conn.execute(&sql, params.as_slice())?;
        Ok(())
    }
}
