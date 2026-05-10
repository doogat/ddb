use rusqlite::params;
use sqlparser::ast::{AssignmentTarget, Expr, FromTable, SetExpr, Statement};
use std::collections::{BTreeMap, BTreeSet};

use crate::error::{DoogatError, Result};
use crate::indexer::materialize::{is_core_column, normalize_bool_str};
use crate::parser;
use crate::types::{DoogatId, TableSchema};

use super::builders::{apply_updates_to_doogat, build_data_doogat, recompute_template_title};
use super::helpers::{
    eval_values_nullable, expr_to_string, extract_from_table, extract_junction_where,
    extract_where_id, is_literal_expr, sqlite_value_to_string_nullable, unquote_identifier,
    value_to_sql,
};
use super::typed_insert::{prepare_typed_insert_validate, TypedInsertCounters};
use super::{PendingDelete, PendingWrite, SqlEngine, SqlResult};

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
fn type_check_value(
    data_type: &str,
    table_name: &str,
    col_name: &str,
    val: &str,
) -> Result<()> {
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

/// Raw rows + parallel null-column sets returned by `extract_insert_rows`.
type ExtractedInsertRows = (Vec<Vec<String>>, NullColsPerRow);

/// Filtered rows, parallel null-column sets, and a vec of existing IDs for
/// conflict-skipped slots.
type ConflictFilterResult = (Vec<Vec<String>>, NullColsPerRow, Vec<Option<String>>);

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
        let (rows, null_cols_per_row) = self.extract_insert_rows(ins, &col_names)?;

        let (rows, null_cols_per_row, on_conflict_existing) = if on_conflict_ignore {
            self.filter_conflict_rows(rows, null_cols_per_row, &schema, &col_names)?
        } else {
            (rows, null_cols_per_row, vec![])
        };

        // PRD 00139 §3 layer 2 + §4: singleton pre-check before Pass 1.
        // (a) If the table already holds a row, reject any further INSERT
        //     immediately with the existing row's id in the structured
        //     context. The materializer-side singleton_lock index from T8
        //     would also catch this at write time, but failing in Pass 1
        //     keeps pre-validate-all-then-write semantics intact for
        //     multi-row INSERTs and surfaces the structured `existing_id`.
        // (b) Multi-row INSERT into an empty SINGLETON typedef: reject the
        //     second row before any commit, with the `<intra-batch>`
        //     marker mirroring the service-layer batch tracker (T10).
        if schema.singleton && !rows.is_empty() {
            let existing_id: Option<String> = self
                .index
                .sql_conn()
                .query_row(
                    &format!("SELECT id FROM \"{table_name}\" LIMIT 1"),
                    [],
                    |row| row.get(0),
                )
                .ok();
            if let Some(id) = existing_id {
                if !on_conflict_ignore {
                    return Err(DoogatError::singleton_violation(
                        table_name.clone(),
                        id,
                    ));
                }
            } else if rows.len() > 1 {
                // Empty table + multi-row INSERT: second row collides
                // with the first within the same statement.
                return Err(DoogatError::singleton_violation(
                    table_name.clone(),
                    "<intra-batch>".to_string(),
                ));
            }
        }

        let ids = self.unique_ids(rows.len())?;
        let ref_folder_types = self.ref_folder_types(&schema);
        let mut created_ids = Vec::with_capacity(rows.len());
        let mut files: Vec<(String, String)> = Vec::with_capacity(rows.len());
        let mut next_counters = TypedInsertCounters::default();

        // Pass 1: validate every row before any side effects. PRD 00122
        // blind review C2 — without pre-validation, a multi-row INSERT
        // whose nth row fails validation would have already committed rows
        // 1..n-1 to the index and materialized table (per-row savepoints
        // are released individually), leaving the store inconsistent
        // because the git commit at the end is skipped.
        let mut prepared_rows: Vec<BTreeMap<String, String>> = Vec::with_capacity(rows.len());
        for (row_values, null_cols) in rows.iter().zip(null_cols_per_row.iter()) {
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
            prepare_typed_insert_validate(
                &schema,
                &mut col_values,
                &mut next_counters,
                self.index.sql_conn(),
            )?;
            Self::validate_row_against_schema(
                &schema,
                &table_name,
                &col_names,
                &col_values,
                null_cols,
                true,
            )?;
            prepared_rows.push(col_values);
        }

        // Pass 2: write each pre-validated row. The per-row SAVEPOINT in
        // `build_and_index_row` still handles UNIQUE collisions (those
        // happen at write time, not pre-check time), so a single-row
        // failure here still rolls back its own row cleanly.
        for (col_values, id) in prepared_rows.into_iter().zip(ids) {
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
        col_names: &[String],
    ) -> Result<ExtractedInsertRows> {
        let query = ins.source.as_ref().ok_or_else(|| {
            DoogatError::SqlEngine("missing VALUES clause".into())
        })?;
        match query.body.as_ref() {
            SetExpr::Values(v) => {
                let mut rows = Vec::with_capacity(v.rows.len());
                let mut null_cols_per_row = Vec::with_capacity(v.rows.len());
                for row in &v.rows {
                    let values = eval_values_nullable(self.index.sql_conn(), row)?;
                    let mut row_strings = Vec::with_capacity(values.len());
                    let mut row_nulls = BTreeSet::new();
                    for (idx, value) in values.into_iter().enumerate() {
                        match value {
                            Some(s) => row_strings.push(s),
                            None => {
                                if let Some(name) = col_names.get(idx) {
                                    row_nulls.insert(name.clone());
                                }
                                row_strings.push(String::new());
                            }
                        }
                    }
                    rows.push(row_strings);
                    null_cols_per_row.push(row_nulls);
                }
                Ok((rows, null_cols_per_row))
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
        let doogat =
            build_data_doogat(id, schema, col_values, ref_folder_types, Some(self.index.sql_conn()));
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
            .and_then(|()| self.insert_materialized_row(schema, &id.0, col_values))
            .and_then(|()| self.index.populate_junction_tables(schema, &id.0, &parsed));

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

    fn delete_single_row(
        &mut self,
        table_name: &str,
        doogat_id: &str,
    ) -> Result<SqlResult> {
        let path = self.index.resolve_path(doogat_id)?;
        // RESTRICT: block the delete if any typed-table row holds this id in
        // a NOT NULL REFERENCES column (#10).
        self.index.check_restrict_blocks_delete(self.repo, doogat_id)?;
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

        // RESTRICT pre-pass: if any matched id has a NOT NULL REFERENCES
        // dependent, reject the whole bulk before touching state (#10).
        for (id, _) in &matches {
            self.index.check_restrict_blocks_delete(self.repo, id)?;
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
            if let Err(rb_err) = self
                .index
                .sql_conn()
                .execute("ROLLBACK TO update_row", [])
            {
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
    pub(super) fn validate_row_against_schema(
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
        let schema_cols: BTreeSet<&str> =
            schema.columns.iter().map(|c| c.name.as_str()).collect();
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
    fn filter_conflict_rows(
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

        for (row_idx, (row_values, nulls)) in rows
            .into_iter()
            .zip(null_cols_per_row)
            .enumerate()
        {
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

    /// Cascade-clean auto-junction rows when a typed row is deleted, in BOTH
    /// directions:
    ///
    /// 1. **Reverse** — junction rows in OTHER typedef tables that pointed at
    ///    `deleted_id` via `<col>_id` (e.g. deleting a `category` row removes
    ///    `bookmark_category WHERE category_id = '<cat>'`).
    /// 2. **Parent/owner** (PRD 00137) — junction rows OWNED by the deleted
    ///    row's typedef, where `<target_type>_id = '<deleted_id>'` (e.g.
    ///    deleting a `bookmark` row removes `bookmark_category WHERE
    ///    bookmark_id = '<bm>'`).
    ///
    /// Both branches share the outer `for table_name in &type_names` loop, but
    /// they fire on different conditions and at different rates:
    ///
    /// - The **reverse** branch fires on every typedef whose schema has a
    ///   column with `references == Some(target_type)` — generally a sparse
    ///   set, scaling with how many other types reference this one.
    /// - The **parent/owner** branch fires *exactly once per call*, on the
    ///   single iteration where `table_name == target_type`. It then iterates
    ///   that one schema's REFERENCES columns. Both branches can fire in the
    ///   same call when the deleted row's typedef is simultaneously a parent
    ///   and a child of other typedefs.
    ///
    /// Error handling is asymmetric to match these semantics: a failure to
    /// load `target_type`'s own schema is fatal (we can't enumerate the
    /// REFERENCES columns the parent owns, so silently skipping would drop
    /// owner-side rows), while failures on unrelated typedefs are warn-and-
    /// skip (one bad typedef shouldn't poison the whole reverse sweep).
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
                    // Asymmetric error handling by direction:
                    // - Parent/owner direction (`table_name == target_type`):
                    //   the target's own typedef is required to enumerate
                    //   REFERENCES columns; failing to load it would silently
                    //   drop owner-side cleanup, violating PRD 00137 §G2.
                    //   Propagate as an explicit error.
                    // - Reverse direction (other typedefs): we sweep every
                    //   typedef looking for ones that reference the target;
                    //   one unrelated typedef failing to load shouldn't poison
                    //   the whole sweep, so warn-and-skip is intentional.
                    if table_name == target_type {
                        return Err(DoogatError::SqlEngine(format!(
                            "cascade junction: failed to load parent typedef \
                             '{target_type}' for owner-side cleanup: {e}"
                        )));
                    }
                    tracing::warn!(type_name = %table_name, error = %e, "cascade junction: failed to load schema");
                    continue;
                }
            };
            for col in &schema.columns {
                // Reverse direction: another typedef references the deleted
                // target through this column.
                if col.references.as_deref() == Some(target_type) {
                    let jt = format!("{table_name}_{}", col.name);
                    let col_id = format!("{}_id", col.name);
                    self.index.sql_conn().execute(
                        &format!("DELETE FROM \"{jt}\" WHERE \"{col_id}\" = ?1"),
                        params![deleted_id],
                    )?;
                }
                // Parent/owner direction (PRD 00137): the deleted row's own
                // typedef has REFERENCES columns; remove the auto-junction
                // rows it owns.
                if table_name == target_type && col.references.is_some() {
                    let jt = format!("{target_type}_{}", col.name);
                    let parent_id_col = format!("{target_type}_id");
                    self.index.sql_conn().execute(
                        &format!("DELETE FROM \"{jt}\" WHERE \"{parent_id_col}\" = ?1"),
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
            .map_err(|e| classify_materialized_insert_error(e, schema, col_values))?;
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
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
) -> DoogatError {
    use rusqlite::ErrorCode;
    if let rusqlite::Error::SqliteFailure(ref ffi_err, ref msg) = err {
        if ffi_err.code == ErrorCode::ConstraintViolation {
            if let Some(detail) = msg.as_deref() {
                if let Some(cols) = parse_unique_failure_columns(detail, &schema.table_name) {
                    let values: Vec<String> = cols
                        .iter()
                        .map(|c| col_values.get(c).cloned().unwrap_or_default())
                        .collect();
                    return DoogatError::unique_violation(
                        schema.table_name.clone(),
                        cols,
                        values,
                    );
                }
            }
        }
    }
    DoogatError::SqlEngine(err.to_string())
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
