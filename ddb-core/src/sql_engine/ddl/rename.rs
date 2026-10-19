use rusqlite::params;

use crate::error::{DoogatError, Result};
use crate::parser;
use crate::types::{DoogatId, TableSchema, Zone};

use crate::sql_engine::builders::{build_typedef_doogat, rename_key_in_doogat, schema_from_parsed};
use crate::sql_engine::helpers::validate_rename_target_name;
use crate::sql_engine::{PendingWrite, SqlEngine, SqlResult};

use super::{apply_path_pair_rewrites, RenamePlan};

impl<'a> SqlEngine<'a> {
    /// `ALTER TABLE foo RENAME TO bar`. Validates inputs, computes a rename
    /// plan covering typedef + folder + REFERENCES + backlinks, commits all
    /// writes in one git commit, then renames the materialized SQLite table
    /// and reindexes affected files. Crash semantics are git's: either the
    /// commit lands or it doesn't. A crash between commit and SQLite
    /// rename is recoverable via `consistency::fix_all`.
    pub(super) fn handle_rename_table(
        &mut self,
        table_name: &str,
        typedef_id: &str,
        typedef_path: &str,
        new_name: &str,
    ) -> Result<SqlResult> {
        validate_rename_target_name(new_name)?;

        if self.load_typedef_location(new_name).is_ok() {
            return Err(DoogatError::SqlEngine(format!(
                "typedef already exists: {new_name}"
            )));
        }

        let data_doogats = self.find_type_data_doogats(table_name)?;
        for (id, path) in &data_doogats {
            let content = self.repo.read_file(path)?;
            let parsed = parser::parse(&content, path)?;
            let actual = parsed.meta.doogat_type.as_deref();
            if actual != Some(table_name) {
                return Err(DoogatError::SqlEngine(format!(
                    "consistency error: doogat {id} at {path} has type {actual:?}, expected {table_name:?}"
                )));
            }
        }

        let old_schema = self.load_schema(table_name)?;

        let plan = self.build_rename_plan(
            table_name,
            typedef_id,
            typedef_path,
            new_name,
            &data_doogats,
        )?;

        let write_refs: Vec<(&str, &str)> = plan
            .writes
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        let delete_refs: Vec<&str> = plan.deletes.iter().map(String::as_str).collect();
        // Immediate commit — NOT transaction-buffered. RENAME TABLE is outside
        // the PRD 00161 task-10 transactional-apply scope: schema-apply's differ
        // never emits a rename-table op, so this handler is unreachable under an
        // open apply transaction. Do not invoke it inside a BEGIN/COMMIT
        // expecting atomicity. Transactional rename is PRD 00161 task 11.
        self.repo.commit_batch(
            &write_refs,
            &delete_refs,
            &format!("rename type {table_name} → {new_name}"),
        )?;

        self.cleanup_materialized_tables(table_name, Some(&old_schema))?;

        for (path, _) in &plan.writes {
            let content = self.repo.read_file(path)?;
            let parsed = parser::parse(&content, path)?;
            self.index.index_doogat(&parsed)?;
        }

        self.index.rematerialize_type(new_name, self.repo)?;

        Ok(SqlResult::Ok(format!(
            "table {table_name} renamed to {new_name}"
        )))
    }

    fn build_rename_plan(
        &mut self,
        table_name: &str,
        typedef_id: &str,
        typedef_path: &str,
        new_name: &str,
        data_doogats: &[(String, String)],
    ) -> Result<RenamePlan> {
        let mut writes: Vec<(String, String)> = Vec::new();
        let mut deletes: Vec<String> = Vec::new();

        let folder_prefix = format!("ddb/{table_name}/");
        let path_pairs: Vec<(String, String)> = data_doogats
            .iter()
            .filter(|(_, old_path)| old_path.starts_with(&folder_prefix))
            .map(|(id, old_path)| (old_path.clone(), format!("ddb/{new_name}/{id}.md")))
            .collect();

        self.plan_typedef_rewrite(table_name, typedef_id, typedef_path, new_name, &mut writes)?;

        self.plan_data_doogats_move(
            table_name,
            new_name,
            data_doogats,
            &path_pairs,
            &mut writes,
            &mut deletes,
        )?;

        self.plan_references_rewrite(table_name, typedef_id, new_name, &mut writes)?;

        self.plan_backlinks_rewrite(table_name, &path_pairs, &deletes, &mut writes)?;

        Ok(RenamePlan { writes, deletes })
    }

