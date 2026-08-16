use std::time::Duration;

use rayon::prelude::*;
use rusqlite::params;

use crate::error::Result;
use crate::git_ops::write_lock;
use crate::traits::DoogatSource;
use crate::types::ParsedDoogat;

use super::Index;

/// Rebuild lock file name, placed under the index's own directory (NOT `.git/`
/// — this lock protects the SQLite index, not the git repo).
const REBUILD_LOCK_FILE_NAME: &str = "ddb-rebuild.lock";

/// How long a rebuild waits for another process's rebuild before failing loud
/// with a retryable `Conflict`.
const REBUILD_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

impl Index {
    /// Check if index is stale (HEAD changed since last rebuild).
    pub fn is_stale(&self, repo: &impl DoogatSource) -> Result<bool> {
        let current_head = repo.head_oid()?.to_string();
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM _ddb_meta WHERE key = 'head'",
                [],
                |row| row.get(0),
            )
            .ok();

        Ok(stored.as_deref() != Some(&current_head))
    }

    /// Return the stored HEAD oid from the last rebuild, if any.
    pub fn stored_head_oid(&self) -> Option<String> {
        self.conn
            .query_row(
                "SELECT value FROM _ddb_meta WHERE key = 'head'",
                [],
                |row| row.get(0),
            )
            .ok()
    }

    /// Store the current HEAD oid in `_ddb_meta`. Callers should invoke this
    /// after committing changes + indexing them to keep `is_stale()` accurate
    /// and avoid spurious incremental_reindex calls on the next operation.
    pub fn store_head(&self, head: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO _ddb_meta (key, value) VALUES ('head', ?1)",
            params![head],
        )?;
        Ok(())
    }

    /// Incremental reindex: only re-index doogats changed between old_head and current HEAD.
    /// Falls back to full rebuild if diff fails (e.g. old HEAD unreachable after gc),
    /// except inside an open write transaction, where a full rebuild is unsafe and
    /// the diff-failure case is a no-op instead (PRD 00157 — see the body).
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn incremental_reindex(
        &self,
        repo: &impl DoogatSource,
        old_head: &str,
        strict: bool,
    ) -> Result<crate::types::RebuildReport> {
        let new_head = repo.head_oid()?.to_string();

        // Try to diff — if it fails, fall back to full rebuild
        let changes = match repo.diff_paths(old_head, &new_head) {
            Ok(c) => c,
            Err(e) => {
                // PRD 00157: a full `rebuild` is unsafe inside an open write
                // transaction (the SINGLETON write paths open `BEGIN IMMEDIATE`,
                // then reach here via a nested `ensure_fresh`): `drop_all_tables`
                // toggles `PRAGMA foreign_keys`, which SQLite ignores inside a
                // transaction, so its `DROP TABLE`s run with FKs enforced and
                // fail with "FOREIGN KEY constraint failed". It is also redundant
                // — the outermost `ensure_fresh` already ran any full rebuild
                // before the transaction opened. Skip the destructive fallback
                // here and leave the index as-is; the next outermost
                // `ensure_fresh` will rebuild if still needed (mirrors the
                // no-stored-HEAD-in-transaction branch in `rebuild_if_stale`).
                if !self.conn.is_autocommit() {
                    tracing::warn!(
                        error = %e,
                        "diff_paths failed inside a transaction; skipping destructive full rebuild"
                    );
                    return Ok(crate::types::RebuildReport::default());
                }
                tracing::warn!(error = %e, "diff_paths failed, falling back to full rebuild");
                return self.locked_rebuild(repo);
            }
        };

        if changes.is_empty() {
            self.store_head(&new_head)?;
            return Ok(crate::types::RebuildReport::default());
        }

        tracing::info!(changed = changes.len(), "incremental_reindex_triggered");

        let (to_index_paths, to_delete, typedef_changed) = Self::partition_changes(&changes);

        // Capture pre-delete types so we can also evict the deleted rows
        // from their materialized type tables.
        let mut delete_types: Vec<(String, String)> = Vec::new();
        for id in &to_delete {
            let t: Option<String> = self
                .conn
                .query_row(
                    "SELECT type FROM doogats WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .ok()
                .flatten();
            if let Some(t) = t {
                if !t.is_empty() && t != "_typedef" {
                    delete_types.push((id.clone(), t));
                }
            }
            self.remove_doogat(id)?;
        }
        self.unmaterialize_data_doogats(&delete_types)?;

        let (indexed, parsed_changes, warnings) =
            self.batch_index_changes(repo, &to_index_paths, strict)?;
        let mut report = crate::types::RebuildReport {
            indexed,
            warnings,
            ..Default::default()
        };

        if typedef_changed {
            self.rematerialize_if_typedef_changed(repo, &mut report)?;
        } else {
            // Typed data doogats need their materialized rows refreshed
            // even when no typedef changed; otherwise path (a) JOINs miss
            // newly-added/edited rows until a full `ddb reindex`.
            self.materialize_data_doogat_changes(repo, &parsed_changes)?;
        }

        self.store_head(&new_head)?;

        tracing::info!(
            indexed = report.indexed,
            tables = report.tables_materialized,
            "incremental_reindex_complete"
        );
        Ok(report)
    }

    /// Separate diff entries into paths to index (added/modified) and IDs to
    /// delete, and flag whether any typedef was touched.
    fn partition_changes(
        changes: &[(crate::types::DiffKind, String)],
    ) -> (Vec<String>, Vec<String>, bool) {
        use crate::types::DiffKind;

        let mut to_index_paths = Vec::new();
        let mut to_delete = Vec::new();
        let mut typedef_changed = false;

        for (kind, path) in changes {
            if path.contains("_typedef/") {
                typedef_changed = true;
            }
            match kind {
                DiffKind::Added | DiffKind::Modified => {
                    to_index_paths.push(path.clone());
                }
                DiffKind::Deleted => {
                    if let Some(id) = crate::parser::extract_id_from_path(path) {
                        to_delete.push(id);
                    }
                }
            }
        }

        (to_index_paths, to_delete, typedef_changed)
    }

    /// Read + parse a batch of changed paths, collecting per-file read/parse
    /// failures as warnings instead of aborting (mirrors `parallel_parse`).
    /// `strict: true` restores the old fatal-on-first-error behavior.
    pub(super) fn batch_index_changes(
        &self,
        repo: &impl DoogatSource,
        paths: &[String],
        strict: bool,
    ) -> Result<(usize, Vec<ParsedDoogat>, Vec<crate::types::ConsistencyWarning>)> {
        if paths.len() > 1 {
            let contents = repo.read_files_batch(paths)?;
            let mut parsed = Vec::with_capacity(contents.len());
            let mut warnings = Vec::new();
            for (path, content_result) in contents {
                let content = match content_result {
                    Ok(content) => content,
                    Err(e) => {
                        if strict {
                            return Err(e);
                        }
                        tracing::warn!(path = %path, error = %e, "batch_index_changes: skipping unreadable file");
                        warnings.push(crate::types::ConsistencyWarning::UnreadableFile {
                            path,
                            error: e.to_string(),
                        });
                        continue;
                    }
                };
                match crate::parser::parse(&content, &path) {
                    Ok(doogat) => parsed.push(doogat),
                    Err(e) => {
                        if strict {
                            return Err(e);
                        }
                        tracing::warn!(path = %path, error = %e, "batch_index_changes: skipping malformed file");
                        warnings.push(crate::types::ConsistencyWarning::MalformedYaml {
                            path,
                            error: e.to_string(),
                        });
                    }
                }
            }
            let count = self.batch_index(&parsed)?;
            Ok((count, parsed, warnings))
        } else if let Some(path) = paths.first() {
            let content = match repo.read_file(path) {
                Ok(content) => content,
                Err(e) => {
                    if strict {
                        return Err(e);
                    }
                    tracing::warn!(path = %path, error = %e, "batch_index_changes: skipping unreadable file");
                    return Ok((
                        0,
                        Vec::new(),
                        vec![crate::types::ConsistencyWarning::UnreadableFile {
                            path: path.clone(),
                            error: e.to_string(),
                        }],
                    ));
                }
            };
            let parsed = match crate::parser::parse(&content, path) {
                Ok(doogat) => doogat,
                Err(e) => {
                    if strict {
                        return Err(e);
                    }
                    tracing::warn!(path = %path, error = %e, "batch_index_changes: skipping malformed file");
                    return Ok((
                        0,
                        Vec::new(),
                        vec![crate::types::ConsistencyWarning::MalformedYaml {
                            path: path.clone(),
                            error: e.to_string(),
                        }],
                    ));
                }
            };
            self.index_doogat(&parsed)?;
            Ok((1, vec![parsed], Vec::new()))
        } else {
            Ok((0, Vec::new(), Vec::new()))
        }
    }

    /// Materialize newly-added or modified data doogats into their type
    /// tables. Skips typedef doogats (handled by `materialize_all_types`)
    /// and untyped doogats (no type table to write to). Loads each typed
    /// doogat's schema once per distinct type to avoid redundant typedef
    /// reads. Errors loading a single type's schema are logged and
    /// skipped — incremental indexing must remain best-effort.
    fn materialize_data_doogat_changes(
        &self,
        repo: &(impl DoogatSource + ?Sized),
        parsed: &[ParsedDoogat],
    ) -> Result<()> {
        use crate::sql_engine::schema_from_parsed;
        use std::collections::HashMap;

        let mut schemas: HashMap<String, Option<crate::types::TableSchema>> = HashMap::new();
        for doogat in parsed {
            let Some(type_name) = doogat.meta.doogat_type.as_deref() else {
                continue;
            };
            if type_name.is_empty() || type_name == "_typedef" {
                continue;
            }
            let id = doogat.meta.id.as_ref().map(|d| d.0.as_str()).unwrap_or("");
            if id.is_empty() {
                continue;
            }
            let schema_opt = schemas.entry(type_name.to_string()).or_insert_with(|| {
                let typedef_path: String = match self.conn.query_row(
                    "SELECT path FROM doogats WHERE type = '_typedef' AND title = ?1 LIMIT 1",
                    rusqlite::params![type_name],
                    |row| row.get(0),
                ) {
                    Ok(p) => p,
                    // No `_typedef` for this type is the normal case — the
                    // type's schema is inferred elsewhere. Not a failure.
                    Err(rusqlite::Error::QueryReturnedNoRows) => return None,
                    Err(e) => {
                        tracing::warn!(error = %e, type_name, "materialize: typedef path query failed, type skipped");
                        return None;
                    }
                };
                let typedef_content = match repo.read_file(&typedef_path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(error = %e, type_name, path = %typedef_path, "materialize: typedef file read failed, type skipped");
                        return None;
                    }
                };
                let typedef_parsed = match crate::parser::parse(&typedef_content, &typedef_path) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(error = %e, type_name, path = %typedef_path, "materialize: typedef parse failed, type skipped");
                        return None;
                    }
                };
                match schema_from_parsed(&typedef_parsed) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        tracing::warn!(error = %e, type_name, "materialize: schema extraction failed, type skipped");
                        None
                    }
                }
            });
            let Some(schema) = schema_opt.as_ref() else {
                continue;
            };
            if let Err(e) = self.materialize_single(schema, id, doogat) {
                tracing::warn!(
                    error = %e,
                    type_name = %type_name,
                    id = %id,
                    "incremental materialization failed for data doogat"
                );
            }
        }
        Ok(())
    }

    /// Mirror `materialize_data_doogat_changes` for deletions: remove rows
    /// for deleted typed doogats from their materialized type tables.
    fn unmaterialize_data_doogats(&self, deleted: &[(String, String)]) -> Result<()> {
        for (id, type_name) in deleted {
            // Quote the table name; user-defined typedefs may legally
            // contain hyphens (e.g. `category-membership`).
            let table = type_name.replace('"', "\"\"");
            let exists: bool = self
                .conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master \
                     WHERE type = 'table' AND name = ?1",
                    rusqlite::params![type_name],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if !exists {
                continue;
            }
            // Junction tables for any REFERENCES column on this type are
            // cleaned up at delete time by `cascade_junction_cleanup` in the
            // service layer; here we only evict the type-table row itself.
            let _ = self.conn.execute(
                &format!("DELETE FROM \"{table}\" WHERE id = ?1"),
                rusqlite::params![id],
            );
        }
        Ok(())
    }

    fn rematerialize_if_typedef_changed(
        &self,
        repo: &impl DoogatSource,
        report: &mut crate::types::RebuildReport,
    ) -> Result<()> {
        tracing::info!("typedef changed, rematerializing all types");
        let mat = self.materialize_all_types(repo)?;
        report.tables_materialized = mat.0;
        report.types_inferred = mat.1;
        Ok(())
    }

    /// Read files sequentially from git, then parse in parallel with rayon.
    ///
    /// Returns successfully parsed doogats and warnings for failures.
    /// Parse errors are collected, not propagated — one bad doogat doesn't block the rest.
    pub fn parallel_parse(
        repo: &impl DoogatSource,
        paths: &[String],
    ) -> Result<(Vec<ParsedDoogat>, Vec<crate::types::ConsistencyWarning>)> {
        // Step 1: sequential git reads (optimal for pack I/O)
        let contents = repo.read_files_batch(paths)?;

        // Step 2: parallel parse (CPU-bound, benefits from rayon)
        // Err payload is (is_read_error, error_text) so the sequential step below
        // can log the error and pick the matching warning variant without cloning
        // the path.
        type ParseOutcome = std::result::Result<ParsedDoogat, (bool, String)>;
        let results: Vec<(String, ParseOutcome)> = contents
            .into_par_iter()
            .map(|(path, content_result)| match content_result {
                Ok(content) => match crate::parser::parse(&content, &path) {
                    Ok(parsed) => (path, Ok(parsed)),
                    Err(e) => (path, Err((false, e.to_string()))),
                },
                Err(e) => (path, Err((true, e.to_string()))),
            })
            .collect();

        // Step 3: partition into successes and warnings
        let mut parsed = Vec::with_capacity(results.len());
        let mut warnings = Vec::new();
        for (path, result) in results {
            match result {
                Ok(z) => parsed.push(z),
                Err((is_read_error, e)) => {
                    tracing::warn!(path = %path, error = %e, "parallel_parse: skipping doogat");
                    warnings.push(if is_read_error {
                        crate::types::ConsistencyWarning::UnreadableFile { path, error: e }
                    } else {
                        crate::types::ConsistencyWarning::MalformedYaml { path, error: e }
                    });
                }
            }
        }

        Ok((parsed, warnings))
    }

    /// Rebuild entire index from all doogats in Git repo.
    /// Indexes all doogats first, collects warnings, then materializes typed tables.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn rebuild(&self, repo: &impl DoogatSource) -> Result<crate::types::RebuildReport> {
        tracing::info!("rebuild_triggered");

        // Drop and recreate all tables so schema changes take effect
        // without needing migrations — the index is a rebuildable cache.
        self.drop_all_tables()?;
        self.conn.execute_batch(Self::SCHEMA_DDL)?;

        let paths = repo.list_doogats()?;
        let mut report = crate::types::RebuildReport::default();

        // Phase 1: sequential git reads + parallel parsing (rayon)
        let (parsed, parse_warnings) = Self::parallel_parse(repo, &paths)?;
        report.warnings.extend(parse_warnings);

        // Phase 2: batch index all parsed doogats (single transaction)
        report.indexed = self.batch_index(&parsed)?;

        // Phase 3: collect consistency warnings
        report
            .warnings
            .extend(self.collect_consistency_warnings(repo));

        // Phase 4: materialize typed tables from cached parse results
        let mat_report = self.materialize_all_types_from(&parsed)?;
        report.tables_materialized = mat_report.0;
        report.types_inferred = mat_report.1;

        let head = repo.head_oid()?.to_string();
        self.conn.execute(
            "INSERT OR REPLACE INTO _ddb_meta (key, value) VALUES ('head', ?1)",
            params![head],
        )?;

        tracing::info!(
            indexed = report.indexed,
            tables = report.tables_materialized,
            warnings = report.warnings.len(),
            "rebuild_complete"
        );

        Ok(report)
    }

    /// Run the destructive full `rebuild` serialized against concurrent
    /// rebuilds in other processes.
    ///
    /// `rebuild` drops every table before repopulating it, so two cold-start
    /// processes racing on one index leave the loser querying tables that no
    /// longer exist. Every *implicit* path to `rebuild` therefore goes through
    /// here: take `<index-dir>/ddb-rebuild.lock`, then re-verify the rebuild is
    /// still needed (the process we waited for may already have done it) and
    /// only then do the destructive work.
    ///
    /// In-memory indexes have no directory to lock and nothing to share, so
    /// they degrade to an unserialized rebuild.
    pub(crate) fn locked_rebuild(
        &self,
        repo: &impl DoogatSource,
    ) -> Result<crate::types::RebuildReport> {
        let _guard = match &self.db_dir {
            Some(dir) => Some(write_lock::acquire(
                dir,
                REBUILD_LOCK_FILE_NAME,
                REBUILD_LOCK_TIMEOUT,
            )?),
            None => None,
        };
        if self.check_integrity()? && !self.is_stale(repo)? {
            return Ok(crate::types::RebuildReport::default());
        }
        self.rebuild(repo)
    }

    /// Rebuild if stale or corrupt. Uses incremental reindex when possible.
    ///
    /// PRD 00157: when reached as a *nested* call from inside an open write
    /// transaction (the SINGLETON write paths open `BEGIN IMMEDIATE`, then
    /// their UPDATE/CREATE branch calls `update_doogat`/`create_doogat_with_extra`,
    /// which call `ensure_fresh` again), the destructive full `rebuild` path is
    /// forbidden: `drop_all_tables` toggles `PRAGMA foreign_keys`, which SQLite
    /// silently ignores inside a transaction, so its `DROP TABLE`s would run
    /// with FKs still enforced and fail with "FOREIGN KEY constraint failed".
    /// That nested call is also redundant — the outer write path already ran
    /// the full integrity-check + rebuild *before* opening the transaction — so
    /// inside a transaction we only ever do the nesting-safe incremental
    /// reindex (or nothing), never a full rebuild.
    pub fn rebuild_if_stale(
        &self,
        repo: &impl DoogatSource,
    ) -> Result<Option<crate::types::RebuildReport>> {
        let in_transaction = !self.conn.is_autocommit();

        // The integrity check's only remedy is a full rebuild, which is unsafe
        // inside a transaction. Skip it there; the outermost ensure_fresh
        // (run before the transaction opened) already performed it.
        if !in_transaction {
            let corrupt = !self.check_integrity()?;
            if corrupt {
                tracing::warn!("index corruption detected, forcing full rebuild");
                return Ok(Some(self.locked_rebuild(repo)?));
            }
        }
        if !self.is_stale(repo)? {
            return Ok(None);
        }
        // Try incremental reindex if we have a stored HEAD. `incremental_reindex`
        // routes its writes through nesting-tolerant helpers, so it composes
        // with an enclosing transaction.
        if let Some(old_head) = self.stored_head_oid() {
            Ok(Some(self.incremental_reindex(repo, &old_head, false)?))
        } else if in_transaction {
            // No stored HEAD inside a transaction: the only option would be a
            // full rebuild, which is unsafe here. Leave the index as-is; the
            // next outermost ensure_fresh will rebuild if still needed.
            Ok(None)
        } else {
            Ok(Some(self.locked_rebuild(repo)?))
        }
    }
}
