mod graph;
pub(crate) mod materialize;
mod resolve;

use std::path::Path;

use rayon::prelude::*;
use rusqlite::{params, Connection};

use crate::error::{Result, DoogatError};
use crate::traits::DoogatSource;
use crate::types::ParsedDoogat;

impl From<rusqlite::Error> for DoogatError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e.to_string())
    }
}

pub use crate::types::{PaginatedSearchResult, SearchResult};
use crate::types::{SearchFieldOp, SearchFilters};

pub struct Index {
    pub(crate) conn: Connection,
}

impl Index {
    /// Schema DDL for all internal tables. Kept in one place so `open` and
    /// `rebuild` (which drops everything first) use the same definitions.
    const SCHEMA_DDL: &str = "
        CREATE TABLE IF NOT EXISTS doogats (
            id TEXT PRIMARY KEY,
            title TEXT,
            date TEXT,
            type TEXT,
            path TEXT UNIQUE NOT NULL,
            body TEXT,
            updated_at TEXT
        );

        CREATE TABLE IF NOT EXISTS _ddb_tags (
            doogat_id TEXT NOT NULL REFERENCES doogats(id),
            tag TEXT NOT NULL,
            source TEXT NOT NULL DEFAULT 'frontmatter'
        );
        CREATE INDEX IF NOT EXISTS idx_ddb_tags_tag ON _ddb_tags(tag);