    fn plan_typedef_rewrite(
        &mut self,
        table_name: &str,
        typedef_id: &str,
        typedef_path: &str,
        new_name: &str,
        writes: &mut Vec<(String, String)>,
    ) -> Result<()> {
        let mut schema = self.load_schema(table_name)?;
        schema.table_name = new_name.to_string();
        let typedef = build_typedef_doogat(&DoogatId(typedef_id.to_string()), &schema);
        writes.push((typedef_path.to_string(), parser::serialize(&typedef)));
        Ok(())
    }

    fn plan_data_doogats_move(
        &self,
        table_name: &str,
        new_name: &str,
        data_doogats: &[(String, String)],
        path_pairs: &[(String, String)],
        writes: &mut Vec<(String, String)>,
        deletes: &mut Vec<String>,
    ) -> Result<()> {
        let folder_prefix = format!("ddb/{table_name}/");
        for (id, old_path) in data_doogats {
            let content = self.repo.read_file(old_path)?;
            let mut parsed = parser::parse(&content, old_path)?;
            parsed.meta.doogat_type = Some(new_name.to_string());
            let serialized = parser::serialize(&parsed);
            let rewritten = apply_path_pair_rewrites(&serialized, path_pairs);
            if old_path.starts_with(&folder_prefix) {
                let new_path = format!("ddb/{new_name}/{id}.md");
                writes.push((new_path, rewritten));
                deletes.push(old_path.clone());
            } else {
                // Flat layout: keep the path, just rewrite content.
                writes.push((old_path.clone(), rewritten));
            }
        }
        Ok(())
    }

