use rayon::prelude::*;
use rusqlite::params;

use crate::error::Result;
use crate::traits::DoogatSource;
use crate::types::ParsedDoogat;

use super::Index;

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
    /// Falls back to full rebuild if diff fails (e.g. old HEAD unreachable after gc).
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn incremental_reindex(
        &self,
        repo: &impl DoogatSource,
        old_head: &str,
    ) -> Result<crate::types::RebuildReport> {
        let new_head = repo.head_oid()?.to_string();

        // Try to diff — if it fails, fall back to full rebuild
        let changes = match repo.diff_paths(old_head, &new_head) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "diff_paths failed, falling back to full rebuild");
                return self.rebuild(repo);
            }
        };

        if changes.is_empty() {
            self.store_head(&new_head)?;
            return Ok(crate::types::RebuildReport::default());
        }

        tracing::info!(changed = changes.len(), "incremental_reindex_triggered");

        let (to_index_paths, to_delete, typedef_changed) = Self::partition_changes(&changes);

        for id in &to_delete {
            self.remove_doogat(id)?;
        }

        let indexed = self.batch_index_changes(repo, &to_index_paths)?;
        let mut report = crate::types::RebuildReport {
            indexed,
            ..Default::default()
        };

        if typedef_changed {
            self.rematerialize_if_typedef_changed(repo, &mut report)?;
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

    /// Read changed files from the repo, parse them, and index. Returns the
    /// number of doogats indexed.
    fn batch_index_changes(
        &self,
        repo: &impl DoogatSource,
        paths: &[String],
    ) -> Result<usize> {
        if paths.len() > 1 {
            let contents = repo.read_files_batch(paths)?;
            let mut parsed = Vec::with_capacity(contents.len());
            for (path, content_result) in contents {
                let content = content_result?;
                parsed.push(crate::parser::parse(&content, &path)?);
            }
            self.batch_index(&parsed)
        } else if let Some(path) = paths.first() {
            let content = repo.read_file(path)?;
            let parsed = crate::parser::parse(&content, path)?;
            self.index_doogat(&parsed)?;
            Ok(1)
        } else {
            Ok(0)
        }
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
        let results: Vec<(String, std::result::Result<ParsedDoogat, String>)> = contents
            .into_par_iter()
            .map(|(path, content_result)| match content_result {
                Ok(content) => match crate::parser::parse(&content, &path) {
                    Ok(parsed) => (path, Ok(parsed)),
                    Err(e) => (path, Err(e.to_string())),
                },
                Err(e) => (path, Err(e.to_string())),
            })
            .collect();

        // Step 3: partition into successes and warnings
        let mut parsed = Vec::with_capacity(results.len());
        let mut warnings = Vec::new();
        for (path, result) in results {
            match result {
                Ok(z) => parsed.push(z),
                Err(e) => {
                    tracing::warn!(path = %path, error = %e, "parallel_parse: skipping doogat");
                    warnings
                        .push(crate::types::ConsistencyWarning::MalformedYaml { path, error: e });
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

    /// Rebuild if stale or corrupt. Uses incremental reindex when possible.
    pub fn rebuild_if_stale(
        &self,
        repo: &impl DoogatSource,
    ) -> Result<Option<crate::types::RebuildReport>> {
        let corrupt = !self.check_integrity()?;
        if corrupt {
            tracing::warn!("index corruption detected, forcing full rebuild");
            return Ok(Some(self.rebuild(repo)?));
        }
        if !self.is_stale(repo)? {
            return Ok(None);
        }
        // Try incremental reindex if we have a stored HEAD
        if let Some(old_head) = self.stored_head_oid() {
            Ok(Some(self.incremental_reindex(repo, &old_head)?))
        } else {
            Ok(Some(self.rebuild(repo)?))
        }
    }
}
