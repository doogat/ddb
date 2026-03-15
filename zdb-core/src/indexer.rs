use std::path::Path;

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
            tag TEXT NOT NULL
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
            zone TEXT
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

    /// Upsert a single parsed zettel into the index.
    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn index_zettel(&self, zettel: &ParsedZettel) -> Result<()> {
        let id = zettel.meta.id.as_ref().map(|z| z.0.as_str()).unwrap_or("");
        let title = zettel.meta.title.as_deref().unwrap_or("");
        let date = zettel.meta.date.as_deref().unwrap_or("");
        let ztype = zettel.meta.zettel_type.as_deref().unwrap_or("");
        let now = chrono::Utc::now().to_rfc3339();
        let tags_str = zettel.meta.tags.join(", ");

        self.with_savepoint("index_zettel", || {
            // Check if exists for FTS cleanup
            let exists: bool = self.conn.query_row(
                "SELECT COUNT(*) > 0 FROM zettels WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )?;

            if exists {
                // Delete old FTS entry
                self.conn.execute(
                    "DELETE FROM _zdb_fts WHERE rowid = (SELECT rowid FROM zettels WHERE id = ?1)",
                    params![id],
                )?;
            }

            // Upsert zettel
            self.conn.execute(
                "INSERT OR REPLACE INTO zettels (id, title, date, type, path, body, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, title, date, ztype, zettel.path, zettel.body, now],
            )?;

            // Delete and reinsert related data
            self.conn.execute("DELETE FROM _zdb_tags WHERE zettel_id = ?1", params![id])?;
            self.conn.execute("DELETE FROM _zdb_fields WHERE zettel_id = ?1", params![id])?;
            self.conn.execute("DELETE FROM _zdb_links WHERE source_id = ?1", params![id])?;
            self.conn.execute("DELETE FROM _zdb_aliases WHERE zettel_id = ?1", params![id])?;

            for tag in &zettel.meta.tags {
                self.conn.execute("INSERT INTO _zdb_tags (zettel_id, tag) VALUES (?1, ?2)", params![id, tag])?;
            }

            for field in &zettel.inline_fields {
                let zone = format!("{:?}", field.zone);
                self.conn.execute(
                    "INSERT INTO _zdb_fields (zettel_id, key, value, zone) VALUES (?1, ?2, ?3, ?4)",
                    params![id, field.key, field.value, zone],
                )?;
            }

            // Insert frontmatter extras (scalar values only)
            for (key, value) in &zettel.meta.extra {
                let str_value = match value {
                    crate::types::Value::String(s) => s.clone(),
                    crate::types::Value::Number(n) => n.to_string(),
                    crate::types::Value::Bool(b) => b.to_string(),
                    crate::types::Value::List(_) | crate::types::Value::Map(_) => continue,
                };
                self.conn.execute(
                    "INSERT INTO _zdb_fields (zettel_id, key, value, zone) VALUES (?1, ?2, ?3, ?4)",
                    params![id, key, str_value, "Frontmatter"],
                )?;
            }

            for link in &zettel.wikilinks {
                let zone = format!("{:?}", link.zone);
                self.conn.execute(
                    "INSERT INTO _zdb_links (source_id, target_path, display, zone) VALUES (?1, ?2, ?3, ?4)",
                    params![id, link.target, link.display, zone],
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
            self.conn.execute("DELETE FROM _zdb_attachments WHERE zettel_id = ?1", params![id])?;
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
        })
    }

    /// Resolve the git-relative path for a zettel ID using the index.
    pub fn resolve_path(&self, id: &str) -> Result<String> {
        self.conn
            .query_row(
                "SELECT path FROM zettels WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .map_err(|_| crate::error::ZettelError::NotFound(format!("zettel {id}")))
    }

    /// Resolve a zettel ID from an alias (case-insensitive).
    pub fn resolve_alias(&self, name: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT zettel_id FROM _zdb_aliases WHERE alias = ?1 LIMIT 1",
            params![name],
            |row| row.get(0),
        );
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Resolve a wikilink target to a zettel path.
    /// Resolution chain: path lookup → ID lookup → alias lookup.
    pub fn resolve_wikilink(&self, target: &str) -> Result<Option<String>> {
        // 1. Try as direct path (path-qualified wikilinks)
        let path_exists: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM zettels WHERE path = ?1",
                params![target],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if path_exists {
            return Ok(Some(target.to_string()));
        }
        // 2. Try as zettel ID
        if let Ok(path) = self.resolve_path(target) {
            return Ok(Some(path));
        }
        // 3. Try as alias
        if let Some(id) = self.resolve_alias(target)? {
            return Ok(Some(self.resolve_path(&id)?));
        }
        Ok(None)
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

        for (kind, path) in &changes {
            if path.contains("_typedef/") {
                typedef_changed = true;
            }
            match kind {
                DiffKind::Added | DiffKind::Modified => {
                    let content = repo.read_file(path)?;
                    let parsed = crate::parser::parse(&content, path)?;
                    self.index_zettel(&parsed)?;
                    report.indexed += 1;
                }
                DiffKind::Deleted => {
                    // Extract ID from path
                    if let Some(id) = crate::parser::extract_id_from_path(path) {
                        self.remove_zettel(&id)?;
                    }
                }
            }
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

        // Phase 1: index all zettels
        for path in &paths {
            let content = repo.read_file(path)?;
            let parsed = crate::parser::parse(&content, path)?;
            self.index_zettel(&parsed)?;
            report.indexed += 1;
        }

        // Phase 2: collect consistency warnings
        report.warnings = self.collect_consistency_warnings(repo);

        // Phase 3: materialize typed tables using merged schemas
        let mat_report = self.materialize_all_types(repo)?;
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

    /// Drop and recreate a materialized SQLite table from a schema.
    fn drop_and_create_materialized_table(&self, schema: &crate::types::TableSchema) -> Result<()> {
        self.conn.execute(
            &format!("DROP TABLE IF EXISTS \"{}\"", schema.table_name),
            [],
        )?;

        let mut col_defs = vec!["id TEXT PRIMARY KEY".to_string()];
        for col in &schema.columns {
            let sql_type = match col.data_type.to_uppercase().as_str() {
                "INTEGER" => "INTEGER",
                "REAL" => "REAL",
                "BOOLEAN" => "INTEGER",
                _ => "TEXT",
            };
            let check = if let Some(ref vals) = col.allowed_values {
                let quoted: Vec<String> = vals
                    .iter()
                    .map(|v| format!("'{}'", v.replace('\'', "''")))
                    .collect();
                format!(
                    " CHECK(\"{}\" IS NULL OR \"{}\" IN ({}))",
                    col.name,
                    col.name,
                    quoted.join(", ")
                )
            } else {
                String::new()
            };
            col_defs.push(format!("\"{}\" {}{}", col.name, sql_type, check));
        }
        self.conn.execute(
            &format!(
                "CREATE TABLE \"{}\" ({})",
                schema.table_name,
                col_defs.join(", ")
            ),
            [],
        )?;
        Ok(())
    }

    /// Populate a materialized table with data zettels of the given type.
    fn populate_materialized_table(
        &self,
        schema: &crate::types::TableSchema,
        type_name: &str,
        repo: &(impl ZettelSource + ?Sized),
    ) -> Result<()> {
        let mut data_stmt = self
            .conn
            .prepare("SELECT id, path FROM zettels WHERE type = ?1")?;
        let data_zettels: Vec<(String, String)> = data_stmt
            .query_map(params![type_name], |row| Ok((row.get(0)?, row.get(1)?)))?
            .filter_map(|r| r.ok())
            .collect();

        for (zettel_id, zettel_path) in &data_zettels {
            let zettel_content = repo.read_file(zettel_path)?;
            let zettel_parsed = crate::parser::parse(&zettel_content, zettel_path)?;
            self.materialize_row(schema, zettel_id, &zettel_parsed)?;
        }
        Ok(())
    }

    /// Rematerialize a single type's SQLite table.
    /// Loads typedef (if any), infers schema from data, merges, drops/creates table, populates rows.
    pub fn rematerialize_type(
        &self,
        type_name: &str,
        repo: &(impl ZettelSource + ?Sized),
    ) -> Result<()> {
        use crate::sql_engine::schema_from_parsed;

        // Load typedef if exists
        let typedef: Option<crate::types::TableSchema> = {
            let mut stmt = self
                .conn
                .prepare("SELECT path FROM zettels WHERE type = '_typedef' AND title = ?1")?;
            let path: Option<String> = stmt.query_row(params![type_name], |row| row.get(0)).ok();
            path.and_then(|p| {
                let content = repo.read_file(&p).ok()?;
                let parsed = crate::parser::parse(&content, &p).ok()?;
                schema_from_parsed(&parsed).ok()
            })
        };

        // Infer schema from data
        let inferred = self.infer_schema(type_name, repo)?;
        let schema = Self::merge_schemas(typedef, inferred);

        if schema.columns.is_empty() {
            return Ok(());
        }

        self.drop_and_create_materialized_table(&schema)?;
        self.populate_materialized_table(&schema, type_name, repo)?;
        Ok(())
    }

    /// Materialize SQLite tables for all typed zettels using merged schemas.
    /// Returns (tables_materialized, types_inferred).
    pub fn materialize_all_types(&self, repo: &impl ZettelSource) -> Result<(usize, Vec<String>)> {
        let mut tables_materialized = 0;
        let mut types_inferred = Vec::new();

        // Load explicit _typedef schemas
        let typedef_schemas = self.load_all_typedefs(repo);

        // Find all distinct types (excluding _typedef and empty)
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT type FROM zettels WHERE type != '_typedef' AND type != '' AND type IS NOT NULL",
        )?;
        let type_names: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        for type_name in &type_names {
            let typedef = typedef_schemas.get(type_name.as_str()).cloned();

            let inferred = self.infer_schema(type_name, repo)?;
            let schema = Self::merge_schemas(typedef.clone(), inferred);

            if schema.columns.is_empty() {
                continue;
            }

            if typedef.is_none() {
                eprintln!("info: type \"{}\" inferred from data", type_name);
                types_inferred.push(type_name.clone());
            }

            self.drop_and_create_materialized_table(&schema)?;
            self.populate_materialized_table(&schema, type_name, repo)?;
            tables_materialized += 1;
        }

        // Also materialize typedef-only types with no data zettels
        for (type_name, schema) in &typedef_schemas {
            if !type_names.contains(type_name) && !schema.columns.is_empty() {
                self.drop_and_create_materialized_table(schema)?;
                tables_materialized += 1;
            }
        }

        Ok((tables_materialized, types_inferred))
    }

    /// Infer a TableSchema for a type by scanning all data zettels of that type.
    pub fn infer_schema(
        &self,
        type_name: &str,
        repo: &(impl ZettelSource + ?Sized),
    ) -> Result<crate::types::TableSchema> {
        use crate::types::{ColumnDef, TableSchema, Zone};
        use std::collections::HashMap;

        // Query all zettels of this type
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM zettels WHERE type = ?1")?;
        let paths: Vec<String> = stmt
            .query_map(params![type_name], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        // Track columns: name -> (zone, data_types_seen)
        let mut columns: HashMap<String, (Zone, Vec<String>)> = HashMap::new();

        for path in &paths {
            let content = repo.read_file(path)?;
            let parsed = crate::parser::parse(&content, path)?;

            // Frontmatter extra keys → frontmatter columns
            // Normalize to lowercase — SQLite column names are case-insensitive
            for (key, value) in &parsed.meta.extra {
                let inferred_type = infer_yaml_type(value);
                columns
                    .entry(key.to_lowercase())
                    .or_insert_with(|| (Zone::Frontmatter, Vec::new()))
                    .1
                    .push(inferred_type);
            }

            // Body ## headings → body TEXT columns
            for heading in extract_body_headings(&parsed.body) {
                columns
                    .entry(heading.to_lowercase())
                    .or_insert_with(|| (Zone::Body, vec!["TEXT".to_string()]));
            }

            // Reference fields → reference columns
            for field in &parsed.inline_fields {
                if field.zone == Zone::Reference {
                    let entry = columns
                        .entry(field.key.to_lowercase())
                        .or_insert_with(|| (Zone::Reference, Vec::new()));
                    entry.1.push("TEXT".to_string());
                }
            }
        }

        // Build final columns with type widening
        let mut cols: Vec<ColumnDef> = columns
            .into_iter()
            .map(|(name, (zone, types))| {
                let data_type = widen_types(&types);
                ColumnDef {
                    name,
                    data_type,
                    references: None,
                    zone: Some(zone),
                    required: false,
                    search_boost: None,
                    allowed_values: None,
                    default_value: None,
                }
            })
            .collect();

        // Sort columns for deterministic output
        cols.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(TableSchema {
            table_name: type_name.to_string(),
            columns: cols,
            crdt_strategy: None,
            template_sections: vec![],
        })
    }

    /// Merge an explicit typedef schema with an inferred schema.
    /// Typedef columns take precedence; inferred columns fill gaps.
    pub fn merge_schemas(
        typedef: Option<crate::types::TableSchema>,
        inferred: crate::types::TableSchema,
    ) -> crate::types::TableSchema {
        match typedef {
            None => inferred,
            Some(mut td) => {
                let existing_names: std::collections::HashSet<String> =
                    td.columns.iter().map(|c| c.name.clone()).collect();
                for col in inferred.columns {
                    if !existing_names.contains(&col.name) {
                        td.columns.push(col);
                    }
                }
                td
            }
        }
    }

    /// Collect structural consistency warnings during rebuild.
    /// Warnings don't prevent indexing — they're advisory only.
    pub fn collect_consistency_warnings(
        &self,
        repo: &impl ZettelSource,
    ) -> Vec<crate::types::ConsistencyWarning> {
        use crate::types::ConsistencyWarning;

        let mut warnings = Vec::new();

        let paths = match repo.list_zettels() {
            Ok(p) => p,
            Err(_) => return warnings,
        };

        let typedef_schemas = self.load_all_typedefs(repo);

        for path in &paths {
            let content = match repo.read_file(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            match crate::parser::parse(&content, path) {
                Ok(parsed) => {
                    // Check for cross-zone duplicate keys
                    let mut seen_keys: std::collections::HashMap<String, &str> =
                        std::collections::HashMap::new();

                    for key in parsed.meta.extra.keys() {
                        seen_keys.insert(key.clone(), "frontmatter");
                    }

                    for field in &parsed.inline_fields {
                        if field.zone == crate::types::Zone::Reference {
                            if let Some(&other_zone) = seen_keys.get(&field.key) {
                                if other_zone != "reference" {
                                    warnings.push(ConsistencyWarning::CrossZoneDuplicate {
                                        path: path.clone(),
                                        key: field.key.clone(),
                                    });
                                }
                            }
                        }
                    }

                    // Check missing required fields
                    if let Some(type_name) = &parsed.meta.zettel_type {
                        if let Some(schema) = typedef_schemas.get(type_name.as_str()) {
                            for col in &schema.columns {
                                if col.required {
                                    let has_value = match col
                                        .zone
                                        .as_ref()
                                        .unwrap_or(&crate::types::Zone::Frontmatter)
                                    {
                                        crate::types::Zone::Frontmatter => {
                                            parsed.meta.extra.contains_key(&col.name)
                                        }
                                        crate::types::Zone::Reference => {
                                            parsed.inline_fields.iter().any(|f| f.key == col.name)
                                        }
                                        crate::types::Zone::Body => {
                                            parsed.body.contains(&format!("## {}", col.name))
                                        }
                                    };
                                    if !has_value {
                                        warnings.push(ConsistencyWarning::MissingRequired {
                                            path: path.clone(),
                                            type_name: type_name.clone(),
                                            field: col.name.clone(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warnings.push(ConsistencyWarning::MalformedYaml {
                        path: path.clone(),
                        error: e.to_string(),
                    });
                }
            }
        }

        warnings
    }

    /// Load all _typedef schemas from the index.
    fn load_all_typedefs(
        &self,
        repo: &impl ZettelSource,
    ) -> std::collections::HashMap<String, crate::types::TableSchema> {
        use crate::sql_engine::schema_from_parsed;

        let mut schemas = std::collections::HashMap::new();

        let mut stmt = match self
            .conn
            .prepare("SELECT path FROM zettels WHERE type = '_typedef'")
        {
            Ok(s) => s,
            Err(_) => return schemas,
        };

        let paths: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();

        for path in &paths {
            if let Ok(content) = repo.read_file(path) {
                if let Ok(parsed) = crate::parser::parse(&content, path) {
                    if let Ok(schema) = schema_from_parsed(&parsed) {
                        schemas.insert(schema.table_name.clone(), schema);
                    }
                }
            }
        }

        schemas
    }

    /// Insert a single data zettel's values into a materialized table.
    fn materialize_row(
        &self,
        schema: &crate::types::TableSchema,
        id: &str,
        zettel: &crate::types::ParsedZettel,
    ) -> Result<()> {
        let mut col_names = vec!["id".to_string()];
        let mut placeholders = vec!["?1".to_string()];
        let mut vals: Vec<Option<String>> = vec![Some(id.to_string())];

        for (i, col) in schema.columns.iter().enumerate() {
            col_names.push(format!("\"{}\"", col.name));
            placeholders.push(format!("?{}", i + 2));
            let val = extract_column_value(zettel, col);
            vals.push(if val.is_empty() { None } else { Some(val) });
        }

        let sql = format!(
            "INSERT OR REPLACE INTO \"{}\" ({}) VALUES ({})",
            schema.table_name,
            col_names.join(", "),
            placeholders.join(", ")
        );

        let params: Vec<&dyn rusqlite::types::ToSql> = vals
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        self.conn.execute(&sql, params.as_slice())?;
        Ok(())
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

    /// Find all zettels linking to a given target.
    pub fn backlinks(&self, target_path: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT source_id FROM _zdb_links WHERE target_path = ?1")?;
        let ids = stmt.query_map(params![target_path], |row| row.get(0))?;
        let mut out = Vec::new();
        for id in ids {
            out.push(id?);
        }
        Ok(out)
    }

    /// Find all zettels linking to a target, returning (source_id, source_path).
    pub fn backlinking_zettel_paths(&self, target: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT l.source_id, z.path \
             FROM _zdb_links l JOIN zettels z ON l.source_id = z.id \
             WHERE l.target_path = ?1",
        )?;
        let rows = stmt.query_map(params![target], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Return (id, title) pairs of zettels with `resurrected: true` frontmatter.
    pub fn resurrected_zettels(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT z.id, z.title FROM zettels z \
             JOIN _zdb_fields f ON f.zettel_id = z.id \
             WHERE f.key = 'resurrected' AND f.value = 'true' \
             AND f.zone = 'Frontmatter'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Return (source_id, target_path) pairs where a link target has no matching zettel.
    pub fn broken_backlinks(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT l.source_id, l.target_path \
             FROM _zdb_links l \
             LEFT JOIN zettels z ON l.target_path = z.id \
             WHERE z.id IS NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
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

/// Extract a column value from a parsed zettel according to zone mapping.
fn extract_column_value(
    zettel: &crate::types::ParsedZettel,
    col: &crate::types::ColumnDef,
) -> String {
    use crate::types::Zone;

    let zone = col.zone.clone().unwrap_or_else(|| {
        if col.references.is_some() {
            Zone::Reference
        } else if matches!(
            col.data_type.to_uppercase().as_str(),
            "INTEGER" | "REAL" | "BOOLEAN"
        ) {
            Zone::Frontmatter
        } else {
            Zone::Body
        }
    });

    match zone {
        Zone::Reference => {
            for field in &zettel.inline_fields {
                if field.key == col.name {
                    let val = field.value.trim();
                    let val = val.strip_prefix("[[").unwrap_or(val);
                    let val = val.strip_suffix("]]").unwrap_or(val);
                    let val = val.split('|').next().unwrap_or(val);
                    return val.to_string();
                }
            }
            String::new()
        }
        Zone::Frontmatter => zettel
            .meta
            .extra
            .get(&col.name)
            .map(|v| match v {
                crate::types::Value::Number(n) => n.to_string(),
                crate::types::Value::Bool(b) => b.to_string(),
                crate::types::Value::String(s) => s.clone(),
                _ => format!("{v:?}"),
            })
            .unwrap_or_default(),
        Zone::Body => extract_body_section(&zettel.body, &col.name),
    }
}

/// Extract text content under a `## heading` in the body.
fn extract_body_section(body: &str, section_name: &str) -> String {
    let heading = format!("## {section_name}");
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == heading {
            i += 1;
            // Skip blank line after heading
            if i < lines.len() && lines[i].trim().is_empty() {
                i += 1;
            }
            let mut content_lines = Vec::new();
            while i < lines.len() && !lines[i].starts_with("## ") {
                content_lines.push(lines[i]);
                i += 1;
            }
            // Trim trailing blank lines
            while content_lines.last().is_some_and(|l| l.trim().is_empty()) {
                content_lines.pop();
            }
            return content_lines.join("\n");
        }
        i += 1;
    }
    String::new()
}

/// Extract all ## heading names from body text.
fn extract_body_headings(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("## ")
                .map(|h| h.trim().to_string())
        })
        .collect()
}

/// Infer a SQL data type from a domain Value.
fn infer_yaml_type(value: &crate::types::Value) -> String {
    match value {
        crate::types::Value::Bool(_) => "BOOLEAN".to_string(),
        crate::types::Value::Number(n) => {
            if n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 {
                "INTEGER".to_string()
            } else {
                "REAL".to_string()
            }
        }
        crate::types::Value::String(s) => {
            if s.parse::<i64>().is_ok() {
                "INTEGER".to_string()
            } else if s.parse::<f64>().is_ok() {
                "REAL".to_string()
            } else if s == "true" || s == "false" {
                "BOOLEAN".to_string()
            } else {
                "TEXT".to_string()
            }
        }
        _ => "TEXT".to_string(),
    }
}

/// Widen types: if all values agree, use that type; otherwise widen to TEXT.
fn widen_types(types: &[String]) -> String {
    if types.is_empty() {
        return "TEXT".to_string();
    }
    let first = &types[0];
    if types.iter().all(|t| t == first) {
        return first.clone();
    }
    // INTEGER + REAL → REAL
    if types.iter().all(|t| t == "INTEGER" || t == "REAL") {
        return "REAL".to_string();
    }
    "TEXT".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_ops::GitRepo;
    use crate::types::{InlineField, Value, WikiLink, ZettelId, ZettelMeta, Zone};

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
            reference_section: "- source:: Wikipedia".into(),
            inline_fields: vec![InlineField {
                key: "source".into(),
                value: "Wikipedia".into(),
                zone: Zone::Reference,
            }],
            wikilinks: vec![WikiLink {
                target: "20260101000000".into(),
                display: Some("Link".into()),
                zone: Zone::Body,
            }],
            body_tags: vec![],
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
        };
        let inferred = TableSchema {
            table_name: "foo".to_string(),
            columns: vec![],
            crdt_strategy: None,
            template_sections: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![],
            body_tags: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![],
            body_tags: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![],
            body_tags: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![],
            body_tags: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![],
            body_tags: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![],
            body_tags: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![],
            body_tags: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![],
            body_tags: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![crate::types::WikiLink {
                target: "20260301120000".to_string(),
                display: None,
                zone: crate::types::Zone::Body,
            }],
            body_tags: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![],
            body_tags: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![],
            body_tags: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![WikiLink {
                target: "20260301100000".into(),
                display: None,
                zone: Zone::Body,
            }],
            body_tags: vec![],
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
            reference_section: String::new(),
            inline_fields: vec![],
            wikilinks: vec![],
            body_tags: vec![],
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
}
