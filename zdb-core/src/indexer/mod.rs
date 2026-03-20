mod graph;
mod materialize;
mod resolve;

use std::path::Path;

use rayon::prelude::*;
use rusqlite::{params, Connection};

use crate::error::{Result, ZettelError};
use crate::traits::ZettelSource;
use crate::types::ParsedZettel;

impl From<rusqlite::Error> for ZettelError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e.to_string())
    }
}

pub use crate::types::{PaginatedSearchResult, SearchResult};

pub struct Index {
    pub(crate) conn: Connection,
}

impl Index {
    /// Schema DDL for all internal tables. Kept in one place so `open` and
    /// `rebuild` (which drops everything first) use the same definitions.
    const SCHEMA_DDL: &str = "
        CREATE TABLE IF NOT EXISTS zettels (
            id TEXT PRIMARY KEY,
            title TEXT,
            date TEXT,
            type TEXT,
            path TEXT UNIQUE NOT NULL,
            body TEXT,
            updated_at TEXT
        );

        CREATE TABLE IF NOT EXISTS _zdb_tags (
            zettel_id TEXT NOT NULL REFERENCES zettels(id),
            tag TEXT NOT NULL,
            source TEXT NOT NULL DEFAULT 'frontmatter'
        );
        CREATE INDEX IF NOT EXISTS idx_zdb_tags_tag ON _zdb_tags(tag);

        CREATE TABLE IF NOT EXISTS _zdb_fields (
            zettel_id TEXT NOT NULL REFERENCES zettels(id),
            key TEXT NOT NULL,
            value TEXT,
            zone TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_zdb_fields_key ON _zdb_fields(key);

        CREATE TABLE IF NOT EXISTS _zdb_links (
            source_id TEXT NOT NULL REFERENCES zettels(id),
            target_path TEXT NOT NULL,
            display TEXT,
            zone TEXT,
            kind TEXT NOT NULL DEFAULT 'wikilink'
        );
        CREATE INDEX IF NOT EXISTS idx_zdb_links_target ON _zdb_links(target_path);

        CREATE TABLE IF NOT EXISTS _zdb_aliases (
            zettel_id TEXT NOT NULL REFERENCES zettels(id),
            alias TEXT COLLATE NOCASE NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_zdb_aliases_alias ON _zdb_aliases(alias);

        CREATE TABLE IF NOT EXISTS _zdb_meta (
            key TEXT PRIMARY KEY,
            value TEXT
        );

        CREATE TABLE IF NOT EXISTS _zdb_attachments (
            zettel_id TEXT NOT NULL REFERENCES zettels(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            mime TEXT,
            size INTEGER,
            path TEXT,
            PRIMARY KEY (zettel_id, name)
        );

        CREATE TABLE IF NOT EXISTS _zdb_checkboxes (
            zettel_id TEXT NOT NULL REFERENCES zettels(id),
            state TEXT NOT NULL CHECK (state IN ('open', 'done', 'info')),
            content TEXT NOT NULL,
            date TEXT,
            due_date TEXT,
            line_number INTEGER,
            indent_level INTEGER DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_zdb_checkboxes_state ON _zdb_checkboxes(state);
        CREATE INDEX IF NOT EXISTS idx_zdb_checkboxes_zettel ON _zdb_checkboxes(zettel_id);

        CREATE VIRTUAL TABLE IF NOT EXISTS _zdb_fts USING fts5(
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

    /// Upsert a single parsed zettel into the index (savepoint-wrapped).
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn index_zettel(&self, zettel: &ParsedZettel) -> Result<()> {
        self.with_savepoint("index_zettel", || self.upsert_zettel(zettel))
    }

    /// Index many zettels in a single transaction.
    ///
    /// Per-zettel errors are logged and skipped — they don't abort the batch.
    /// Returns the number of successfully indexed zettels.
    pub fn batch_index(&self, zettels: &[ParsedZettel]) -> Result<usize> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;

        let mut count = 0;
        for zettel in zettels {
            if let Err(e) = self.upsert_zettel(zettel) {
                tracing::warn!(path = %zettel.path, error = %e, "batch_index: skipping zettel");
                continue;
            }
            count += 1;
        }

        self.conn.execute_batch("COMMIT")?;
        Ok(count)
    }

    /// Shared upsert logic used by both `index_zettel` (savepoint) and `batch_index` (transaction).
    fn upsert_zettel(&self, zettel: &ParsedZettel) -> Result<()> {
        let id = zettel.meta.id.as_ref().map(|z| z.0.as_str()).unwrap_or("");
        let title = zettel.meta.title.as_deref().unwrap_or("");
        let date = zettel.meta.date.as_deref().unwrap_or("");
        let ztype = zettel.meta.zettel_type.as_deref().unwrap_or("");
        let now = chrono::Utc::now().to_rfc3339();
        let tags_str = zettel.meta.tags.join(", ");

        // Delete old FTS entry (no-op if zettel doesn't exist yet)
        self.conn.execute(
            "DELETE FROM _zdb_fts WHERE rowid = (SELECT rowid FROM zettels WHERE id = ?1)",
            params![id],
        )?;

        // Upsert zettel
        self.conn.execute(
            "INSERT OR REPLACE INTO zettels (id, title, date, type, path, body, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, title, date, ztype, zettel.path, zettel.body, now],
        )?;

        // Delete and reinsert related data
        self.conn
            .execute("DELETE FROM _zdb_tags WHERE zettel_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM _zdb_fields WHERE zettel_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM _zdb_links WHERE source_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM _zdb_aliases WHERE zettel_id = ?1", params![id])?;
        self.conn.execute(
            "DELETE FROM _zdb_checkboxes WHERE zettel_id = ?1",
            params![id],
        )?;

        for tag in &zettel.meta.tags {
            self.conn.execute(
                "INSERT INTO _zdb_tags (zettel_id, tag, source) VALUES (?1, ?2, 'frontmatter')",
                params![id, tag],
            )?;
        }

        for tag in &zettel.body_tags {
            self.conn.execute(
                "INSERT INTO _zdb_tags (zettel_id, tag, source) VALUES (?1, ?2, 'body')",
                params![id, tag],
            )?;
        }

        for cb in &zettel.checkboxes {
            let state = match cb.state {
                crate::types::CheckboxState::Open => "open",
                crate::types::CheckboxState::Done => "done",
                crate::types::CheckboxState::Info => "info",
            };
            self.conn.execute(
                "INSERT INTO _zdb_checkboxes (zettel_id, state, content, date, due_date, line_number, indent_level) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, state, cb.content, cb.date, cb.due_date, cb.line_number as i64, cb.indent_level as i64],
            )?;
        }

        for field in &zettel.inline_fields {
            let zone = format!("{:?}", field.zone);
            self.conn.execute(
                "INSERT INTO _zdb_fields (zettel_id, key, value, zone) VALUES (?1, ?2, ?3, ?4)",
                params![id, field.key, field.value, zone],
            )?;
        }

        // Insert frontmatter extras, flattening nested maps/lists into dot-notation keys
        for (key, value) in &zettel.meta.extra {
            let escaped = key
                .replace('\\', "\\\\")
                .replace('.', "\\.")
                .replace('[', "\\[");
            flatten_value_into_fields(&self.conn, id, &escaped, value)?;
        }

        for link in &zettel.links {
            let zone = format!("{:?}", link.zone);
            let kind = link.kind.as_str();
            self.conn.execute(
                "INSERT INTO _zdb_links (source_id, target_path, display, zone, kind) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, link.target, link.display, zone, kind],
            )?;
        }

        // Insert aliases
        if let Some(crate::types::Value::List(aliases)) = zettel.meta.extra.get("aliases") {
            for alias in aliases {
                if let crate::types::Value::String(a) = alias {
                    self.conn.execute(
                        "INSERT INTO _zdb_aliases (zettel_id, alias) VALUES (?1, ?2)",
                        params![id, a],
                    )?;
                }
            }
        }

        // Insert attachments
        self.conn.execute(
            "DELETE FROM _zdb_attachments WHERE zettel_id = ?1",
            params![id],
        )?;
        if let Some(crate::types::Value::List(items)) = zettel.meta.extra.get("attachments") {
            for item in items {
                if let crate::types::Value::Map(map) = item {
                    let name = map.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let mime = map.get("mime").and_then(|v| v.as_str()).unwrap_or("");
                    let size = map.get("size").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
                    let path = format!("reference/{}/{}", id, name);
                    self.conn.execute(
                        "INSERT INTO _zdb_attachments (zettel_id, name, mime, size, path) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![id, name, mime, size, path],
                    )?;
                }
            }
        }

        // Insert FTS entry
        self.conn.execute(
            "INSERT INTO _zdb_fts (rowid, title, body, tags) VALUES (
                (SELECT rowid FROM zettels WHERE id = ?1), ?2, ?3, ?4
            )",
            params![id, title, zettel.body, tags_str],
        )?;

        Ok(())
    }

    /// Remove a zettel from the index by ID.
    pub fn remove_zettel(&self, id: &str) -> Result<()> {
        self.with_savepoint("remove_zettel", || {
            self.conn.execute(
                "DELETE FROM _zdb_fts WHERE rowid = (SELECT rowid FROM zettels WHERE id = ?1)",
                params![id],
            )?;
            self.conn
                .execute("DELETE FROM _zdb_tags WHERE zettel_id = ?1", params![id])?;
            self.conn
                .execute("DELETE FROM _zdb_fields WHERE zettel_id = ?1", params![id])?;
            self.conn
                .execute("DELETE FROM _zdb_links WHERE source_id = ?1", params![id])?;
            self.conn
                .execute("DELETE FROM _zdb_aliases WHERE zettel_id = ?1", params![id])?;
            self.conn.execute(
                "DELETE FROM _zdb_checkboxes WHERE zettel_id = ?1",
                params![id],
            )?;
            self.conn
                .execute("DELETE FROM zettels WHERE id = ?1", params![id])?;
            Ok(())
        })
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
            "zettels",
            "_zdb_fts",
            "_zdb_tags",
            "_zdb_fields",
            "_zdb_links",
            "_zdb_aliases",
            "_zdb_checkboxes",
            "_zdb_meta",
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
    pub fn is_stale(&self, repo: &impl ZettelSource) -> Result<bool> {
        let current_head = repo.head_oid()?.to_string();
        let stored: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM _zdb_meta WHERE key = 'head'",
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
                "SELECT value FROM _zdb_meta WHERE key = 'head'",
                [],
                |row| row.get(0),
            )
            .ok()
    }

    /// Incremental reindex: only re-index zettels changed between old_head and current HEAD.
    /// Falls back to full rebuild if diff fails (e.g. old HEAD unreachable after gc).
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn incremental_reindex(
        &self,
        repo: &impl ZettelSource,
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
            // HEAD changed but no zettel files changed (e.g. config-only commit)
            self.conn.execute(
                "INSERT OR REPLACE INTO _zdb_meta (key, value) VALUES ('head', ?1)",
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
            self.remove_zettel(id)?;
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
            self.index_zettel(&parsed)?;
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
            "INSERT OR REPLACE INTO _zdb_meta (key, value) VALUES ('head', ?1)",
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
    /// Returns successfully parsed zettels and warnings for failures.
    /// Parse errors are collected, not propagated — one bad zettel doesn't block the rest.
    pub fn parallel_parse(
        repo: &impl ZettelSource,
        paths: &[String],
    ) -> Result<(Vec<ParsedZettel>, Vec<crate::types::ConsistencyWarning>)> {
        // Step 1: sequential git reads (optimal for pack I/O)
        let contents = repo.read_files_batch(paths)?;

        // Step 2: parallel parse (CPU-bound, benefits from rayon)
        let results: Vec<(String, std::result::Result<ParsedZettel, String>)> = contents
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
                    tracing::warn!(path = %path, error = %e, "parallel_parse: skipping zettel");
                    warnings
                        .push(crate::types::ConsistencyWarning::MalformedYaml { path, error: e });
                }
            }
        }

        Ok((parsed, warnings))
    }

    /// Rebuild entire index from all zettels in Git repo.
    /// Indexes all zettels first, collects warnings, then materializes typed tables.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn rebuild(&self, repo: &impl ZettelSource) -> Result<crate::types::RebuildReport> {
        tracing::info!("rebuild_triggered");

        // Drop and recreate all tables so schema changes take effect
        // without needing migrations — the index is a rebuildable cache.
        self.drop_all_tables()?;
        self.conn.execute_batch(Self::SCHEMA_DDL)?;

        let paths = repo.list_zettels()?;
        let mut report = crate::types::RebuildReport::default();

        // Phase 1: sequential git reads + parallel parsing (rayon)
        let (parsed, parse_warnings) = Self::parallel_parse(repo, &paths)?;
        report.warnings.extend(parse_warnings);

        // Phase 2: batch index all parsed zettels (single transaction)
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
            "INSERT OR REPLACE INTO _zdb_meta (key, value) VALUES ('head', ?1)",
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
        repo: &impl ZettelSource,
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
        self.search_hits(query, None)
    }

    /// Paginated full-text search with snippets, ranking, and total count.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn search_paginated(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<PaginatedSearchResult> {
        let hits = self.search_hits(query, Some((limit, offset)))?;

        let total_count: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM _zdb_fts WHERE _zdb_fts MATCH ?1",
            params![query],
            |row| row.get(0),
        )?;