        CREATE TABLE IF NOT EXISTS _ddb_fields (
            doogat_id TEXT NOT NULL REFERENCES doogats(id),
            key TEXT NOT NULL,
            value TEXT,
            zone TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_ddb_fields_key ON _ddb_fields(key);

        CREATE TABLE IF NOT EXISTS _ddb_links (
            source_id TEXT NOT NULL REFERENCES doogats(id),
            target_path TEXT NOT NULL,
            display TEXT,
            zone TEXT,
            kind TEXT NOT NULL DEFAULT 'wikilink'
        );
        CREATE INDEX IF NOT EXISTS idx_ddb_links_target ON _ddb_links(target_path);

        CREATE TABLE IF NOT EXISTS _ddb_aliases (
            doogat_id TEXT NOT NULL REFERENCES doogats(id),
            alias TEXT COLLATE NOCASE NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_ddb_aliases_alias ON _ddb_aliases(alias);

        CREATE TABLE IF NOT EXISTS _ddb_meta (
            key TEXT PRIMARY KEY,
            value TEXT
        );

        CREATE TABLE IF NOT EXISTS _ddb_attachments (
            doogat_id TEXT NOT NULL REFERENCES doogats(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            mime TEXT,
            size INTEGER,
            path TEXT,
            PRIMARY KEY (doogat_id, name)
        );

        CREATE TABLE IF NOT EXISTS _ddb_checkboxes (
            doogat_id TEXT NOT NULL REFERENCES doogats(id),
            state TEXT NOT NULL CHECK (state IN ('open', 'done', 'info')),
            content TEXT NOT NULL,
            date TEXT,
            due_date TEXT,
            line_number INTEGER,
            indent_level INTEGER DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_ddb_checkboxes_state ON _ddb_checkboxes(state);
        CREATE INDEX IF NOT EXISTS idx_ddb_checkboxes_doogat ON _ddb_checkboxes(doogat_id);

        CREATE VIRTUAL TABLE IF NOT EXISTS _ddb_fts USING fts5(
            title, body, tags,
            tokenize = 'porter unicode61'
        );
    ";

    /// Open (or create) the SQLite index database.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::configure_connection(conn)
    }

    /// Open an isolated in-memory SQLite index.
    ///
    /// This is primarily useful for tests that need a fresh derived index
    /// without paying filesystem setup costs on every case.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::configure_connection(conn)
    }

    fn configure_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA busy_timeout=5000;")?;
        conn.execute_batch(Self::SCHEMA_DDL)?;

        Ok(Self { conn })
    }

    /// Drop every table (internal + materialized) so the schema can be
    /// recreated from scratch. The index is a derived cache — no migrations,
    /// just rebuild.
    fn drop_all_tables(&self) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT name FROM sqlite_master \
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'",
        )?;
        let tables: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);
        // Disable FK checks so drop order doesn't matter.
        self.conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
        for table in &tables {
            self.conn
                .execute_batch(&format!("DROP TABLE IF EXISTS \"{table}\""))?;
        }
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(())
    }

    /// Run `f` inside a named SAVEPOINT, rolling back on error.
    fn with_savepoint(&self, name: &str, f: impl FnOnce() -> Result<()>) -> Result<()> {
        self.conn.execute(&format!("SAVEPOINT {name}"), [])?;
        match f() {
            Ok(()) => {
                self.conn.execute(&format!("RELEASE {name}"), [])?;
                Ok(())
            }
            Err(e) => {
                if let Err(rb_err) = self.conn.execute(&format!("ROLLBACK TO {name}"), []) {
                    tracing::warn!(savepoint = name, error = %rb_err, "savepoint rollback failed");
                }
                if let Err(rl_err) = self.conn.execute(&format!("RELEASE {name}"), []) {
                    tracing::warn!(savepoint = name, error = %rl_err, "savepoint release failed");
                }
                Err(e)
            }
        }
    }

    /// Upsert a single parsed doogat into the index (savepoint-wrapped).
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn index_doogat(&self, doogat: &ParsedDoogat) -> Result<()> {
        self.with_savepoint("index_doogat", || self.upsert_doogat(doogat))
    }

    /// Index many doogats in a single transaction.
    ///
    /// Per-doogat errors are logged and skipped — they don't abort the batch.
    /// Returns the number of successfully indexed doogats.
    pub fn batch_index(&self, doogats: &[ParsedDoogat]) -> Result<usize> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let mut count = 0;
        for doogat in doogats {
            if let Err(e) = self.upsert_doogat(doogat) {
                tracing::warn!(path = %doogat.path, error = %e, "batch_index: skipping doogat");
                continue;
            }
            count += 1;
        }

        self.conn.execute_batch("COMMIT")?;
        Ok(count)
    }

    /// Shared upsert logic used by both `index_doogat` (savepoint) and `batch_index` (transaction).
    fn upsert_doogat(&self, doogat: &ParsedDoogat) -> Result<()> {
        let id = doogat.meta.id.as_ref().map(|z| z.0.as_str()).unwrap_or("");
        let title = doogat.meta.title.as_deref().unwrap_or("");
        let date = doogat.meta.date.as_deref().unwrap_or("");
        let ztype = doogat.meta.doogat_type.as_deref().unwrap_or("");
        let now = chrono::Utc::now().to_rfc3339();
        let tags_str = doogat.meta.tags.join(", ");

        // Delete old FTS entry (no-op if doogat doesn't exist yet)
        self.conn.execute(
            "DELETE FROM _ddb_fts WHERE rowid = (SELECT rowid FROM doogats WHERE id = ?1)",
            params![id],
        )?;

        // Upsert doogat
        self.conn.execute(
            "INSERT OR REPLACE INTO doogats (id, title, date, type, path, body, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, title, date, ztype, doogat.path, doogat.body, now],
        )?;

        // Delete and reinsert related data
        self.conn
            .execute("DELETE FROM _ddb_tags WHERE doogat_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM _ddb_fields WHERE doogat_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM _ddb_links WHERE source_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM _ddb_aliases WHERE doogat_id = ?1", params![id])?;
        self.conn.execute(
            "DELETE FROM _ddb_checkboxes WHERE doogat_id = ?1",
            params![id],
        )?;

        for tag in &doogat.meta.tags {
            self.conn.execute(
                "INSERT INTO _ddb_tags (doogat_id, tag, source) VALUES (?1, ?2, 'frontmatter')",
                params![id, tag],
            )?;
        }

        for tag in &doogat.body_tags {
            self.conn.execute(
                "INSERT INTO _ddb_tags (doogat_id, tag, source) VALUES (?1, ?2, 'body')",
                params![id, tag],
            )?;
        }

        for cb in &doogat.checkboxes {
            let state = match cb.state {
                crate::types::CheckboxState::Open => "open",
                crate::types::CheckboxState::Done => "done",
                crate::types::CheckboxState::Info => "info",
            };
            self.conn.execute(
                "INSERT INTO _ddb_checkboxes (doogat_id, state, content, date, due_date, line_number, indent_level) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, state, cb.content, cb.date, cb.due_date, cb.line_number as i64, cb.indent_level as i64],
            )?;
        }

        for field in &doogat.inline_fields {
            let zone = format!("{:?}", field.zone);
            self.conn.execute(
                "INSERT INTO _ddb_fields (doogat_id, key, value, zone) VALUES (?1, ?2, ?3, ?4)",
                params![id, field.key, field.value, zone],
            )?;
        }

        // Insert frontmatter extras, flattening nested maps/lists into dot-notation keys
        for (key, value) in &doogat.meta.extra {
            let escaped = key
                .replace('\\', "\\\\")
                .replace('.', "\\.")
                .replace('[', "\\[");
            flatten_value_into_fields(&self.conn, id, &escaped, value)?;
        }

        for link in &doogat.links {
            let zone = format!("{:?}", link.zone);
            let kind = link.kind.as_str();
            self.conn.execute(
                "INSERT INTO _ddb_links (source_id, target_path, display, zone, kind) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, link.target, link.display, zone, kind],
            )?;
        }

        // Insert aliases
        if let Some(crate::types::Value::List(aliases)) = doogat.meta.extra.get("aliases") {
            for alias in aliases {
                if let crate::types::Value::String(a) = alias {
                    self.conn.execute(
                        "INSERT INTO _ddb_aliases (doogat_id, alias) VALUES (?1, ?2)",
                        params![id, a],
                    )?;
                }
            }
        }

        // Insert attachments
        self.conn.execute(
            "DELETE FROM _ddb_attachments WHERE doogat_id = ?1",
            params![id],
        )?;
        if let Some(crate::types::Value::List(items)) = doogat.meta.extra.get("attachments") {
            for item in items {
                if let crate::types::Value::Map(map) = item {
                    let name = map.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let mime = map.get("mime").and_then(|v| v.as_str()).unwrap_or("");
                    let size = map.get("size").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
                    let path = format!("reference/{}/{}", id, name);
                    self.conn.execute(
                        "INSERT INTO _ddb_attachments (doogat_id, name, mime, size, path) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![id, name, mime, size, path],
                    )?;
                }
            }
        }

        // Insert FTS entry
        self.conn.execute(
            "INSERT INTO _ddb_fts (rowid, title, body, tags) VALUES (
                (SELECT rowid FROM doogats WHERE id = ?1), ?2, ?3, ?4
            )",
            params![id, title, doogat.body, tags_str],
        )?;

        Ok(())
    }

    /// Remove a doogat from the index by ID.
    pub fn remove_doogat(&self, id: &str) -> Result<()> {
        self.with_savepoint("remove_doogat", || {
            self.conn.execute(
                "DELETE FROM _ddb_fts WHERE rowid = (SELECT rowid FROM doogats WHERE id = ?1)",
                params![id],
            )?;
            self.conn
                .execute("DELETE FROM _ddb_tags WHERE doogat_id = ?1", params![id])?;
            self.conn
                .execute("DELETE FROM _ddb_fields WHERE doogat_id = ?1", params![id])?;
            self.conn
                .execute("DELETE FROM _ddb_links WHERE source_id = ?1", params![id])?;
            self.conn
                .execute("DELETE FROM _ddb_aliases WHERE doogat_id = ?1", params![id])?;
            self.conn.execute(
                "DELETE FROM _ddb_checkboxes WHERE doogat_id = ?1",
                params![id],
            )?;
            self.conn
                .execute("DELETE FROM doogats WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    /// Remove junction table rows where `deleted_id` appears as a referenced
    /// target.  Scans all typedef schemas for REFERENCES columns pointing to
    /// `target_type` and deletes matching rows from their junction tables.
    pub fn cascade_junction_cleanup(
        &self,
        repo: &dyn DoogatSource,
        target_type: &str,
        deleted_id: &str,
    ) -> Result<()> {
        let schemas = self.load_all_typedefs(repo);
        for (table_name, schema) in &schemas {
            for col in &schema.columns {
                if col.references.as_deref() == Some(target_type) {
                    let jt = format!("{table_name}_{}", col.name);
                    let col_id = format!("{}_id", col.name);
                    self.conn.execute(
                        &format!("DELETE FROM \"{jt}\" WHERE \"{col_id}\" = ?1"),
                        params![deleted_id],
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Check database integrity: runs PRAGMA integrity_check and verifies core tables exist.
    pub fn check_integrity(&self) -> Result<bool> {
        // PRAGMA integrity_check returns "ok" if clean
        let integrity: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap_or_else(|_| "error".to_string());
        if integrity != "ok" {
            return Ok(false);
        }

        // Verify core tables exist
        for table in &[
            "doogats",
            "_ddb_fts",
            "_ddb_tags",
            "_ddb_fields",
            "_ddb_links",
            "_ddb_aliases",
            "_ddb_checkboxes",
            "_ddb_meta",
        ] {
            let exists: bool = self
                .conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if !exists {
                return Ok(false);
            }
        }

        Ok(true)
    }

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

    /// Incremental reindex: only re-index doogats changed between old_head and current HEAD.
    /// Falls back to full rebuild if diff fails (e.g. old HEAD unreachable after gc).
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn incremental_reindex(
        &self,
        repo: &impl DoogatSource,
        old_head: &str,
    ) -> Result<crate::types::RebuildReport> {
        use crate::types::DiffKind;

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
            // HEAD changed but no doogat files changed (e.g. config-only commit)
            self.conn.execute(
                "INSERT OR REPLACE INTO _ddb_meta (key, value) VALUES ('head', ?1)",
                params![new_head],
            )?;
            return Ok(crate::types::RebuildReport::default());
        }

        tracing::info!(changed = changes.len(), "incremental_reindex_triggered");
        let mut report = crate::types::RebuildReport::default();
        let mut typedef_changed = false;

        // Partition changes by kind
        let mut to_index_paths = Vec::new();
        let mut to_delete = Vec::new();

        for (kind, path) in &changes {
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

        // Handle deletes individually (cheap operations)
        for id in &to_delete {
            self.remove_doogat(id)?;
        }

        // Batch-index additions/modifications
        if to_index_paths.len() > 1 {
            let contents = repo.read_files_batch(&to_index_paths)?;
            let mut parsed = Vec::with_capacity(contents.len());
            for (path, content_result) in contents {
                let content = content_result?;
                parsed.push(crate::parser::parse(&content, &path)?);
            }
            report.indexed = self.batch_index(&parsed)?;
        } else if let Some(path) = to_index_paths.first() {
            let content = repo.read_file(path)?;
            let parsed = crate::parser::parse(&content, path)?;
            self.index_doogat(&parsed)?;
            report.indexed = 1;
        }

        // If any typedef changed, full rematerialization is needed
        if typedef_changed {
            tracing::info!("typedef changed, rematerializing all types");
            let mat = self.materialize_all_types(repo)?;
            report.tables_materialized = mat.0;
            report.types_inferred = mat.1;
        }

        // Update stored HEAD
        self.conn.execute(
            "INSERT OR REPLACE INTO _ddb_meta (key, value) VALUES ('head', ?1)",
            params![new_head],
        )?;

        tracing::info!(
            indexed = report.indexed,
            tables = report.tables_materialized,
            "incremental_reindex_complete"
        );
        Ok(report)
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

    /// Full-text search with snippets and ranking.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        self.search_hits(query, None, &SearchFilters::default())
    }

    /// Paginated full-text search with snippets, ranking, and total count.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn search_paginated(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<PaginatedSearchResult> {
        self.search_paginated_filtered(query, limit, offset, &SearchFilters::default())
    }

    /// Paginated full-text search with optional type/tag/field filters.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn search_paginated_filtered(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
        filters: &SearchFilters,
    ) -> Result<PaginatedSearchResult> {
        let (filter_clauses, filter_params) = Self::build_filter_clauses(filters);
        let hits = self.search_hits_inner(query, Some((limit, offset)), &filter_clauses, filter_params.clone())?;

        let filter_sql = filter_clauses.join(" ");
        let count_sql = if filter_sql.is_empty() {
            "SELECT COUNT(*) FROM _ddb_fts WHERE _ddb_fts MATCH ?1".to_string()
        } else {
            format!(
                "SELECT COUNT(*) FROM _ddb_fts \
                 JOIN doogats z ON z.rowid = _ddb_fts.rowid \
                 WHERE _ddb_fts MATCH ?1 {filter_sql}"
            )
        };

        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(query.to_string())];
        for p in filter_params {
            all_params.push(Box::new(p));
        }
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| &**p).collect();
        let total_count: usize = self.conn.query_row(&count_sql, param_refs.as_slice(), |row| row.get(0))?;

        Ok(PaginatedSearchResult { hits, total_count })
    }

    fn search_hits(
        &self,
        query: &str,
        pagination: Option<(usize, usize)>,
        filters: &SearchFilters,
    ) -> Result<Vec<SearchResult>> {
        let (filter_clauses, filter_params) = Self::build_filter_clauses(filters);
        self.search_hits_inner(query, pagination, &filter_clauses, filter_params)
    }

    fn search_hits_inner(
        &self,
        query: &str,
        pagination: Option<(usize, usize)>,
        filter_clauses: &[String],
        filter_params: Vec<String>,
    ) -> Result<Vec<SearchResult>> {
        let filter_sql = filter_clauses.join(" ");
        let base = format!(
            "SELECT z.id, z.title, z.path, \
             snippet(_ddb_fts, 1, '<b>', '</b>', '...', 32), rank \
             FROM _ddb_fts \
             JOIN doogats z ON z.rowid = _ddb_fts.rowid \
             WHERE _ddb_fts MATCH ?1 {filter_sql}\
             ORDER BY rank"
        );

        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(query.to_string())];
        for p in filter_params {
            all_params.push(Box::new(p));
        }

        let sql = match pagination {
            Some(_) => {
                let limit_idx = all_params.len() + 1;
                let offset_idx = all_params.len() + 2;
                format!("{base} LIMIT ?{limit_idx} OFFSET ?{offset_idx}")
            }
            None => base,
        };

        if let Some((limit, offset)) = pagination {
            all_params.push(Box::new(limit as i64));
            all_params.push(Box::new(offset as i64));
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> = all_params.iter().map(|p| &**p).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), Self::map_search_row)?;

        let mut hits = Vec::new();
        for r in rows {
            hits.push(r?);
        }
        Ok(hits)
    }

    fn build_filter_clauses(filters: &SearchFilters) -> (Vec<String>, Vec<String>) {
        let mut clauses = Vec::new();
        let mut params = Vec::new();
        let mut idx = 2; // ?1 is always the FTS query

        if let Some(ref types) = filters.types {
            if !types.is_empty() {
                let placeholders: Vec<String> =
                    types.iter().map(|_| { let p = format!("?{idx}"); idx += 1; p }).collect();
                clauses.push(format!("AND z.type IN ({})", placeholders.join(", ")));
                params.extend(types.clone());
            }
        }

        if let Some(ref tag) = filters.tag {
            clauses.push(format!(
                "AND z.id IN (SELECT doogat_id FROM _ddb_tags WHERE tag = ?{idx})"
            ));
            params.push(tag.clone());
            idx += 1;
        }

        if let Some(ref where_filters) = filters.where_filters {
            for wf in where_filters {
                match &wf.op {
                    SearchFieldOp::Eq(val) => {
                        clauses.push(format!(
                            "AND z.id IN (SELECT doogat_id FROM _ddb_fields WHERE key = ?{} AND value = ?{})",
                            idx, idx + 1
                        ));
                        params.push(wf.field.clone());
                        params.push(val.clone());
                        idx += 2;
                    }
                    SearchFieldOp::Contains(val) => {
                        clauses.push(format!(
                            "AND z.id IN (SELECT doogat_id FROM _ddb_fields WHERE key = ?{} AND value LIKE '%' || ?{} || '%')",
                            idx, idx + 1
                        ));
                        params.push(wf.field.clone());
                        params.push(val.clone());
                        idx += 2;
                    }
                }
            }
        }

        (clauses, params)
    }

    fn map_search_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchResult> {
        Ok(SearchResult {
            id: row.get(0)?,
            title: row.get(1)?,
            path: row.get(2)?,
            snippet: row.get(3)?,
            rank: row.get(4)?,
        })
    }

    /// Return all tags with their usage counts, ordered by count descending then name ascending.
    pub fn list_tags(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT tag, COUNT(*) as count FROM _ddb_tags GROUP BY tag ORDER BY count DESC, tag ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Find doogats by hierarchical tag prefix.
    pub fn by_tag(&self, prefix: &str) -> Result<Vec<String>> {
        let pattern = format!("{prefix}%");
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT doogat_id FROM _ddb_tags WHERE tag LIKE ?1")?;
        let ids = stmt.query_map(params![pattern], |row| row.get(0))?;
        let mut out = Vec::new();
        for id in ids {
            out.push(id?);
        }
        Ok(out)
    }

    /// Execute arbitrary SQL query, return rows as string vectors.
    pub fn query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>> {
        let mut stmt = self.conn.prepare(sql)?;
        let col_count = stmt.column_count();
        let mut rows = Vec::new();

        let mut query_rows = stmt.query([])?;
        while let Some(row) = query_rows.next()? {
            let mut values = Vec::new();
            for i in 0..col_count {
                let val: String = row
                    .get::<_, rusqlite::types::Value>(i)
                    .map(|v| match v {
                        rusqlite::types::Value::Null => "NULL".to_string(),
                        rusqlite::types::Value::Integer(i) => i.to_string(),
                        rusqlite::types::Value::Real(f) => f.to_string(),
                        rusqlite::types::Value::Text(s) => s,
                        rusqlite::types::Value::Blob(b) => format!("<blob:{} bytes>", b.len()),
                    })
                    .unwrap_or_else(|_| "ERROR".to_string());
                values.push(val);
            }
            rows.push(values);
        }

        Ok(rows)
    }

    /// Execute arbitrary SQL query with parameters, return rows as string vectors.
    pub fn query_raw_with_params(
        &self,
        sql: &str,
        params: &[rusqlite::types::Value],
    ) -> Result<Vec<Vec<String>>> {
        let mut stmt = self.conn.prepare(sql)?;
        let col_count = stmt.column_count();
        let mut rows = Vec::new();

        let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        let mut query_rows = stmt.query(param_refs.as_slice())?;
        while let Some(row) = query_rows.next()? {
            let mut values = Vec::new();
            for i in 0..col_count {
                let val: String = row
                    .get::<_, rusqlite::types::Value>(i)
                    .map(|v| match v {
                        rusqlite::types::Value::Null => "NULL".to_string(),
                        rusqlite::types::Value::Integer(i) => i.to_string(),
                        rusqlite::types::Value::Real(f) => f.to_string(),
                        rusqlite::types::Value::Text(s) => s,
                        rusqlite::types::Value::Blob(b) => format!("<blob:{} bytes>", b.len()),
                    })
                    .unwrap_or_else(|_| "ERROR".to_string());
                values.push(val);
            }
            rows.push(values);
        }

        Ok(rows)
    }

    /// Execute arbitrary SQL query, return column names and rows as string vectors.
    pub fn query_raw_with_columns(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        let mut stmt = self.conn.prepare(sql)?;
        let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let col_count = stmt.column_count();
        let mut rows = Vec::new();

        let mut query_rows = stmt.query([])?;
        while let Some(row) = query_rows.next()? {
            let mut values = Vec::new();
            for i in 0..col_count {
                let val: String = row
                    .get::<_, rusqlite::types::Value>(i)
                    .map(|v| match v {
                        rusqlite::types::Value::Null => "NULL".to_string(),
                        rusqlite::types::Value::Integer(i) => i.to_string(),
                        rusqlite::types::Value::Real(f) => f.to_string(),
                        rusqlite::types::Value::Text(s) => s,
                        rusqlite::types::Value::Blob(b) => format!("<blob:{} bytes>", b.len()),
                    })
                    .unwrap_or_else(|_| "ERROR".to_string());
                values.push(val);
            }
            rows.push(values);
        }

        Ok((columns, rows))
    }

    /// Find the path of a _typedef doogat by its title (type name).
    pub fn find_typedef_path(&self, type_name: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT path FROM doogats WHERE type = '_typedef' AND title = ?1",
            params![type_name],
            |row| row.get(0),
        );
        match result {
            Ok(path) => Ok(Some(path)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Execute a SQL statement with string parameters. Returns rows affected.
    pub fn execute_sql(&self, sql: &str, params: &[&str]) -> Result<usize> {
        let p: Vec<&dyn rusqlite::types::ToSql> = params
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();
        let count = self.conn.execute(sql, p.as_slice())?;
        Ok(count)
    }
}

impl crate::traits::DoogatIndex for Index {
    fn index_doogat(&self, doogat: &ParsedDoogat) -> Result<()> {
        self.index_doogat(doogat)
    }

    fn remove_doogat(&self, id: &str) -> Result<()> {
        self.remove_doogat(id)
    }

    fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        self.search(query)
    }

    fn search_paginated(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<PaginatedSearchResult> {
        self.search_paginated(query, limit, offset)
    }

    fn resolve_path(&self, id: &str) -> Result<String> {
        self.resolve_path(id)
    }

    fn query_raw(&self, sql: &str) -> Result<Vec<Vec<String>>> {
        self.query_raw(sql)
    }

    fn find_typedef_path(&self, type_name: &str) -> Result<Option<String>> {
        self.find_typedef_path(type_name)
    }

    fn execute_sql(&self, sql: &str, params: &[&str]) -> Result<usize> {
        self.execute_sql(sql, params)
    }
}

/// Recursively flatten a `Value` into `_ddb_fields` rows with dot-notation keys.
fn flatten_value_into_fields(
    conn: &rusqlite::Connection,
    id: &str,
    prefix: &str,
    value: &crate::types::Value,
) -> Result<()> {
    match value {
        crate::types::Value::String(s) => {
            conn.execute(
                "INSERT INTO _ddb_fields (doogat_id, key, value, zone) VALUES (?1, ?2, ?3, ?4)",
                params![id, prefix, s, "Frontmatter"],
            )?;
        }
        crate::types::Value::Number(n) => {
            conn.execute(
                "INSERT INTO _ddb_fields (doogat_id, key, value, zone) VALUES (?1, ?2, ?3, ?4)",
                params![id, prefix, n.to_string(), "Frontmatter"],
            )?;
        }
        crate::types::Value::Bool(b) => {
            conn.execute(
                "INSERT INTO _ddb_fields (doogat_id, key, value, zone) VALUES (?1, ?2, ?3, ?4)",
                params![id, prefix, b.to_string(), "Frontmatter"],
            )?;
        }
        crate::types::Value::Map(map) => {
            for (k, v) in map {
                // Escape dots and brackets in key names to avoid ambiguity with path separators
                let escaped = k
                    .replace('\\', "\\\\")
                    .replace('.', "\\.")
                    .replace('[', "\\[");
                let nested_key = format!("{prefix}.{escaped}");
                flatten_value_into_fields(conn, id, &nested_key, v)?;
            }
        }
        crate::types::Value::List(list) => {
            for (i, v) in list.iter().enumerate() {
                let nested_key = format!("{prefix}[{i}]");
                flatten_value_into_fields(conn, id, &nested_key, v)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