    fn plan_references_rewrite(
        &mut self,
        table_name: &str,
        source_typedef_id: &str,
        new_name: &str,
        writes: &mut Vec<(String, String)>,
    ) -> Result<()> {
        let mut stmt = self
            .index
            .sql_conn()
            .prepare("SELECT id, path FROM doogats WHERE type = '_typedef' AND id != ?1")?;
        let other_typedefs: Vec<(String, String)> = stmt
            .query_map(params![source_typedef_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        for (other_id, other_path) in other_typedefs {
            let content = self.repo.read_file(&other_path)?;
            let parsed = parser::parse(&content, &other_path)?;
            let mut schema = schema_from_parsed(&parsed)?;
            let mut changed = false;
            for col in schema.columns.iter_mut() {
                if col.references.as_deref() == Some(table_name) {
                    col.references = Some(new_name.to_string());
                    changed = true;
                }
            }
            if changed {
                let rewritten = build_typedef_doogat(&DoogatId(other_id), &schema);
                writes.push((other_path, parser::serialize(&rewritten)));
            }
        }
        Ok(())
    }

    fn plan_backlinks_rewrite(
        &self,
        table_name: &str,
        path_pairs: &[(String, String)],
        deletes: &[String],
        writes: &mut Vec<(String, String)>,
    ) -> Result<()> {
        let folder_prefix = format!("ddb/{table_name}/");
        let mut stmt = self.index.sql_conn().prepare(
            "SELECT DISTINCT z.path FROM _ddb_links l \
             JOIN doogats z ON l.source_id = z.id \
             WHERE l.target_path LIKE ?1 || '%'",
        )?;
        let backlink_paths: Vec<String> = stmt
            .query_map(params![folder_prefix], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);

        for src_path in backlink_paths {
            if deletes.contains(&src_path) || writes.iter().any(|(p, _)| *p == src_path) {
                continue;
            }
            let original = self.repo.read_file(&src_path)?;
            let rewritten = apply_path_pair_rewrites(&original, path_pairs);
            if rewritten != original {
                writes.push((src_path, rewritten));
            }
        }
        Ok(())
    }

    pub(super) fn handle_rename_column(
        &mut self,
        table_name: &str,
        typedef_id: &str,
        typedef_path: &str,
        schema: &mut TableSchema,
        old_name: &str,
        new_name: &str,
    ) -> Result<SqlResult> {
        if schema.columns.iter().any(|c| c.name == new_name) {
            return Err(DoogatError::SqlEngine(format!(
                "column already exists: {new_name}"
            )));
        }
        let col = schema
            .columns
            .iter_mut()
            .find(|c| c.name == old_name)
            .ok_or_else(|| DoogatError::SqlEngine(format!("column not found: {old_name}")))?;
        let zone = col.effective_zone();
        col.name = new_name.to_string();

        let id = DoogatId(typedef_id.to_string());
        let schema_doogat = build_typedef_doogat(&id, schema);
        let typedef_content = parser::serialize(&schema_doogat);

        let data_doogats = self.resolve_matching_ids(table_name, &None)?;
        let data_writes =
            self.rewrite_data_doogats_for_rename(&data_doogats, old_name, new_name, &zone)?;

        let mut files: Vec<(String, String)> = Vec::with_capacity(data_writes.len() + 1);
        files.push((typedef_path.to_string(), typedef_content.clone()));
        files.extend(data_writes);

        if self.txn.is_some() {
            // Inside an open transaction (e.g. a declarative schema apply that
            // emits RenameColumn alongside other ops, PRD 00161 task 11), buffer
            // the typedef AND every renamed row-doogat into the SAME
            // `TransactionBuffer` so the whole rename flushes in the single
            // `commit_batch` on COMMIT — atomic with the rest of the plan. A
            // mid-plan failure rolls the SAVEPOINT back and nothing reaches git.
            self.buffer_rename_writes(&files);
            // Index the renamed typedef + rows under the SAVEPOINT so a later op
            // in the same plan sees the renamed state. Index from the in-memory
            // content we just buffered, not a fresh git read (the writes are not
            // committed yet, so a git read would be buffer-blind).
            self.reindex_rename_from_buffer(typedef_path, &files)?;
            // `rematerialize_type` reads typedef + rows from git, so it would be
            // blind to the buffered writes — defer it to COMMIT, after the flush.
            self.rematerialize_or_defer(table_name)?;
        } else {
            let file_refs: Vec<(&str, &str)> = files
                .iter()
                .map(|(p, c)| (p.as_str(), c.as_str()))
                .collect();
            self.repo.commit_files(
                &file_refs,
                &format!("alter table {table_name} rename {old_name} to {new_name}"),
            )?;
            self.reindex_after_rename(&typedef_content, typedef_path, &data_doogats, table_name)?;
        }

        Ok(SqlResult::Ok(format!(
            "renamed {old_name} to {new_name} in {table_name}"
        )))
    }

    /// Push every `(path, content)` write a rename produces into the open
    /// transaction buffer. Caller must hold an open transaction.
    fn buffer_rename_writes(&mut self, files: &[(String, String)]) {
        if let Some(ref mut buf) = self.txn {
            for (path, content) in files {
                buf.writes.push(PendingWrite {
                    path: path.clone(),
                    content: content.clone(),
                });
            }
        }
    }

    /// Index the renamed typedef and each renamed row-doogat under the open
    /// SAVEPOINT, parsing the in-memory buffered content (not a git read, which
    /// would be blind to the uncommitted buffer). `files[0]` is the typedef.
    fn reindex_rename_from_buffer(
        &mut self,
        typedef_path: &str,
        files: &[(String, String)],
    ) -> Result<()> {
        let parsed_typedef = parser::parse(&files[0].1, typedef_path)?;
        self.index.index_doogat(&parsed_typedef)?;
        for (path, content) in &files[1..] {
            let parsed = parser::parse(content, path)?;
            self.index.index_doogat(&parsed)?;
        }
        Ok(())
    }

    fn rewrite_data_doogats_for_rename(
        &self,
        data_doogats: &[(String, String)],
        old_name: &str,
        new_name: &str,
        zone: &Zone,
    ) -> Result<Vec<(String, String)>> {
        let mut writes = Vec::with_capacity(data_doogats.len());
        for (_, path) in data_doogats {
            let content = self.repo.read_file(path)?;
            let mut parsed = parser::parse(&content, path)?;
            rename_key_in_doogat(&mut parsed, old_name, new_name, zone);
            writes.push((path.clone(), parser::serialize(&parsed)));
        }
        Ok(writes)
    }

    fn reindex_after_rename(
        &mut self,
        typedef_content: &str,
        typedef_path: &str,
        data_doogats: &[(String, String)],
        table_name: &str,
    ) -> Result<()> {
        let parsed_typedef = parser::parse(typedef_content, typedef_path)?;
        self.index.index_doogat(&parsed_typedef)?;
        for (_, path) in data_doogats {
            let content = self.repo.read_file(path)?;
            let parsed = parser::parse(&content, path)?;
            self.index.index_doogat(&parsed)?;
        }
        self.index.rematerialize_type(table_name, self.repo)?;
        Ok(())
    }
}