        Ok(PaginatedSearchResult { hits, total_count })
    }

    fn search_hits(
        &self,
        query: &str,
        pagination: Option<(usize, usize)>,
    ) -> Result<Vec<SearchResult>> {
        let base = "SELECT z.id, z.title, z.path, \
                    snippet(_zdb_fts, 1, '<b>', '</b>', '...', 32), rank \
                    FROM _zdb_fts \
                    JOIN zettels z ON z.rowid = _zdb_fts.rowid \
                    WHERE _zdb_fts MATCH ?1 \
                    ORDER BY rank";
        let sql = match pagination {
            Some(_) => format!("{base} LIMIT ?2 OFFSET ?3"),
            None => base.to_string(),
        };

        let mut stmt = self.conn.prepare(&sql)?;

        let rows = match pagination {
            Some((limit, offset)) => stmt.query_map(
                params![query, limit as i64, offset as i64],
                Self::map_search_row,
            )?,
            None => stmt.query_map(params![query], Self::map_search_row)?,
        };

        let mut hits = Vec::new();
        for r in rows {
            hits.push(r?);
        }
        Ok(hits)
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

    /// Find zettels by hierarchical tag prefix.
    pub fn by_tag(&self, prefix: &str) -> Result<Vec<String>> {
        let pattern = format!("{prefix}%");
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT zettel_id FROM _zdb_tags WHERE tag LIKE ?1")?;
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

    /// Find the path of a _typedef zettel by its title (type name).
    pub fn find_typedef_path(&self, type_name: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT path FROM zettels WHERE type = '_typedef' AND title = ?1",
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

impl crate::traits::ZettelIndex for Index {
    fn index_zettel(&self, zettel: &ParsedZettel) -> Result<()> {
        self.index_zettel(zettel)
    }

    fn remove_zettel(&self, id: &str) -> Result<()> {
        self.remove_zettel(id)
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

/// Recursively flatten a `Value` into `_zdb_fields` rows with dot-notation keys.
fn flatten_value_into_fields(
    conn: &rusqlite::Connection,
    id: &str,
    prefix: &str,
    value: &crate::types::Value,
) -> Result<()> {
    match value {
        crate::types::Value::String(s) => {
            conn.execute(
                "INSERT INTO _zdb_fields (zettel_id, key, value, zone) VALUES (?1, ?2, ?3, ?4)",
                params![id, prefix, s, "Frontmatter"],
            )?;
        }
        crate::types::Value::Number(n) => {
            conn.execute(
                "INSERT INTO _zdb_fields (zettel_id, key, value, zone) VALUES (?1, ?2, ?3, ?4)",
                params![id, prefix, n.to_string(), "Frontmatter"],
            )?;
        }
        crate::types::Value::Bool(b) => {
            conn.execute(
                "INSERT INTO _zdb_fields (zettel_id, key, value, zone) VALUES (?1, ?2, ?3, ?4)",
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
mod tests {
    use super::*;
    use crate::git_ops::GitRepo;
    use crate::types::{InlineField, Link, Value, ZettelId, ZettelMeta, Zone};

    fn sample_zettel() -> ParsedZettel {
        ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260226120000".into())),
                title: Some("Test Note".into()),
                date: Some("2026-02-26".into()),
                zettel_type: Some("permanent".into()),
                tags: vec!["client/acme".into(), "test".into()],
                extra: Default::default(),
            },
            body: "Body with searchable content and [[20260101000000|Link]]".into(),
            sections: vec![],
            reference_section: "- source:: Wikipedia".into(),
            inline_fields: vec![InlineField {
                key: "source".into(),
                value: "Wikipedia".into(),
                zone: Zone::Reference,
            }],
            links: vec![Link {
                target: "20260101000000".into(),
                display: Some("Link".into()),
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: Zone::Body,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260226120000.md".into(),
        }
    }

    fn in_memory_index() -> Index {
        Index::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn schema_creation_idempotent() {
        let idx = in_memory_index();
        // Opening again should not error
        let _idx2 = Index::open(Path::new(":memory:")).unwrap();
        // Verify tables exist
        let count: i64 = idx
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='zettels'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    fn make_sample_zettels(n: usize) -> Vec<ParsedZettel> {
        (0..n)
            .map(|i| {
                let id = format!("{:014}", 20260226120000u64 + i as u64);
                ParsedZettel {
                    meta: ZettelMeta {
                        id: Some(ZettelId(id.clone())),
                        title: Some(format!("Note {i}")),
                        date: Some("2026-02-26".into()),
                        zettel_type: Some("permanent".into()),
                        tags: vec!["test".into()],
                        extra: Default::default(),
                    },
                    body: format!("Body of zettel {i}"),
                    sections: vec![],
                    reference_section: String::new(),
                    inline_fields: vec![],
                    links: vec![],
                    body_tags: vec![],
                    checkboxes: vec![],
                    path: format!("zettelkasten/{id}.md"),
                }
            })
            .collect()
    }

    fn dump_table(idx: &Index, table: &str) -> Vec<Vec<String>> {
        idx.query_raw(&format!("SELECT * FROM \"{table}\" ORDER BY 1"))
            .unwrap()
    }

    #[test]
    fn batch_index_matches_sequential() {
        let zettels = make_sample_zettels(10);

        // Sequential: index one-by-one
        let idx_seq = in_memory_index();
        for z in &zettels {
            idx_seq.index_zettel(z).unwrap();
        }

        // Batch: single transaction
        let idx_batch = in_memory_index();
        let count = idx_batch.batch_index(&zettels).unwrap();
        assert_eq!(count, 10);

        // Compare all tables
        for table in &[
            "zettels",
            "_zdb_tags",
            "_zdb_fields",
            "_zdb_links",
            "_zdb_aliases",
            "_zdb_checkboxes",
        ] {
            let seq_rows = dump_table(&idx_seq, table);
            let batch_rows = dump_table(&idx_batch, table);
            assert_eq!(
                seq_rows.len(),
                batch_rows.len(),
                "row count mismatch in {table}"
            );
            // Compare non-timestamp columns (updated_at varies)
            if *table == "zettels" {
                for (s, b) in seq_rows.iter().zip(batch_rows.iter()) {
                    // Compare all columns except updated_at (index 6)
                    assert_eq!(&s[..6], &b[..6], "zettels row mismatch");
                }
            } else {
                assert_eq!(seq_rows, batch_rows, "mismatch in {table}");
            }
        }

        // Verify FTS also works
        let seq_fts = idx_seq.search("Body").unwrap();
        let batch_fts = idx_batch.search("Body").unwrap();
        assert_eq!(seq_fts.len(), batch_fts.len());
    }

    #[test]
    fn parallel_parse_error_resilience() {
        use crate::traits::mock::MockSource;

        let mut source = MockSource::new();
        // 9 valid zettels
        for i in 0..9 {
            let id = format!("{:014}", 20260226120000u64 + i);
            let content =
                format!("---\nid: {id}\ntitle: Note {i}\ndate: 2026-02-26\n---\nBody {i}");
            source
                .files
                .insert(format!("zettelkasten/{id}.md"), content);
        }
        // 1 malformed zettel (invalid YAML frontmatter)
        source.files.insert(
            "zettelkasten/20260226129999.md".into(),
            "---\n: invalid yaml [\n---\nbody".into(),
        );

        let paths = source.list_zettels().unwrap();
        let (parsed, warnings) = Index::parallel_parse(&source, &paths).unwrap();

        assert_eq!(parsed.len(), 9);
        assert_eq!(warnings.len(), 1);
        assert!(matches!(
            &warnings[0],
            crate::types::ConsistencyWarning::MalformedYaml { path, .. }
            if path == "zettelkasten/20260226129999.md"
        ));
    }

    #[test]
    fn index_and_query_zettel() {
        let idx = in_memory_index();
        let z = sample_zettel();
        idx.index_zettel(&z).unwrap();

        // Query back
        let rows = idx.query_raw("SELECT id, title FROM zettels").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "20260226120000");
        assert_eq!(rows[0][1], "Test Note");
    }

    #[test]
    fn body_hashtags_indexed() {
        let idx = in_memory_index();
        let mut z = sample_zettel();
        z.body = "Some text #gtd/act/next here".into();
        z.body_tags = vec!["gtd/act/next".into()];
        idx.index_zettel(&z).unwrap();

        let rows = idx
            .query_raw("SELECT tag, source FROM _zdb_tags WHERE tag = 'gtd/act/next'")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "gtd/act/next");
        assert_eq!(rows[0][1], "body");
    }

    #[test]
    fn body_and_frontmatter_tags_unified() {
        let idx = in_memory_index();
        let mut z = sample_zettel();
        // sample_zettel has frontmatter tags: ["client/acme", "test"]
        z.body_tags = vec!["gtd/wait".into()];
        idx.index_zettel(&z).unwrap();

        let id = z.meta.id.as_ref().unwrap().0.as_str();
        let rows = idx
            .query_raw(&format!(
                "SELECT tag FROM _zdb_tags WHERE zettel_id = '{id}' ORDER BY tag"
            ))
            .unwrap();
        let tags: Vec<&str> = rows.iter().map(|r| r[0].as_str()).collect();
        assert!(tags.contains(&"client/acme"), "missing frontmatter tag");
        assert!(tags.contains(&"test"), "missing frontmatter tag");
        assert!(tags.contains(&"gtd/wait"), "missing body tag");
    }

    #[test]
    fn tag_source_column() {
        let idx = in_memory_index();
        let mut z = sample_zettel();
        z.body_tags = vec!["gtd/act/next".into()];
        idx.index_zettel(&z).unwrap();

        let id = z.meta.id.as_ref().unwrap().0.as_str();

        // Frontmatter tags have source='frontmatter'
        let rows = idx
            .query_raw(&format!(
                "SELECT source FROM _zdb_tags WHERE zettel_id = '{id}' AND tag = 'test'"
            ))
            .unwrap();
        assert_eq!(rows[0][0], "frontmatter");

        // Body tags have source='body'
        let rows = idx
            .query_raw(&format!(
                "SELECT source FROM _zdb_tags WHERE zettel_id = '{id}' AND tag = 'gtd/act/next'"
            ))
            .unwrap();
        assert_eq!(rows[0][0], "body");
    }

    #[test]
    fn checkboxes_indexed() {
        let idx = in_memory_index();
        let mut z = sample_zettel();
        z.body = "- [ ] open task\n- [x] done task\n- [i] 2026-01-01 10:00 - note".into();
        z.checkboxes = vec![
            crate::types::CheckboxItem {
                state: crate::types::CheckboxState::Open,
                content: "open task".into(),
                date: None,
                due_date: None,
                line_number: 1,
                indent_level: 0,
            },
            crate::types::CheckboxItem {
                state: crate::types::CheckboxState::Done,
                content: "done task".into(),
                date: None,
                due_date: None,
                line_number: 2,
                indent_level: 0,
            },
            crate::types::CheckboxItem {
                state: crate::types::CheckboxState::Info,
                content: "note".into(),
                date: Some("2026-01-01 10:00".into()),
                due_date: None,
                line_number: 3,
                indent_level: 0,
            },
        ];
        idx.index_zettel(&z).unwrap();

        let rows = idx
            .query_raw("SELECT state, content FROM _zdb_checkboxes ORDER BY line_number")
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0][0], "open");
        assert_eq!(rows[1][0], "done");
        assert_eq!(rows[2][0], "info");
    }

    #[test]
    fn checkbox_state_query() {
        let idx = in_memory_index();
        let mut z = sample_zettel();
        z.checkboxes = vec![
            crate::types::CheckboxItem {
                state: crate::types::CheckboxState::Open,
                content: "pending".into(),
                date: None,
                due_date: None,
                line_number: 1,
                indent_level: 0,
            },
            crate::types::CheckboxItem {
                state: crate::types::CheckboxState::Done,
                content: "finished".into(),
                date: None,
                due_date: None,
                line_number: 2,
                indent_level: 0,
            },
        ];
        idx.index_zettel(&z).unwrap();

        let open = idx
            .query_raw("SELECT content FROM _zdb_checkboxes WHERE state = 'open'")
            .unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0][0], "pending");
    }

    #[test]
    fn checkbox_reindex_state_change() {
        let idx = in_memory_index();
        let mut z = sample_zettel();

        // Initial: one open item
        z.checkboxes = vec![crate::types::CheckboxItem {
            state: crate::types::CheckboxState::Open,
            content: "buy milk".into(),
            date: None,
            due_date: None,
            line_number: 1,
            indent_level: 0,
        }];
        idx.index_zettel(&z).unwrap();

        let open = idx
            .query_raw("SELECT content FROM _zdb_checkboxes WHERE state = 'open'")
            .unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0][0], "buy milk");

        // Reindex with state changed to done
        z.checkboxes = vec![crate::types::CheckboxItem {
            state: crate::types::CheckboxState::Done,
            content: "buy milk".into(),
            date: None,
            due_date: None,
            line_number: 1,
            indent_level: 0,
        }];
        idx.index_zettel(&z).unwrap();

        let open = idx
            .query_raw("SELECT content FROM _zdb_checkboxes WHERE state = 'open'")
            .unwrap();
        assert_eq!(open.len(), 0);

        let done = idx
            .query_raw("SELECT content FROM _zdb_checkboxes WHERE state = 'done'")
            .unwrap();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0][0], "buy milk");
    }

    #[test]
    fn gtd_state_aggregated_from_all_zones() {
        let idx = in_memory_index();

        // Parse a zettel with GTD-relevant data in all zones:
        // - frontmatter: processed=true, tags=[gtd/ignore]
        // - body: checkboxes + hashtag #gtd/act/next
        let content = "\
---
id: 20260301120000
title: GTD Aggregation Test
date: 2026-03-01
tags:
  - gtd/ignore
processed: true
---
- [ ] task one #gtd/act/next
- [x] done task
- [i] info entry
---
- source:: Wikipedia";

        let parsed = crate::parser::parse(content, "zettelkasten/20260301120000.md").unwrap();
        idx.index_zettel(&parsed).unwrap();

        let id = "20260301120000";

        // Verify _zdb_fields has processed=true (from frontmatter extra)
        let fields = idx
            .query_raw(&format!(
                "SELECT key, value FROM _zdb_fields WHERE zettel_id = '{id}' AND key = 'processed'"
            ))
            .unwrap();
        assert_eq!(fields.len(), 1, "should have processed field");
        assert_eq!(fields[0][0], "processed");
        assert_eq!(fields[0][1], "true");

        // Verify _zdb_tags has both frontmatter tag and body hashtag
        let tags = idx
            .query_raw(&format!(
                "SELECT tag, source FROM _zdb_tags WHERE zettel_id = '{id}' ORDER BY tag"
            ))
            .unwrap();
        let tag_names: Vec<&str> = tags.iter().map(|r| r[0].as_str()).collect();
        assert!(
            tag_names.contains(&"gtd/ignore"),
            "should have frontmatter tag gtd/ignore: {tag_names:?}"
        );
        assert!(
            tag_names.contains(&"gtd/act/next"),
            "should have body hashtag gtd/act/next: {tag_names:?}"
        );

        // Verify sources are correct
        let fm_tag = tags.iter().find(|r| r[0] == "gtd/ignore").unwrap();
        assert_eq!(fm_tag[1], "frontmatter", "gtd/ignore should be from frontmatter");
        let body_tag = tags.iter().find(|r| r[0] == "gtd/act/next").unwrap();
        assert_eq!(body_tag[1], "body", "gtd/act/next should be from body");

        // Verify _zdb_checkboxes has 3 rows with correct states
        let checkboxes = idx
            .query_raw(&format!(
                "SELECT state, content FROM _zdb_checkboxes WHERE zettel_id = '{id}' ORDER BY line_number"
            ))
            .unwrap();
        assert_eq!(checkboxes.len(), 3, "should have 3 checkboxes");
        assert_eq!(checkboxes[0][0], "open", "first checkbox should be open");
        assert!(
            checkboxes[0][1].contains("task one"),
            "first checkbox content: {}",
            checkboxes[0][1]
        );
        assert_eq!(checkboxes[1][0], "done", "second checkbox should be done");
        assert!(
            checkboxes[1][1].contains("done task"),
            "second checkbox content: {}",
            checkboxes[1][1]
        );
        assert_eq!(checkboxes[2][0], "info", "third checkbox should be info");
        assert!(
            checkboxes[2][1].contains("info entry"),
            "third checkbox content: {}",
            checkboxes[2][1]
        );
    }

    #[test]
    fn fts_search() {
        let idx = in_memory_index();
        idx.index_zettel(&sample_zettel()).unwrap();

        let results = idx.search("searchable").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "20260226120000");
    }

    #[test]
    fn tag_prefix_query() {
        let idx = in_memory_index();
        idx.index_zettel(&sample_zettel()).unwrap();

        let ids = idx.by_tag("client/").unwrap();
        assert!(ids.contains(&"20260226120000".to_string()));

        let ids = idx.by_tag("test").unwrap();
        assert!(ids.contains(&"20260226120000".to_string()));

        let ids = idx.by_tag("nonexistent").unwrap();
        assert!(ids.is_empty());
    }

    #[test]
    fn index_all_link_kinds() {
        let idx = in_memory_index();
        let z = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260226120000".into())),
                title: Some("Mixed Links".into()),
                date: None,
                zettel_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![
                crate::types::Link {
                    target: "wiki_target".into(),
                    display: None,
                    section: None,
                    kind: crate::types::LinkKind::WikiLink,
                    zone: Zone::Body,
                },
                crate::types::Link {
                    target: "path.md".into(),
                    display: Some("title".into()),
                    section: None,
                    kind: crate::types::LinkKind::MarkdownLink,
                    zone: Zone::Body,
                },
                crate::types::Link {
                    target: "embed_file".into(),
                    display: None,
                    section: Some("sec".into()),
                    kind: crate::types::LinkKind::Embed,
                    zone: Zone::Body,
                },
                crate::types::Link {
                    target: "https://example.com".into(),
                    display: None,
                    section: None,
                    kind: crate::types::LinkKind::BareUrl,
                    zone: Zone::Body,
                },
            ],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260226120000.md".into(),
        };
        idx.index_zettel(&z).unwrap();

        let rows = idx
            .query_raw("SELECT target_path, kind FROM _zdb_links ORDER BY kind")
            .unwrap();
        assert_eq!(rows.len(), 4);
        let kinds: Vec<&str> = rows.iter().map(|r| r[1].as_str()).collect();
        assert!(kinds.contains(&"wikilink"));
        assert!(kinds.contains(&"markdown"));
        assert!(kinds.contains(&"embed"));
        assert!(kinds.contains(&"url"));
    }

    #[test]
    fn backlinks_include_all_link_kinds() {
        let idx = in_memory_index();

        // Target zettel
        let target = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301120000".into())),
                title: Some("Target".into()),
                date: None,
                zettel_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260301120000.md".into(),
        };
        idx.index_zettel(&target).unwrap();

        // Source zettel linking via all 4 kinds
        let source = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301100000".into())),
                title: Some("Source".into()),
                date: None,
                zettel_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![
                crate::types::Link {
                    target: "20260301120000".into(),
                    display: None,
                    section: None,
                    kind: crate::types::LinkKind::WikiLink,
                    zone: Zone::Body,
                },
                crate::types::Link {
                    target: "20260301120000".into(),
                    display: Some("t".into()),
                    section: None,
                    kind: crate::types::LinkKind::MarkdownLink,
                    zone: Zone::Body,
                },
                crate::types::Link {
                    target: "20260301120000".into(),
                    display: None,
                    section: None,
                    kind: crate::types::LinkKind::Embed,
                    zone: Zone::Body,
                },
            ],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260301100000.md".into(),
        };
        idx.index_zettel(&source).unwrap();

        // backlinks() returns the source regardless of link kind
        let bl = idx.backlinks("20260301120000").unwrap();
        assert_eq!(bl.len(), 1);
        assert_eq!(bl[0], "20260301100000");
    }

    #[test]
    fn backlink_query() {
        let idx = in_memory_index();
        idx.index_zettel(&sample_zettel()).unwrap();

        let ids = idx.backlinks("20260101000000").unwrap();
        assert!(ids.contains(&"20260226120000".to_string()));
    }

    #[test]
    fn query_raw_join() {
        let idx = in_memory_index();
        idx.index_zettel(&sample_zettel()).unwrap();

        let rows = idx.query_raw(
            "SELECT z.title, t.tag FROM zettels z JOIN _zdb_tags t ON t.zettel_id = z.id ORDER BY t.tag"
        ).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn upsert_replaces_old_data() {
        let idx = in_memory_index();
        let mut z = sample_zettel();
        idx.index_zettel(&z).unwrap();

        // Update title and tags
        z.meta.title = Some("Updated Title".into());
        z.meta.tags = vec!["newtag".into()];
        idx.index_zettel(&z).unwrap();

        let rows = idx
            .query_raw("SELECT title FROM zettels WHERE id = '20260226120000'")
            .unwrap();
        assert_eq!(rows[0][0], "Updated Title");

        let rows = idx
            .query_raw("SELECT COUNT(*) FROM _zdb_tags WHERE zettel_id = '20260226120000'")
            .unwrap();
        assert_eq!(rows[0][0], "1");
    }

    #[test]
    fn rebuild_and_staleness() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        let zettel_content =
            "---\nid: 20260226120000\ntitle: Rebuild Test\ntags:\n  - test\n---\nBody here.";
        repo.commit_file(
            "zettelkasten/20260226120000.md",
            zettel_content,
            "add zettel",
        )
        .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();

        // Initially stale (no head recorded)
        assert!(idx.is_stale(&repo).unwrap());

        let report = idx.rebuild(&repo).unwrap();
        assert_eq!(report.indexed, 1);

        // No longer stale
        assert!(!idx.is_stale(&repo).unwrap());

        // rebuild_if_stale should skip
        assert!(idx.rebuild_if_stale(&repo).unwrap().is_none());

        // After new commit, should be stale again
        repo.commit_file(
            "zettelkasten/20260226130000.md",
            "---\ntitle: New\n---\nNew body.",
            "add another",
        )
        .unwrap();
        assert!(idx.is_stale(&repo).unwrap());

        // Incremental reindex only processes changed files (1 new zettel)
        let report = idx.rebuild_if_stale(&repo).unwrap().unwrap();
        assert_eq!(report.indexed, 1);
    }

    #[test]
    fn rebuild_materializes_user_tables() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Create a typedef zettel
        let schema_content = "\
---
id: 20260226140000
title: items
type: _typedef
columns:
  - name: name
    data_type: TEXT
    zone: body
  - name: count
    data_type: INTEGER
    zone: frontmatter
