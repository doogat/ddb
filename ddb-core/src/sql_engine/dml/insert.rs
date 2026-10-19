use std::collections::BTreeMap;
use std::collections::BTreeSet;

use sqlparser::ast::SetExpr;

use crate::error::{DoogatError, Result};
use crate::parser;
use crate::types::{DoogatId, TableSchema};

use crate::sql_engine::builders::build_data_doogat;
use crate::sql_engine::helpers::{eval_values_nullable, unquote_identifier};
use crate::sql_engine::typed_insert::{prepare_typed_insert_validate, TypedInsertCounters};
use crate::sql_engine::{PendingWrite, SqlEngine, SqlResult};

use super::NullColsPerRow;

/// Raw rows + parallel null-column sets returned by `extract_insert_rows`.
type ExtractedInsertRows = (Vec<Vec<String>>, NullColsPerRow);

impl<'a> SqlEngine<'a> {
    pub(in crate::sql_engine) fn handle_insert(&mut self, ins: &sqlparser::ast::Insert) -> Result<SqlResult> {
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
            // ORDER BY id keeps the reported `existing_id` consistent with
            // Layer 1 (service/validation.rs::check_singleton_constraint) and
            // Layer 3 (lookup_singleton_existing_id below). Invariant pinned
            // by singleton_layers.rs:127-141.
            let existing_id: Option<String> = self
                .index
                .sql_conn()
                .query_row(
                    &format!("SELECT id FROM \"{table_name}\" ORDER BY id ASC LIMIT 1"),
                    [],
                    |row| row.get(0),
                )
                .ok();
            if let Some(id) = existing_id {
                if !on_conflict_ignore {
                    return Err(DoogatError::singleton_violation(table_name.clone(), id));
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
            let (path, content) = self.build_and_index_row(
                &schema,
                &table_name,
                &id,
                &col_values,
                &ref_folder_types,
            )?;
            self.buffer_or_collect_write(path, content, &mut files);
            created_ids.push(id.0.clone());
        }

        self.commit_insert_files(&files, &table_name, created_ids.len())?;
        self.merge_insert_results(on_conflict_ignore, on_conflict_existing, created_ids)
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
                    "ON CONFLICT DO UPDATE is not supported; only DO NOTHING is allowed".into(),
                )),
            },
            _ => Err(DoogatError::SqlEngine(
                "INSERT OR REPLACE/UPSERT not supported: bypasses git storage".into(),
            )),
        }
    }

    fn extract_insert_rows(
        &self,
        ins: &sqlparser::ast::Insert,
        col_names: &[String],
    ) -> Result<ExtractedInsertRows> {
        let query = ins
            .source
            .as_ref()
            .ok_or_else(|| DoogatError::SqlEngine("missing VALUES clause".into()))?;
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
        let doogat = build_data_doogat(
            id,
            schema,
            col_values,
            ref_folder_types,
            Some(self.index.sql_conn()),
        );
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
            if let Err(rb_err) = self.index.sql_conn().execute("ROLLBACK TO insert_row", []) {
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
}