---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260226140000.md",
            schema_content,
            "add typedef",
        )
        .unwrap();

        // Create a data zettel matching the schema
        let data_content = "\
---
id: 20260226140100
title: Widget
type: items
count: 42
---

## name

Widget
";
        repo.commit_file("zettelkasten/20260226140100.md", data_content, "add item")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();

        let report = idx.rebuild(&repo).unwrap();
        assert_eq!(report.indexed, 2);

        // Materialized table should exist and have data
        let rows = idx.query_raw("SELECT name, count FROM items").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "Widget");
        assert_eq!(rows[0][1], "42");
    }

    #[test]
    fn materialize_from_cached_matches_repo() {
        use crate::traits::mock::MockSource;

        // Typedef zettel
        let typedef_content = "\
---
id: 20260226140000
title: items
type: _typedef
columns:
  - name: name
    data_type: TEXT
    zone: body
  - name: count
    data_type: INTEGER
    zone: frontmatter
---\n";

        // Data zettel
        let data_content = "\
---
id: 20260226140100
title: Widget
type: items
count: 42
---

## name

Widget
";

        let mut source = MockSource::new();
        source.files.insert(
            "zettelkasten/_typedef/20260226140000.md".into(),
            typedef_content.into(),
        );
        source
            .files
            .insert("zettelkasten/20260226140100.md".into(), data_content.into());

        // Build parsed zettels
        let paths = source.list_zettels().unwrap();
        let parsed: Vec<ParsedZettel> = paths
            .iter()
            .map(|p| {
                let c = source.read_file(p).unwrap();
                crate::parser::parse(&c, p).unwrap()
            })
            .collect();

        // Path A: repo-based materialization
        let idx_repo = in_memory_index();
        idx_repo.batch_index(&parsed).unwrap();
        let (mat_a, inf_a) = idx_repo.materialize_all_types(&source).unwrap();

        // Path B: cached materialization
        let idx_cached = in_memory_index();
        idx_cached.batch_index(&parsed).unwrap();
        let (mat_b, inf_b) = idx_cached.materialize_all_types_from(&parsed).unwrap();

        assert_eq!(mat_a, mat_b);
        assert_eq!(inf_a, inf_b);

        // Compare materialized table contents
        let rows_a = idx_repo.query_raw("SELECT name, count FROM items").unwrap();
        let rows_b = idx_cached
            .query_raw("SELECT name, count FROM items")
            .unwrap();
        assert_eq!(rows_a, rows_b);
        assert_eq!(rows_a[0][0], "Widget");
        assert_eq!(rows_a[0][1], "42");
    }

    #[test]
    fn rebuild_deterministic_across_runs() {
        use crate::traits::mock::MockSource;

        let mut source = MockSource::new();
        // 20 zettels of mixed types
        for i in 0..15 {
            let id = format!("{:014}", 20260226120000u64 + i);
            let content = format!(
                "---\nid: {id}\ntitle: Note {i}\ndate: 2026-02-26\ntype: permanent\ntags:\n  - test\n---\nBody of {i}.\n---\n- source:: ref-{i}"
            );
            source
                .files
                .insert(format!("zettelkasten/{id}.md"), content);
        }
        for i in 0..5 {
            let id = format!("{:014}", 20260226130000u64 + i);
            let content = format!(
                "---\nid: {id}\ntitle: Task {i}\ndate: 2026-02-26\ntype: task\npriority: {i}\n---\nTask body {i}."
            );
            source
                .files
                .insert(format!("zettelkasten/{id}.md"), content);
        }

        // Rebuild twice into separate indexes
        let idx_a = in_memory_index();
        let report_a = idx_a.rebuild(&source).unwrap();

        let idx_b = in_memory_index();
        let report_b = idx_b.rebuild(&source).unwrap();

        assert_eq!(report_a.indexed, report_b.indexed);

        // Compare all core tables
        for table in &[
            "zettels",
            "_zdb_tags",
            "_zdb_fields",
            "_zdb_links",
            "_zdb_checkboxes",
        ] {
            let rows_a = dump_table(&idx_a, table);
            let rows_b = dump_table(&idx_b, table);
            if *table == "zettels" {
                // Skip updated_at column
                for (a, b) in rows_a.iter().zip(rows_b.iter()) {
                    assert_eq!(&a[..6], &b[..6], "zettels row mismatch");
                }
            } else {
                assert_eq!(rows_a, rows_b, "mismatch in {table}");
            }
        }

        // Verify FTS produces same results
        let fts_a = idx_a.search("Body").unwrap();
        let fts_b = idx_b.search("Body").unwrap();
        assert_eq!(fts_a.len(), fts_b.len());
    }

    #[test]
    fn infer_schema_frontmatter_types() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        let z1 = "---\nid: 20260226150000\ntitle: Task 1\ntype: task\npriority: 1\ndone: true\nscore: 3.5\n---\nBody.";
        let z2 = "---\nid: 20260226150100\ntitle: Task 2\ntype: task\npriority: 2\ndone: false\nscore: 7.0\n---\nBody.";
        repo.commit_file("zettelkasten/20260226150000.md", z1, "add task 1")
            .unwrap();
        repo.commit_file("zettelkasten/20260226150100.md", z2, "add task 2")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let schema = idx.infer_schema("task", &repo).unwrap();
        assert_eq!(schema.table_name, "task");

        let find = |name: &str| schema.columns.iter().find(|c| c.name == name);

        let done = find("done").expect("done column");
        assert_eq!(done.data_type, "BOOLEAN");
        assert_eq!(done.zone, Some(Zone::Frontmatter));

        let priority = find("priority").expect("priority column");
        assert_eq!(priority.data_type, "INTEGER");

        let score = find("score").expect("score column");
        assert_eq!(score.data_type, "REAL");
    }

    #[test]
    fn infer_schema_body_headings() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        let z1 = "---\nid: 20260226160000\ntitle: Note 1\ntype: article\n---\n\n## Summary\n\nSome text\n\n## Details\n\nMore text";
        repo.commit_file("zettelkasten/20260226160000.md", z1, "add article")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let schema = idx.infer_schema("article", &repo).unwrap();
        let find = |name: &str| schema.columns.iter().find(|c| c.name == name);

        let summary = find("summary").expect("summary column");
        assert_eq!(summary.data_type, "TEXT");
        assert_eq!(summary.zone, Some(Zone::Body));

        let details = find("details").expect("details column");
        assert_eq!(details.data_type, "TEXT");
        assert_eq!(details.zone, Some(Zone::Body));
    }

    #[test]
    fn infer_schema_ignores_code_block_headings() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        let z1 = "---\nid: 20260226160100\ntitle: Code\ntype: article\n---\n\n## Real\n\nContent\n\n```\n## Fake\ncode block\n```";
        repo.commit_file("zettelkasten/20260226160100.md", z1, "add")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let schema = idx.infer_schema("article", &repo).unwrap();
        let find = |name: &str| schema.columns.iter().find(|c| c.name == name);

        assert!(find("real").is_some(), "Real heading should be a column");
        assert!(
            find("fake").is_none(),
            "Code block heading should not be a column"
        );
    }

    #[test]
    fn infer_schema_reference_fields() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        let z1 = "---\nid: 20260226170000\ntitle: Proj 1\ntype: project\n---\n\nBody\n\n---\n\n- parent:: [[20260226170100]]\n- ticket:: JIRA-123";
        repo.commit_file("zettelkasten/20260226170000.md", z1, "add project")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let schema = idx.infer_schema("project", &repo).unwrap();
        let find = |name: &str| schema.columns.iter().find(|c| c.name == name);

        let parent = find("parent").expect("parent column");
        assert_eq!(parent.zone, Some(Zone::Reference));

        let ticket = find("ticket").expect("ticket column");
        assert_eq!(ticket.zone, Some(Zone::Reference));
    }

    #[test]
    fn infer_schema_empty_type() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.commit_file(
            "zettelkasten/20260226180000.md",
            "---\ntitle: Dummy\n---\nBody",
            "add",
        )
        .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let schema = idx.infer_schema("nonexistent", &repo).unwrap();
        assert!(schema.columns.is_empty());
        assert_eq!(schema.table_name, "nonexistent");
    }

    #[test]
    fn infer_schema_type_widening() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        let z1 = "---\nid: 20260226190000\ntitle: A\ntype: mixed\ncount: 5\n---\nBody.";
        let z2 = "---\nid: 20260226190100\ntitle: B\ntype: mixed\ncount: many\n---\nBody.";
        repo.commit_file("zettelkasten/20260226190000.md", z1, "add A")
            .unwrap();
        repo.commit_file("zettelkasten/20260226190100.md", z2, "add B")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let schema = idx.infer_schema("mixed", &repo).unwrap();
        let count = schema
            .columns
            .iter()
            .find(|c| c.name == "count")
            .expect("count column");
        assert_eq!(count.data_type, "TEXT");
    }

    #[test]
    fn infer_schema_case_variant_keys_deduplicated() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Frontmatter with case-variant keys: xP and xp
        let z1 = "---\nid: 20260226200000\ntitle: Dupe\ntype: dupe\nxP: a\nxp: A\n---\nBody.";
        repo.commit_file("zettelkasten/20260226200000.md", z1, "add dupe")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let schema = idx.infer_schema("dupe", &repo).unwrap();
        let xp_cols: Vec<_> = schema
            .columns
            .iter()
            .filter(|c| c.name.eq_ignore_ascii_case("xp"))
            .collect();
        assert_eq!(
            xp_cols.len(),
            1,
            "case-variant keys should merge into one column"
        );
        assert_eq!(xp_cols[0].name, "xp");
    }

    #[test]
    fn merge_schemas_typedef_only() {
        use crate::types::{ColumnDef, TableSchema};

        let typedef = TableSchema {
            table_name: "foo".to_string(),
            columns: vec![
                ColumnDef {
                    name: "a".into(),
                    data_type: "TEXT".into(),
                    references: None,
                    zone: Some(Zone::Body),
                    required: false,
                    search_boost: None,
                    allowed_values: None,
                    default_value: None,
                },
                ColumnDef {
                    name: "b".into(),
                    data_type: "INTEGER".into(),
                    references: None,
                    zone: Some(Zone::Frontmatter),
                    required: true,
                    search_boost: None,
                    allowed_values: None,
                    default_value: None,
                },
            ],
            crdt_strategy: Some("preset:default".into()),
            template_sections: vec!["A".into()],
            folder: false,
            stale_after_days: None,
        };
        let inferred = TableSchema {
            table_name: "foo".to_string(),
            columns: vec![],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
        };

        let merged = Index::merge_schemas(Some(typedef), inferred);
        assert_eq!(merged.columns.len(), 2);
        assert_eq!(merged.crdt_strategy, Some("preset:default".to_string()));
    }

    #[test]
    fn merge_schemas_inferred_only() {
        use crate::types::{ColumnDef, TableSchema};

        let inferred = TableSchema {
            table_name: "bar".to_string(),
            columns: vec![ColumnDef {
                name: "x".into(),
                data_type: "INTEGER".into(),
                references: None,
                zone: Some(Zone::Frontmatter),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
            }],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
        };

        let merged = Index::merge_schemas(None, inferred);
        assert_eq!(merged.columns.len(), 1);
        assert_eq!(merged.table_name, "bar");
    }

    #[test]
    fn merge_schemas_overlap() {
        use crate::types::{ColumnDef, TableSchema};

        let typedef = TableSchema {
            table_name: "baz".to_string(),
            columns: vec![ColumnDef {
                name: "shared".into(),
                data_type: "INTEGER".into(),
                references: None,
                zone: Some(Zone::Frontmatter),
                required: true,
                search_boost: Some(2.0),
                allowed_values: None,
                default_value: None,
            }],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
        };
        let inferred = TableSchema {
            table_name: "baz".to_string(),
            columns: vec![
                ColumnDef {
                    name: "shared".into(),
                    data_type: "TEXT".into(),
                    references: None,
                    zone: Some(Zone::Body),
                    required: false,
                    search_boost: None,
                    allowed_values: None,
                    default_value: None,
                },
                ColumnDef {
                    name: "extra".into(),
                    data_type: "TEXT".into(),
                    references: None,
                    zone: Some(Zone::Body),
                    required: false,
                    search_boost: None,
                    allowed_values: None,
                    default_value: None,
                },
            ],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
        };

        let merged = Index::merge_schemas(Some(typedef), inferred);
        assert_eq!(merged.columns.len(), 2);
        let shared = merged.columns.iter().find(|c| c.name == "shared").unwrap();
        assert_eq!(shared.data_type, "INTEGER");
        assert!(shared.required);
        assert!(merged.columns.iter().any(|c| c.name == "extra"));
    }

    #[test]
    fn merge_schemas_no_overlap() {
        use crate::types::{ColumnDef, TableSchema};

        let typedef = TableSchema {
            table_name: "qux".to_string(),
            columns: vec![ColumnDef {
                name: "a".into(),
                data_type: "TEXT".into(),
                references: None,
                zone: Some(Zone::Body),
                required: false,
                search_boost: None,
                allowed_values: None,
                default_value: None,
            }],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
        };
        let inferred = TableSchema {
            table_name: "qux".to_string(),
            columns: vec![
                ColumnDef {
                    name: "b".into(),
                    data_type: "INTEGER".into(),
                    references: None,
                    zone: Some(Zone::Frontmatter),
                    required: false,
                    search_boost: None,
                    allowed_values: None,
                    default_value: None,
                },
                ColumnDef {
                    name: "c".into(),
                    data_type: "REAL".into(),
                    references: None,
                    zone: Some(Zone::Frontmatter),
                    required: false,
                    search_boost: None,
                    allowed_values: None,
                    default_value: None,
                },
            ],
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
        };

        let merged = Index::merge_schemas(Some(typedef), inferred);
        assert_eq!(merged.columns.len(), 3);
    }

    #[test]
    fn consistency_warnings_valid_zettel() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        let z = "---\nid: 20260226200000\ntitle: Valid\ntype: note\n---\nBody text.";
        repo.commit_file("zettelkasten/20260226200000.md", z, "add")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let warnings = idx.collect_consistency_warnings(&repo);
        assert!(warnings.is_empty());
    }

    #[test]
    fn consistency_warnings_missing_required() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        let typedef_content = "---\nid: 20260226210000\ntitle: task\ntype: _typedef\ncolumns:\n  - name: priority\n    data_type: INTEGER\n    zone: frontmatter\n    required: true\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260226210000.md",
            typedef_content,
            "add typedef",
        )
        .unwrap();

        let z = "---\nid: 20260226210100\ntitle: My Task\ntype: task\n---\nBody.";
        repo.commit_file("zettelkasten/20260226210100.md", z, "add task")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let warnings = idx.collect_consistency_warnings(&repo);
        assert!(!warnings.is_empty());
        let has_missing = warnings.iter().any(|w| matches!(w,
            crate::types::ConsistencyWarning::MissingRequired { field, .. } if field == "priority"
        ));
        assert!(
            has_missing,
            "should warn about missing required 'priority' field"
        );
    }

    #[test]
    fn integration_inferred_type_full_cycle() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Create zettels with type "foo" — no _typedef exists
        let z1 = "---\nid: 20260226220000\ntitle: Foo 1\ntype: foo\npriority: 3\n---\n\n## Description\n\nFirst foo\n\n---\n\n- owner:: [[20260226220100]]";
        let z2 = "---\nid: 20260226220100\ntitle: Foo 2\ntype: foo\npriority: 7\n---\n\n## Description\n\nSecond foo\n\n---\n\n- owner:: [[20260226220000]]";
        repo.commit_file("zettelkasten/20260226220000.md", z1, "add foo 1")
            .unwrap();
        repo.commit_file("zettelkasten/20260226220100.md", z2, "add foo 2")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        let report = idx.rebuild(&repo).unwrap();

        // Table "foo" should exist with inferred columns
        assert!(report.types_inferred.contains(&"foo".to_string()));
        assert!(report.tables_materialized > 0);

        // SELECT should return data
        let rows = idx
            .query_raw("SELECT id, priority FROM foo ORDER BY id")
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][1], "3");
        assert_eq!(rows[1][1], "7");
    }

    #[test]
    fn integration_typedef_plus_inferred_merge() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Create typedef with 2 columns
        let typedef = "---\nid: 20260226230000\ntitle: widget\ntype: _typedef\ncolumns:\n  - name: weight\n    data_type: REAL\n    zone: frontmatter\n  - name: color\n    data_type: TEXT\n    zone: frontmatter\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260226230000.md",
            typedef,
            "add typedef",
        )
        .unwrap();

        // Create zettel with 3 extra fields (2 from typedef + 1 new)
        let z = "---\nid: 20260226230100\ntitle: Red Widget\ntype: widget\nweight: 2.5\ncolor: red\nsize: large\n---\n\nBody";
        repo.commit_file("zettelkasten/20260226230100.md", z, "add widget")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        // Table should have 3 columns (2 typedef + 1 inferred "size")
        let rows = idx
            .query_raw("SELECT weight, color, size FROM widget")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "2.5");
        assert_eq!(rows[0][1], "red");
        assert_eq!(rows[0][2], "large");
    }

    #[test]
    fn integration_external_edit_reconciliation() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Initial zettel with type "doc" and one field
        let z1 = "---\nid: 20260226240000\ntitle: Doc 1\ntype: doc\nversion: 1\n---\nBody";
        repo.commit_file("zettelkasten/20260226240000.md", z1, "add doc")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        // Externally add a zettel with a new field
        let z2 = "---\nid: 20260226240100\ntitle: Doc 2\ntype: doc\nversion: 2\nauthor: Alice\n---\nBody";
        repo.commit_file("zettelkasten/20260226240100.md", z2, "add doc externally")
            .unwrap();

        // Rebuild picks up new fields
        let report = idx.rebuild(&repo).unwrap();
        assert_eq!(report.indexed, 2);

        // Table should now have "author" column from inferred merge
        let rows = idx
            .query_raw("SELECT id, author FROM doc WHERE author != ''")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][1], "Alice");
    }

    #[test]
    fn integration_consistency_warnings_in_rebuild() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Create typedef with required field
        let typedef = "---\nid: 20260226250000\ntitle: strict\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n    required: true\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260226250000.md",
            typedef,
            "add typedef",
        )
        .unwrap();

        // Create zettel missing required field
        let z = "---\nid: 20260226250100\ntitle: Incomplete\ntype: strict\n---\nBody";
        repo.commit_file("zettelkasten/20260226250100.md", z, "add incomplete")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        let report = idx.rebuild(&repo).unwrap();

        // Should have warnings but still index
        assert!(!report.warnings.is_empty());
        assert_eq!(report.indexed, 2); // typedef + data zettel both indexed

        // Data should still be accessible
        let rows = idx
            .query_raw("SELECT id FROM zettels WHERE type = 'strict'")
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn rebuild_via_mock_source() {
        use crate::traits::mock::MockSource;

        let mut source = MockSource::new();
        source.files.insert(
            "zettelkasten/20260226120000.md".into(),
            "---\ntitle: Mock Note\ntype: permanent\ntags:\n  - test\n---\nBody text.\n".into(),
        );
        source.files.insert(
            "zettelkasten/20260226120001.md".into(),
            "---\ntitle: Second Note\ntype: permanent\n---\nMore text.\n".into(),
        );

        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let idx = Index::open(&db_path).unwrap();
        let report = idx.rebuild(&source).unwrap();

        assert_eq!(report.indexed, 2);
        assert!(!idx.is_stale(&source).unwrap());

        let results = idx.search("Mock").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Mock Note");
    }

    #[test]
    fn infer_schema_via_mock_source() {
        use crate::traits::mock::MockSource;

        let mut source = MockSource::new();
        source.files.insert(
            "zettelkasten/20260226120000.md".into(),
            "---\ntitle: Project A\ntype: project\npriority: 1\nactive: true\n---\n## Notes\nSome notes.\n".into(),
        );

        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&source).unwrap();

        let schema = idx.infer_schema("project", &source).unwrap();
        let col_names: Vec<&str> = schema.columns.iter().map(|c| c.name.as_str()).collect();
        assert!(col_names.contains(&"priority"));
        assert!(col_names.contains(&"active"));
        assert!(col_names.contains(&"notes"));
    }

    #[test]
    fn check_integrity_healthy_db() {
        let idx = in_memory_index();
        assert!(idx.check_integrity().unwrap());
    }

    #[test]
    fn check_integrity_missing_table() {
        // Open a fresh db without the schema setup — simulate partial corruption
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE zettels (id TEXT PRIMARY KEY)")
            .unwrap();
        drop(conn);

        // Open via Index — schema creates missing tables, but let's test
        // a scenario where we drop a table after open
        let idx = Index::open(&db_path).unwrap();
        idx.conn.execute_batch("DROP TABLE _zdb_fts").unwrap();
        assert!(!idx.check_integrity().unwrap());
    }

    #[test]
    fn alias_indexed_and_resolved() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        let mut extra = std::collections::BTreeMap::new();
        extra.insert(
            "aliases".to_string(),
            crate::types::Value::List(vec![
                crate::types::Value::String("My Project".to_string()),
                crate::types::Value::String("proj-x".to_string()),
            ]),
        );

        let zettel = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId("20240101120000".to_string())),
                title: Some("Project X".to_string()),
                date: Some("2024-01-01".to_string()),
                zettel_type: None,
                tags: vec![],
                extra,
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20240101120000.md".to_string(),
        };

        index.index_zettel(&zettel).unwrap();

        // Resolve by alias
        assert_eq!(
            index.resolve_alias("My Project").unwrap(),
            Some("20240101120000".to_string())
        );
        assert_eq!(
            index.resolve_alias("proj-x").unwrap(),
            Some("20240101120000".to_string())
        );
        // Case-insensitive
        assert_eq!(
            index.resolve_alias("my project").unwrap(),
            Some("20240101120000".to_string())
        );
        // No match
        assert_eq!(index.resolve_alias("nonexistent").unwrap(), None);
    }

    #[test]
    fn alias_removed_on_zettel_delete() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        let mut extra = std::collections::BTreeMap::new();
        extra.insert(
            "aliases".to_string(),
            crate::types::Value::List(vec![crate::types::Value::String("alias1".to_string())]),
        );

        let zettel = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId("20240101120000".to_string())),
                title: Some("Test".to_string()),
                date: None,
                zettel_type: None,
                tags: vec![],
                extra,
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20240101120000.md".to_string(),
        };

        index.index_zettel(&zettel).unwrap();
        assert!(index.resolve_alias("alias1").unwrap().is_some());

        index.remove_zettel("20240101120000").unwrap();
        assert_eq!(index.resolve_alias("alias1").unwrap(), None);
    }

    #[test]
    fn wikilink_resolves_via_alias() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        let mut extra = std::collections::BTreeMap::new();
        extra.insert(
            "aliases".to_string(),
            crate::types::Value::List(vec![crate::types::Value::String("My Note".to_string())]),
        );

        let zettel = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId("20240101120000".to_string())),
                title: Some("Note".to_string()),
                date: None,
                zettel_type: None,
                tags: vec![],
                extra,
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20240101120000.md".to_string(),
        };

        index.index_zettel(&zettel).unwrap();

        // Resolves via ID
        let result = index.resolve_wikilink("20240101120000").unwrap();
        assert_eq!(result, Some("zettelkasten/20240101120000.md".to_string()));

        // Resolves via alias
        let result = index.resolve_wikilink("My Note").unwrap();
        assert_eq!(result, Some("zettelkasten/20240101120000.md".to_string()));

        // No match
        let result = index.resolve_wikilink("nonexistent").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_wikilink_path_takes_precedence() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        // Zettel A: its *path* is the collision target
        let zettel_a = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId("20240101120000".to_string())),
                title: Some("Contact A".to_string()),
                date: None,
                zettel_type: Some("contact".to_string()),
                tags: vec![],
                extra: std::collections::BTreeMap::new(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/contact/20240101120000.md".to_string(),
        };

        // Zettel B: its *ID* equals A's full path — contrived but tests precedence
        let zettel_b = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId(
                    "zettelkasten/contact/20240101120000.md".to_string(),
                )),
                title: Some("Zettel B".to_string()),
                date: None,
                zettel_type: None,
                tags: vec![],
                extra: std::collections::BTreeMap::new(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20240202120000.md".to_string(),
        };

        index.index_zettel(&zettel_a).unwrap();
        index.index_zettel(&zettel_b).unwrap();

        // Target matches A's path AND B's ID — path lookup must win
        let result = index
            .resolve_wikilink("zettelkasten/contact/20240101120000.md")
            .unwrap();
        assert_eq!(
            result,
            Some("zettelkasten/contact/20240101120000.md".to_string()),
            "path lookup should take precedence over ID lookup"
        );

        // Bare ID still resolves via ID fallback (step 2)
        let result = index.resolve_wikilink("20240101120000").unwrap();
        assert_eq!(
            result,
            Some("zettelkasten/contact/20240101120000.md".to_string())
        );

        // Nonexistent returns None
        let result = index.resolve_wikilink("nonexistent").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn resolve_partial_path_basic() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.commit_file(
            "zettelkasten/meeting-notes.md",
            "---\nid: 20260301000000\ntitle: Meeting Notes\n---\n",
            "add",
        )
        .unwrap();
        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let result = idx.resolve_wikilink("meeting-notes").unwrap();
        assert_eq!(result, Some("zettelkasten/meeting-notes.md".into()));
    }

    #[test]
    fn resolve_partial_path_nested() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.commit_file(
            "zettelkasten/projects/meeting-notes.md",
            "---\nid: 20260301000000\ntitle: Meeting Notes\n---\n",
            "add",
        )
        .unwrap();
        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let result = idx.resolve_wikilink("meeting-notes").unwrap();
        assert_eq!(
            result,
            Some("zettelkasten/projects/meeting-notes.md".into())
        );
    }

    #[test]
    fn resolve_partial_path_ambiguous_shortest_wins() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.commit_file(
            "zettelkasten/meeting-notes.md",
            "---\nid: 20260301000000\ntitle: Short\n---\n",
            "add short",
        )
        .unwrap();
        repo.commit_file(
            "zettelkasten/projects/acme/meeting-notes.md",
            "---\nid: 20260301000001\ntitle: Long\n---\n",
            "add long",
        )
        .unwrap();
        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let result = idx.resolve_wikilink("meeting-notes").unwrap();
        assert_eq!(result, Some("zettelkasten/meeting-notes.md".into()));
    }

    #[test]
    fn resolve_partial_path_with_md_suffix() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        repo.commit_file(
            "zettelkasten/meeting-notes.md",
            "---\nid: 20260301000000\ntitle: Notes\n---\n",
            "add",
        )
        .unwrap();
        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let result = idx.resolve_wikilink("meeting-notes.md").unwrap();
        assert_eq!(result, Some("zettelkasten/meeting-notes.md".into()));
    }

    #[test]
    fn resolve_partial_path_no_match() {
        let idx = in_memory_index();
        let result = idx.resolve_wikilink("nonexistent-thing").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn schema_parses_allowed_values_and_default() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        let typedef = "---\nid: 20260301100000\ntitle: task\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n    allowed_values:\n      - todo\n      - doing\n      - done\n    default_value: todo\n  - name: priority\n    data_type: TEXT\n    zone: frontmatter\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260301100000.md",
            typedef,
            "add typedef",
        )
        .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let schemas = idx.load_all_typedefs(&repo);
        let schema = schemas.get("task").unwrap();
        let status_col = schema.columns.iter().find(|c| c.name == "status").unwrap();
        assert_eq!(
            status_col.allowed_values.as_ref().unwrap(),
            &["todo", "doing", "done"]
        );
        assert_eq!(status_col.default_value.as_deref(), Some("todo"));

        let priority_col = schema
            .columns
            .iter()
            .find(|c| c.name == "priority")
            .unwrap();
        assert!(priority_col.allowed_values.is_none());
        assert!(priority_col.default_value.is_none());
    }

    #[test]
    fn materialize_emits_check_constraint() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        let typedef = "---\nid: 20260301100100\ntitle: task\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n    allowed_values:\n      - todo\n      - doing\n      - done\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260301100100.md",
            typedef,
            "add typedef",
        )
        .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        // Verify CHECK constraint exists by reading table info
        let sql = idx
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='task'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        assert!(sql.contains("CHECK"), "expected CHECK constraint in: {sql}");
        assert!(sql.contains("'todo'"));
        assert!(sql.contains("'doing'"));
        assert!(sql.contains("'done'"));
    }

    fn make_zettel(n: usize) -> ParsedZettel {
        ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId(format!("2026022612{n:04}"))),
                title: Some(format!("Note {n}")),
                date: Some("2026-02-26".into()),
                zettel_type: Some("permanent".into()),
                tags: vec!["test".into()],
                extra: Default::default(),
            },
            body: format!("Searchable body number {n}"),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: format!("zettelkasten/2026022612{n:04}.md"),
        }
    }

    #[test]
    fn paginated_search_basic() {
        let idx = in_memory_index();
        for i in 0..30 {
            idx.index_zettel(&make_zettel(i)).unwrap();
        }

        let result = idx.search_paginated("searchable", 10, 0).unwrap();
        assert_eq!(result.hits.len(), 10);
        assert_eq!(result.total_count, 30);
    }

    #[test]
    fn paginated_search_offset_beyond() {
        let idx = in_memory_index();
        for i in 0..5 {
            idx.index_zettel(&make_zettel(i)).unwrap();
        }

        let result = idx.search_paginated("searchable", 10, 100).unwrap();
        assert!(result.hits.is_empty());
        assert_eq!(result.total_count, 5);
    }

    #[test]
    fn paginated_search_no_results() {
        let idx = in_memory_index();
        idx.index_zettel(&make_zettel(0)).unwrap();

        let result = idx.search_paginated("nonexistent", 10, 0).unwrap();
        assert!(result.hits.is_empty());
        assert_eq!(result.total_count, 0);
    }

    #[test]
    fn search_returns_same_hits_as_paginated() {
        let idx = in_memory_index();
        for i in 0..5 {
            idx.index_zettel(&make_zettel(i)).unwrap();
        }

        let results = idx.search("searchable").unwrap();
        let paginated = idx.search_paginated("searchable", usize::MAX, 0).unwrap();

        assert_eq!(results.len(), 5);
        assert_eq!(results.len(), paginated.hits.len());
        assert_eq!(
            results.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            paginated
                .hits
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn attachments_indexed_and_queried() {
        let idx = in_memory_index();
        let mut extra = std::collections::BTreeMap::new();
        extra.insert(
            "attachments".into(),
            Value::List(vec![
                Value::Map({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert("name".into(), Value::String("photo.jpg".into()));
                    m.insert("mime".into(), Value::String("image/jpeg".into()));
                    m.insert("size".into(), Value::Number(1024.0));
                    m
                }),
                Value::Map({
                    let mut m = std::collections::BTreeMap::new();
                    m.insert("name".into(), Value::String("doc.pdf".into()));
                    m.insert("mime".into(), Value::String("application/pdf".into()));
                    m.insert("size".into(), Value::Number(2048.0));
                    m
                }),
            ]),
        );
        let zettel = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301130000".into())),
                title: Some("Test".into()),
                extra,
                ..Default::default()
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260301130000.md".into(),
        };
        idx.index_zettel(&zettel).unwrap();

        let rows: Vec<(String, String, String, i64)> = idx
            .conn
            .prepare("SELECT zettel_id, name, mime, size FROM _zdb_attachments ORDER BY name")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1, "doc.pdf");
        assert_eq!(rows[1].1, "photo.jpg");
        assert_eq!(rows[1].3, 1024);
    }

    #[test]
    fn incremental_reindex_only_processes_changed_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Create 3 zettels
        repo.commit_file(
            "zettelkasten/20240101000000.md",
            "---\ntitle: A\n---\nBody A.",
            "add a",
        )
        .unwrap();
        repo.commit_file(
            "zettelkasten/20240102000000.md",
            "---\ntitle: B\n---\nBody B.",
            "add b",
        )
        .unwrap();
        repo.commit_file(
            "zettelkasten/20240103000000.md",
            "---\ntitle: C\n---\nBody C.",
            "add c",
        )
        .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        let report = idx.rebuild(&repo).unwrap();
        assert_eq!(report.indexed, 3);

        // Modify one zettel
        repo.commit_file(
            "zettelkasten/20240101000000.md",
            "---\ntitle: A Modified\n---\nBody A modified.",
            "modify a",
        )
        .unwrap();
        let old_head = idx.stored_head_oid().unwrap();

        let report = idx.incremental_reindex(&repo, &old_head).unwrap();
        assert_eq!(report.indexed, 1); // Only the modified file

        // Verify the modification is reflected
        let rows = idx
            .query_raw("SELECT title FROM zettels WHERE id = '20240101000000'")
            .unwrap();
        assert_eq!(rows[0][0], "A Modified");
    }

    #[test]
    fn incremental_reindex_handles_deletes() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        repo.commit_file(
            "zettelkasten/20240101000000.md",
            "---\ntitle: A\n---\nBody A.",
            "add a",
        )
        .unwrap();
        repo.commit_file(
            "zettelkasten/20240102000000.md",
            "---\ntitle: B\n---\nBody B.",
            "add b",
        )
        .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        // Delete one zettel
        repo.delete_file("zettelkasten/20240102000000.md", "delete b")
            .unwrap();
        let old_head = idx.stored_head_oid().unwrap();

        let report = idx.incremental_reindex(&repo, &old_head).unwrap();
        assert_eq!(report.indexed, 0); // No adds/modifies

        // Verify deletion
        let rows = idx
            .query_raw("SELECT id FROM zettels WHERE id = '20240102000000'")
            .unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn incremental_reindex_fallback_on_bad_oid() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        repo.commit_file(
            "zettelkasten/20240101000000.md",
            "---\ntitle: A\n---\nBody A.",
            "add a",
        )
        .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();

        // Use a fake old HEAD — should fall back to full rebuild
        let report = idx
            .incremental_reindex(&repo, "0000000000000000000000000000000000000000")
            .unwrap();
        assert_eq!(report.indexed, 1); // Full rebuild found 1 zettel
    }

    #[test]
    fn incremental_batch_mode_multi_change() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Create 5 zettels
        for i in 0..5 {
            repo.commit_file(
                &format!("zettelkasten/2024010{i}000000.md"),
                &format!("---\ntitle: Note {i}\n---\nBody {i}."),
                &format!("add {i}"),
            )
            .unwrap();
        }

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();
        let old_head = idx.stored_head_oid().unwrap();

        // Modify 3 zettels in a single commit
        let modifications: Vec<(&str, &str)> = vec![
            (
                "zettelkasten/20240100000000.md",
                "---\ntitle: Modified 0\n---\nUpdated body 0.",
            ),
            (
                "zettelkasten/20240101000000.md",
                "---\ntitle: Modified 1\n---\nUpdated body 1.",
            ),
            (
                "zettelkasten/20240102000000.md",
                "---\ntitle: Modified 2\n---\nUpdated body 2.",
            ),
        ];
        repo.commit_files(&modifications, "modify 3").unwrap();

        let report = idx.incremental_reindex(&repo, &old_head).unwrap();
        assert_eq!(report.indexed, 3);

        // Verify modifications
        let rows = idx
            .query_raw("SELECT title FROM zettels WHERE id = '20240100000000'")
            .unwrap();
        assert_eq!(rows[0][0], "Modified 0");

        let rows = idx
            .query_raw("SELECT title FROM zettels WHERE id = '20240102000000'")
            .unwrap();
        assert_eq!(rows[0][0], "Modified 2");
    }

    #[test]
    fn typedef_change_triggers_rematerialization() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Create a typedef
        let typedef = "---\nid: 20260301100100\ntitle: task\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260301100100.md",
            typedef,
            "add typedef",
        )
        .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();
        let old_head = idx.stored_head_oid().unwrap();

        // Modify the typedef (add a column)
        let typedef2 = "---\nid: 20260301100100\ntitle: task\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n  - name: priority\n    data_type: INTEGER\n    zone: frontmatter\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260301100100.md",
            typedef2,
            "modify typedef",
        )
        .unwrap();

        let report = idx.incremental_reindex(&repo, &old_head).unwrap();
        assert!(
            report.tables_materialized > 0,
            "typedef change should trigger rematerialization"
        );
    }

    #[test]
    fn resurrected_zettel_not_duplicated_after_reindex() {
        let idx = in_memory_index();
        let mut z = sample_zettel();
        z.meta
            .extra
            .insert("resurrected".into(), crate::types::Value::Bool(true));
        idx.index_zettel(&z).unwrap();
        // Reindex same zettel
        idx.index_zettel(&z).unwrap();

        let id = z.meta.id.as_ref().unwrap().0.as_str();
        let count: i64 = idx
            .conn
            .query_row(
                "SELECT COUNT(*) FROM zettels WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Also verify the resurrected field isn't duplicated
        let field_count: i64 = idx
            .conn
            .query_row(
                "SELECT COUNT(*) FROM _zdb_fields WHERE zettel_id = ?1 AND key = 'resurrected'",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(field_count, 1);
    }

    #[test]
    fn resurrected_zettels_query() {
        let idx = in_memory_index();

        // Zettel with resurrected: true
        let mut z1 = sample_zettel();
        z1.meta.extra.insert(
            "resurrected".into(),
            crate::types::Value::String("true".into()),
        );
        idx.index_zettel(&z1).unwrap();

        // Normal zettel without resurrected
        let z2 = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260302120000".into())),
                title: Some("Normal".into()),
                date: None,
                zettel_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260302120000.md".into(),
        };
        idx.index_zettel(&z2).unwrap();

        let results = idx.resurrected_zettels().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, z1.meta.id.as_ref().unwrap().0);
        assert_eq!(results[0].1, "Test Note");
    }

    #[test]
    fn resurrected_zettels_empty_when_none() {
        let idx = in_memory_index();
        let z = sample_zettel();
        idx.index_zettel(&z).unwrap();
        assert!(idx.resurrected_zettels().unwrap().is_empty());
    }

    #[test]
    fn frontmatter_extras_indexed_as_fields() {
        let idx = in_memory_index();
        let mut z = sample_zettel();
        z.meta
            .extra
            .insert("resurrected".into(), crate::types::Value::Bool(true));
        z.meta
            .extra
            .insert("priority".into(), crate::types::Value::Number(3.0));
        z.meta.extra.insert(
            "source_url".into(),
            crate::types::Value::String("https://example.com".into()),
        );
        idx.index_zettel(&z).unwrap();

        let id = z.meta.id.as_ref().unwrap().0.as_str();
        let rows: Vec<(String, String, String)> = idx
            .conn
            .prepare("SELECT key, value, zone FROM _zdb_fields WHERE zettel_id = ?1 AND zone = 'Frontmatter'")
            .unwrap()
            .query_map(params![id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();

        assert!(rows
            .iter()
            .any(|(k, v, _)| k == "resurrected" && v == "true"));
        assert!(rows.iter().any(|(k, v, _)| k == "priority" && v == "3"));
        assert!(rows
            .iter()
            .any(|(k, v, _)| k == "source_url" && v == "https://example.com"));
        // List/Map extras should NOT appear
        assert!(!rows
            .iter()
            .any(|(k, _, _)| k == "aliases" || k == "attachments"));
    }

    #[test]
    fn backlinking_zettel_paths_returns_source_id_and_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        // Zettel A links to target B
        let zettel_a = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId("20260301100000".to_string())),
                title: Some("A".to_string()),
                date: None,
                zettel_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: "See [[20260301120000]]".to_string(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![crate::types::Link {
                target: "20260301120000".to_string(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: crate::types::Zone::Body,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260301100000.md".to_string(),
        };

        // Zettel B is the target (no outgoing links)
        let zettel_b = crate::types::ParsedZettel {
            meta: crate::types::ZettelMeta {
                id: Some(crate::types::ZettelId("20260301120000".to_string())),
                title: Some("B".to_string()),
                date: None,
                zettel_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260301120000.md".to_string(),
        };

        index.index_zettel(&zettel_a).unwrap();
        index.index_zettel(&zettel_b).unwrap();

        let results = index.backlinking_zettel_paths("20260301120000").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "20260301100000");
        assert_eq!(results[0].1, "zettelkasten/20260301100000.md");

        // No backlinks for A
        let empty = index.backlinking_zettel_paths("20260301100000").unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn broken_backlinks_after_delete() {
        let index = in_memory_index();

        // Create target zettel A
        let a = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301100000".into())),
                title: Some("Target".into()),
                date: None,
                zettel_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260301100000.md".into(),
        };

        // Create zettel B that links to A
        let b = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301100001".into())),
                title: Some("Linker".into()),
                date: None,
                zettel_type: None,
                tags: vec![],
                extra: Default::default(),
            },
            body: "See [[20260301100000]]".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![Link {
                target: "20260301100000".into(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: Zone::Body,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260301100001.md".into(),
        };

        index.index_zettel(&a).unwrap();
        index.index_zettel(&b).unwrap();

        // No broken backlinks yet
        let broken = index.broken_backlinks().unwrap();
        assert!(broken.is_empty());

        // Delete A
        index.remove_zettel("20260301100000").unwrap();

        // B's link to A is now broken
        let broken = index.broken_backlinks().unwrap();
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].0, "20260301100001");
        assert_eq!(broken[0].1, "20260301100000");
    }

    #[test]
    fn concurrent_read_during_write() {
        // Simulates widget/extension reading index while host app writes.
        // Two Index instances on the same DB — WAL + busy_timeout must handle this.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("index.db");

        let writer = Index::open(&db_path).unwrap();

        // Index a zettel via the writer
        let zettel = sample_zettel();
        writer.index_zettel(&zettel).unwrap();

        // Open a second read-only connection (simulates widget process)
        let reader = Index::open(&db_path).unwrap();

        // Reader sees the zettel written by writer
        let results = reader.search("searchable").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "20260226120000");

        // Writer can still write while reader is open
        let zettel2 = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260226120001".into())),
                title: Some("Second Note".into()),
                date: Some("2026-02-26".into()),
                zettel_type: Some("permanent".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "Another body".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260226120001.md".into(),
        };
        writer.index_zettel(&zettel2).unwrap();

        // Reader sees both zettels (WAL allows concurrent read + write)
        let all = reader
            .conn
            .prepare("SELECT id FROM zettels ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn concurrent_readers_no_contention() {
        // Multiple simultaneous readers should never block each other.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("index.db");

        let writer = Index::open(&db_path).unwrap();
        let zettel = sample_zettel();
        writer.index_zettel(&zettel).unwrap();

        // Open three concurrent readers
        let r1 = Index::open(&db_path).unwrap();
        let r2 = Index::open(&db_path).unwrap();
        let r3 = Index::open(&db_path).unwrap();

        // All three read successfully
        assert_eq!(r1.search("searchable").unwrap().len(), 1);
        assert_eq!(r2.search("searchable").unwrap().len(), 1);
        assert_eq!(r3.search("searchable").unwrap().len(), 1);
    }

    #[test]
    fn busy_timeout_is_set() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("index.db");
        let index = Index::open(&db_path).unwrap();

        let timeout: i64 = index
            .conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(timeout, 5000);
    }

    // ── unlinked_mentions tests ─────────────────────────────────────

    #[test]
    fn unlinked_mentions_basic() {
        let idx = in_memory_index();

        // Zettel A: title "Project Alpha"
        let a = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301000000".into())),
                title: Some("Project Alpha".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "This is Project Alpha.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260301000000.md".into(),
        };

        // Zettel B: body mentions "Project Alpha" but does NOT link to A
        let b = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260301000001".into())),
                title: Some("Meeting Notes".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "Discussed Project Alpha progress today.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260301000001.md".into(),
        };

        idx.index_zettel(&a).unwrap();
        idx.index_zettel(&b).unwrap();

        let mentions = idx.unlinked_mentions("20260301000000").unwrap();
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].source_id, "20260301000001");
    }

    #[test]
    fn unlinked_mentions_excludes_linked() {
        let idx = in_memory_index();

        // Zettel A
        let a = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260302000000".into())),
                title: Some("Project Beta".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "This is Project Beta.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260302000000.md".into(),
        };

        // Zettel B: mentions "Project Beta" AND links to A via wikilink
        let b = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260302000001".into())),
                title: Some("Status Update".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "Project Beta is on track. See [[20260302000000]].".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![Link {
                target: "20260302000000".into(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: Zone::Body,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260302000001.md".into(),
        };

        idx.index_zettel(&a).unwrap();
        idx.index_zettel(&b).unwrap();

        let mentions = idx.unlinked_mentions("20260302000000").unwrap();
        assert!(
            mentions.is_empty(),
            "linked zettel should not appear in unlinked mentions"
        );
    }

    #[test]
    fn unlinked_mentions_excludes_self() {
        let idx = in_memory_index();

        // Zettel whose body mentions its own title
        let a = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260303000000".into())),
                title: Some("Self Reference".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "This zettel is about Self Reference patterns.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260303000000.md".into(),
        };

        idx.index_zettel(&a).unwrap();

        let mentions = idx.unlinked_mentions("20260303000000").unwrap();
        assert!(
            mentions.is_empty(),
            "zettel should not appear in its own unlinked mentions"
        );
    }

    // ── suggest_links tests ─────────────────────────────────────────

    #[test]
    fn suggest_links_tag_overlap() {
        let idx = in_memory_index();

        // Source: tags [a, b, c]
        let source = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260304000000".into())),
                title: Some("Source".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec!["a".into(), "b".into(), "c".into()],
                extra: Default::default(),
            },
            body: "Source body.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260304000000.md".into(),
        };

        // Candidate1: tags [a, b] — 2 shared tags
        let c1 = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260304000001".into())),
                title: Some("Candidate One".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec!["a".into(), "b".into()],
                extra: Default::default(),
            },
            body: "Candidate one body.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260304000001.md".into(),
        };

        // Candidate2: tags [a] — 1 shared tag
        let c2 = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260304000002".into())),
                title: Some("Candidate Two".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec!["a".into()],
                extra: Default::default(),
            },
            body: "Candidate two body.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260304000002.md".into(),
        };

        idx.index_zettel(&source).unwrap();
        idx.index_zettel(&c1).unwrap();
        idx.index_zettel(&c2).unwrap();

        let suggestions = idx.suggest_links("20260304000000", 10).unwrap();
        assert!(
            suggestions.len() >= 2,
            "should suggest at least 2 candidates"
        );

        // Candidate1 (2 shared tags) should rank higher than candidate2 (1 shared tag)
        let pos_c1 = suggestions.iter().position(|s| s.id == "20260304000001");
        let pos_c2 = suggestions.iter().position(|s| s.id == "20260304000002");
        assert!(
            pos_c1.unwrap() < pos_c2.unwrap(),
            "candidate with more shared tags should rank higher"
        );
    }

    #[test]
    fn suggest_links_excludes_linked() {
        let idx = in_memory_index();

        // Source links to candidate
        let source = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260305000000".into())),
                title: Some("Source".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec!["shared".into()],
                extra: Default::default(),
            },
            body: "Source body with [[20260305000001]].".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![Link {
                target: "20260305000001".into(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: Zone::Body,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260305000000.md".into(),
        };

        // Candidate: same tag as source
        let candidate = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260305000001".into())),
                title: Some("Candidate".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec!["shared".into()],
                extra: Default::default(),
            },
            body: "Candidate body.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260305000001.md".into(),
        };

        idx.index_zettel(&source).unwrap();
        idx.index_zettel(&candidate).unwrap();

        let suggestions = idx.suggest_links("20260305000000", 10).unwrap();
        assert!(
            !suggestions.iter().any(|s| s.id == "20260305000001"),
            "already-linked zettel should be excluded from suggestions"
        );
    }

    #[test]
    fn suggest_links_respects_limit() {
        let idx = in_memory_index();

        // Source with tags
        let source = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260306000000".into())),
                title: Some("Source".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec!["common".into()],
                extra: Default::default(),
            },
            body: "Source body.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260306000000.md".into(),
        };
        idx.index_zettel(&source).unwrap();

        // Create 5 candidates all sharing the tag
        for i in 1..=5 {
            let id = format!("2026030600000{i}");
            let c = ParsedZettel {
                meta: ZettelMeta {
                    id: Some(ZettelId(id.clone())),
                    title: Some(format!("Candidate {i}")),
                    date: None,
                    zettel_type: Some("note".into()),
                    tags: vec!["common".into()],
                    extra: Default::default(),
                },
                body: format!("Candidate {i} body."),
                sections: vec![],
                reference_section: String::new(),
                inline_fields: vec![],
                links: vec![],
                body_tags: vec![],
                checkboxes: vec![],
                path: format!("zettelkasten/{id}.md"),
            };
            idx.index_zettel(&c).unwrap();
        }

        let suggestions = idx.suggest_links("20260306000000", 2).unwrap();
        assert!(
            suggestions.len() <= 2,
            "should respect limit of 2, got {}",
            suggestions.len()
        );
    }

    #[test]
    fn suggest_links_content_similarity() {
        let idx = in_memory_index();

        // Zettel A: no tags, title "Machine Learning"
        let a = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260314000000".into())),
                title: Some("Machine Learning".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "An overview of ML techniques.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260314000000.md".into(),
        };

        // Zettel B: no shared tags, body contains "machine learning"
        let b = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260314000001".into())),
                title: Some("Deep Learning".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "This explores machine learning algorithms and neural networks.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260314000001.md".into(),
        };

        idx.index_zettel(&a).unwrap();
        idx.index_zettel(&b).unwrap();

        // A has no tags, so suggest_links falls back to content-only similarity.
        // B's body contains "machine learning" which matches A's title via FTS5.
        let suggestions = idx.suggest_links("20260314000000", 5).unwrap();
        assert!(
            suggestions.iter().any(|s| s.id == "20260314000001"),
            "B should appear via content similarity; got: {:?}",
            suggestions.iter().map(|s| &s.id).collect::<Vec<_>>()
        );
    }

    // ── stale_zettels tests ─────────────────────────────────────────

    /// Helper: commit a file with a custom git timestamp (epoch seconds).
    fn commit_file_with_time(
        repo: &GitRepo,
        rel_path: &str,
        content: &str,
        message: &str,
        epoch_secs: i64,
    ) {
        let full_path = repo.path.join(rel_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full_path, content).unwrap();

        let git_repo = &repo.repo;
        let mut index = git_repo.index().unwrap();
        index.add_path(std::path::Path::new(rel_path)).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = git_repo.find_tree(tree_oid).unwrap();

        let sig = git2::Signature::new("zdb", "zdb@test", &git2::Time::new(epoch_secs, 0)).unwrap();

        let parents: Vec<git2::Commit<'_>> = match git_repo.head() {
            Ok(head) => vec![head.peel_to_commit().unwrap()],
            Err(_) => vec![],
        };
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();

        git_repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .unwrap();
    }

    #[test]
    fn stale_zettels_basic() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Create a _typedef with stale_after_days: 1
        let typedef =
            "---\nid: 20260307000000\ntitle: task\ntype: _typedef\nstale_after_days: 1\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260307000000.md",
            typedef,
            "add typedef",
        )
        .unwrap();

        // Create a zettel of type "task" with an OLD git commit time (2020-01-01)
        let zettel =
            "---\nid: 20260307000001\ntitle: Old Task\ntype: task\ndate: 2020-01-01\n---\nBody.";
        commit_file_with_time(
            &repo,
            "zettelkasten/20260307000001.md",
            zettel,
            "add old task",
            1577836800, // 2020-01-01T00:00:00 UTC
        );

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let stale = idx.stale_zettels(&repo, None).unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id, "20260307000001");
        assert_eq!(stale[0].zettel_type, "task");
    }

    #[test]
    fn stale_zettels_respects_type() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // Type A: stale_after_days: 1
        let typedef_a =
            "---\nid: 20260313000000\ntitle: taskA\ntype: _typedef\nstale_after_days: 1\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260313000000.md",
            typedef_a,
            "add typedef A",
        )
        .unwrap();

        // Type B: stale_after_days: 1
        let typedef_b =
            "---\nid: 20260313000001\ntitle: taskB\ntype: _typedef\nstale_after_days: 1\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260313000001.md",
            typedef_b,
            "add typedef B",
        )
        .unwrap();

        // Zettel of type A with old git commit time
        let zettel_a = "---\nid: 20260313000002\ntitle: Old A\ntype: taskA\n---\nBody A.";
        commit_file_with_time(
            &repo,
            "zettelkasten/20260313000002.md",
            zettel_a,
            "add old A",
            1577836800, // 2020-01-01
        );

        // Zettel of type B with old git commit time
        let zettel_b = "---\nid: 20260313000003\ntitle: Old B\ntype: taskB\n---\nBody B.";
        commit_file_with_time(
            &repo,
            "zettelkasten/20260313000003.md",
            zettel_b,
            "add old B",
            1577836800, // 2020-01-01
        );

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        // Filter by type A — only type A zettel should be returned
        let stale = idx.stale_zettels(&repo, Some("taskA")).unwrap();
        assert_eq!(stale.len(), 1, "should return exactly one stale zettel");
        assert_eq!(stale[0].id, "20260313000002");
        assert_eq!(stale[0].zettel_type, "taskA");

        // Unfiltered — both should appear
        let all_stale = idx.stale_zettels(&repo, None).unwrap();
        assert_eq!(
            all_stale.len(),
            2,
            "unfiltered should return both stale zettels"
        );
    }

    #[test]
    fn stale_zettels_no_threshold() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();

        // _typedef without stale_after_days
        let typedef = "---\nid: 20260308000000\ntitle: note\ntype: _typedef\n---\n";
        repo.commit_file(
            "zettelkasten/_typedef/20260308000000.md",
            typedef,
            "add typedef",
        )
        .unwrap();

        // Zettel of type "note" with old date
        let zettel =
            "---\nid: 20260308000001\ntitle: Old Note\ntype: note\ndate: 2020-01-01\n---\nBody.";
        repo.commit_file("zettelkasten/20260308000001.md", zettel, "add note")
            .unwrap();

        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let idx = Index::open(&db_path).unwrap();
        idx.rebuild(&repo).unwrap();

        let stale = idx.stale_zettels(&repo, None).unwrap();
        assert!(
            stale.is_empty(),
            "type without stale_after_days should not report stale zettels"
        );
    }

    // ── orphan_zettels tests ────────────────────────────────────────

    #[test]
    fn orphan_zettels_basic() {
        let idx = in_memory_index();

        // Zettel with no incoming links
        let orphan = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260309000000".into())),
                title: Some("Orphan".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "Nobody links to me.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260309000000.md".into(),
        };
        idx.index_zettel(&orphan).unwrap();

        let orphans = idx.orphan_zettels(None).unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].id, "20260309000000");
    }

    #[test]
    fn orphan_zettels_excludes_linked() {
        let idx = in_memory_index();

        // Zettel B: target of a link
        let b = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260310000001".into())),
                title: Some("Linked Target".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "I have an incoming link.".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260310000001.md".into(),
        };

        // Zettel A: links to B
        let a = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260310000000".into())),
                title: Some("Linker".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "See [[20260310000001]].".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![Link {
                target: "20260310000001".into(),
                display: None,
                section: None,
                kind: crate::types::LinkKind::WikiLink,
                zone: Zone::Body,
            }],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260310000000.md".into(),
        };

        idx.index_zettel(&b).unwrap();
        idx.index_zettel(&a).unwrap();

        let orphans = idx.orphan_zettels(None).unwrap();
        assert!(
            !orphans.iter().any(|o| o.id == "20260310000001"),
            "zettel with incoming link should not be an orphan"
        );
    }

    #[test]
    fn orphan_zettels_excludes_typedef() {
        let idx = in_memory_index();

        // _typedef zettel (no incoming links)
        let typedef = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260311000000".into())),
                title: Some("task".into()),
                date: None,
                zettel_type: Some("_typedef".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/_typedef/20260311000000.md".into(),
        };
        idx.index_zettel(&typedef).unwrap();

        let orphans = idx.orphan_zettels(None).unwrap();
        assert!(
            !orphans.iter().any(|o| o.id == "20260311000000"),
            "_typedef zettels should never appear in orphan results"
        );
    }

    #[test]
    fn orphan_zettels_includes_outgoing_count() {
        let idx = in_memory_index();

        // Orphan zettel with 2 outgoing links (but no incoming)
        let orphan = ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId("20260312000000".into())),
                title: Some("Orphan With Links".into()),
                date: None,
                zettel_type: Some("note".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: "Links to [[20260312000001]] and [[20260312000002]].".into(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![
                Link {
                    target: "20260312000001".into(),
                    display: None,
                    section: None,
                    kind: crate::types::LinkKind::WikiLink,
                    zone: Zone::Body,
                },
                Link {
                    target: "20260312000002".into(),
                    display: None,
                    section: None,
                    kind: crate::types::LinkKind::WikiLink,
                    zone: Zone::Body,
                },
            ],
            body_tags: vec![],
            checkboxes: vec![],
            path: "zettelkasten/20260312000000.md".into(),
        };
        idx.index_zettel(&orphan).unwrap();

        let orphans = idx.orphan_zettels(None).unwrap();
        let found = orphans.iter().find(|o| o.id == "20260312000000");
        assert!(found.is_some(), "orphan should be returned");
        assert_eq!(found.unwrap().outgoing_links, 2);
    }

    // ── Sequence tests ──────────────────────────────────────────────

    fn seq_zettel(id: &str, title: &str, parent: Option<&str>) -> ParsedZettel {
        let mut extra = std::collections::BTreeMap::new();
        if let Some(pid) = parent {
            extra.insert("sequence".into(), Value::String(pid.into()));
        }
        ParsedZettel {
            meta: ZettelMeta {
                id: Some(ZettelId(id.into())),
                title: Some(title.into()),
                date: None,
                zettel_type: None,
                tags: vec![],
                extra,
            },
            body: String::new(),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: format!("zettelkasten/{id}.md"),
        }
    }

    #[test]
    fn sequence_children_basic() {
        let idx = in_memory_index();
        let parent = seq_zettel("20260315100000", "Root", None);
        let child1 = seq_zettel("20260315100001", "Child A", Some("20260315100000"));
        let child2 = seq_zettel("20260315100002", "Child B", Some("20260315100000"));
        idx.index_zettel(&parent).unwrap();
        idx.index_zettel(&child1).unwrap();
        idx.index_zettel(&child2).unwrap();

        let children = idx.sequence_children("20260315100000").unwrap();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].id, "20260315100001");
        assert_eq!(children[1].id, "20260315100002");
    }

    #[test]
    fn sequence_children_empty() {
        let idx = in_memory_index();
        let z = seq_zettel("20260315110000", "Standalone", None);
        idx.index_zettel(&z).unwrap();

        let children = idx.sequence_children("20260315110000").unwrap();
        assert!(children.is_empty());
    }

    #[test]
    fn sequence_breadcrumb_chain() {
        let idx = in_memory_index();
        let root = seq_zettel("20260315120000", "Root", None);
        let mid = seq_zettel("20260315120001", "Mid", Some("20260315120000"));
        let leaf = seq_zettel("20260315120002", "Leaf", Some("20260315120001"));
        idx.index_zettel(&root).unwrap();
        idx.index_zettel(&mid).unwrap();
        idx.index_zettel(&leaf).unwrap();

        let bc = idx.sequence_breadcrumb("20260315120002").unwrap();
        assert_eq!(bc.len(), 3);
        assert_eq!(bc[0].id, "20260315120000");
        assert_eq!(bc[1].id, "20260315120001");
        assert_eq!(bc[2].id, "20260315120002");
    }

    #[test]
    fn sequence_breadcrumb_root() {
        let idx = in_memory_index();
        let root = seq_zettel("20260315130000", "Root", None);
        idx.index_zettel(&root).unwrap();

        let bc = idx.sequence_breadcrumb("20260315130000").unwrap();
        assert_eq!(bc.len(), 1);
        assert_eq!(bc[0].id, "20260315130000");
    }

    #[test]
    fn sequence_breadcrumb_cycle() {
        let idx = in_memory_index();
        let a = seq_zettel("20260315140000", "A", Some("20260315140001"));
        let b = seq_zettel("20260315140001", "B", Some("20260315140000"));
        idx.index_zettel(&a).unwrap();
        idx.index_zettel(&b).unwrap();

        let bc = idx.sequence_breadcrumb("20260315140000").unwrap();
        // Should not hang; returns partial chain
        assert!(bc.len() <= 3);
    }

    #[test]
    fn sequence_info_complete() {
        let idx = in_memory_index();
        let root = seq_zettel("20260315150000", "Root", None);
        let mid = seq_zettel("20260315150001", "Mid", Some("20260315150000"));
        let child1 = seq_zettel("20260315150002", "Child C", Some("20260315150001"));
        let child2 = seq_zettel("20260315150003", "Child D", Some("20260315150001"));
        idx.index_zettel(&root).unwrap();
        idx.index_zettel(&mid).unwrap();
        idx.index_zettel(&child1).unwrap();
        idx.index_zettel(&child2).unwrap();

        let info = idx.sequence_info("20260315150001").unwrap();
        assert!(info.parent.is_some());
        assert_eq!(info.parent.unwrap().id, "20260315150000");
        assert_eq!(info.children.len(), 2);
        assert_eq!(info.children[0].id, "20260315150002");
        assert_eq!(info.breadcrumb.len(), 2);
        assert_eq!(info.breadcrumb[0].id, "20260315150000");
        assert_eq!(info.breadcrumb[1].id, "20260315150001");
    }

    #[test]
    fn broken_sequence_detected() {
        let idx = in_memory_index();
        let z = seq_zettel("20260315160000", "Orphan", Some("99999999999999"));
        idx.index_zettel(&z).unwrap();

        let broken = idx.broken_sequences().unwrap();
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].zettel_id, "20260315160000");
        assert_eq!(broken[0].broken_parent_id, "99999999999999");
    }

    #[test]
    fn broken_sequence_clean() {
        let idx = in_memory_index();
        let root = seq_zettel("20260315170000", "Root", None);
        let child = seq_zettel("20260315170001", "Child", Some("20260315170000"));
        idx.index_zettel(&root).unwrap();
        idx.index_zettel(&child).unwrap();

        let broken = idx.broken_sequences().unwrap();
        assert!(broken.is_empty());
    }

    #[test]
    fn sequence_tree_recursive() {
        let idx = in_memory_index();
        let root = seq_zettel("20260315180000", "Root", None);
        let mid = seq_zettel("20260315180001", "Mid", Some("20260315180000"));
        let leaf = seq_zettel("20260315180002", "Leaf", Some("20260315180001"));
        idx.index_zettel(&root).unwrap();
        idx.index_zettel(&mid).unwrap();
        idx.index_zettel(&leaf).unwrap();

        let tree = idx.sequence_tree("20260315180000", 100).unwrap();
        assert_eq!(tree.len(), 3);
        assert_eq!(tree[0].0.id, "20260315180000");
        assert_eq!(tree[0].1, 0);
        assert_eq!(tree[1].0.id, "20260315180001");
        assert_eq!(tree[1].1, 1);
        assert_eq!(tree[2].0.id, "20260315180002");
        assert_eq!(tree[2].1, 2);
    }

    #[test]
    fn sequence_breadcrumb_broken_parent() {
        let idx = in_memory_index();
        // Zettel points to nonexistent parent
        let z = seq_zettel("20260315190000", "Orphan", Some("99999999999999"));
        idx.index_zettel(&z).unwrap();

        let bc = idx.sequence_breadcrumb("20260315190000").unwrap();
        // Should return just self, not a phantom node for the missing parent
        assert_eq!(bc.len(), 1);
        assert_eq!(bc[0].id, "20260315190000");
    }
}
