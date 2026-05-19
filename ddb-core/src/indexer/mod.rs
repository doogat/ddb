mod filter;
mod graph;
pub(crate) mod materialize;
mod rebuild;
mod resolve;
mod search;

use std::path::Path;

use rusqlite::{params, Connection};

use crate::error::{DoogatError, Result};
use crate::traits::DoogatSource;
use crate::types::ParsedDoogat;

impl From<rusqlite::Error> for DoogatError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e.to_string())
    }
}

pub use crate::types::{PaginatedSearchResult, SearchResult};
pub use materialize::is_core_column;

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
            title, body, tags, fields,
            tokenize = 'porter unicode61'
        );

        CREATE TABLE IF NOT EXISTS _ddb_boost (
            type_name TEXT PRIMARY KEY,
            max_boost REAL NOT NULL DEFAULT 1.0
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

        // Detect old 3-column FTS5 schema and upgrade if needed.
        // FTS5 virtual tables cannot be ALTERed, so we drop all tables
        // and recreate from the current SCHEMA_DDL.
        if Self::needs_schema_upgrade(&conn) {
            tracing::info!("index schema outdated, dropping tables for upgrade");
            conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_master \
                 WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'",
            )?;
            let tables: Vec<String> = stmt
                .query_map([], |row| row.get(0))?
                .filter_map(|r| r.ok())
                .collect();
            drop(stmt);
            for table in &tables {
                conn.execute_batch(&format!("DROP TABLE IF EXISTS \"{table}\""))?;
            }
            conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        }

        conn.execute_batch(Self::SCHEMA_DDL)?;
        Ok(Self { conn })
    }

    /// Check whether the existing schema needs upgrading (e.g. old 3-column
    /// FTS5 table missing `fields`, or missing `_ddb_boost` table).
    fn needs_schema_upgrade(conn: &Connection) -> bool {
        let fts_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master \
                 WHERE type='table' AND name='_ddb_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if !fts_exists {
            return false; // fresh DB, SCHEMA_DDL will create everything
        }
        // Check FTS5 column list via sqlite_master DDL for the `fields` column
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='_ddb_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap_or_default();
        !sql.contains("fields")
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

        self.clear_doogat_relations(id)?;

        self.conn.execute(
            "INSERT OR REPLACE INTO doogats (id, title, date, type, path, body, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, title, date, ztype, doogat.path, doogat.body, now],
        )?;

        self.insert_tags(id, doogat)?;
        self.insert_checkboxes(id, doogat)?;
        self.insert_inline_fields(id, doogat)?;
        self.insert_links(id, doogat)?;
        self.insert_aliases(id, doogat)?;
        self.insert_attachments(id, doogat)?;
        self.insert_fts_entry(id, title, doogat)?;

        Ok(())
    }

    /// Delete all related data for a doogat before reinserting.
    fn clear_doogat_relations(&self, id: &str) -> Result<()> {
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
        Ok(())
    }

    fn insert_fts_entry(&self, id: &str, title: &str, doogat: &ParsedDoogat) -> Result<()> {
        let tags_str = doogat.meta.tags.join(", ");
        let fields_str = collect_fts_fields(&doogat.meta.extra);
        self.conn.execute(
            "INSERT INTO _ddb_fts (rowid, title, body, tags, fields) VALUES (
                (SELECT rowid FROM doogats WHERE id = ?1), ?2, ?3, ?4, ?5
            )",
            params![id, title, doogat.body, tags_str, fields_str],
        )?;
        Ok(())
    }

    fn insert_tags(&self, id: &str, doogat: &ParsedDoogat) -> Result<()> {
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
        Ok(())
    }

    fn insert_checkboxes(&self, id: &str, doogat: &ParsedDoogat) -> Result<()> {
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
        Ok(())
    }

    fn insert_inline_fields(&self, id: &str, doogat: &ParsedDoogat) -> Result<()> {
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
        Ok(())
    }

    fn insert_links(&self, id: &str, doogat: &ParsedDoogat) -> Result<()> {
        for link in &doogat.links {
            let zone = format!("{:?}", link.zone);
            let kind = link.kind.as_str();
            self.conn.execute(
                "INSERT INTO _ddb_links (source_id, target_path, display, zone, kind) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, link.target, link.display, zone, kind],
            )?;
        }
        Ok(())
    }

    fn insert_aliases(&self, id: &str, doogat: &ParsedDoogat) -> Result<()> {
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
        Ok(())
    }

    fn insert_attachments(&self, id: &str, doogat: &ParsedDoogat) -> Result<()> {
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

/// Run `f` inside a `BEGIN IMMEDIATE` transaction, committing on `Ok` and
/// rolling back on `Err`. The closure's error propagates unchanged; a failed
/// `ROLLBACK` is non-fatal — it is logged at `warn` level and the original
/// closure error is still returned.
///
/// `#[cfg(test)]` is temporary: the helper has no production caller yet, so
/// without the gate it would warn as dead code (the project forbids
/// dead-code lint suppression). The task that wires it into the SINGLETON
/// write paths is its first production caller and must remove this gate.
#[cfg(test)]
pub(crate) fn with_immediate_transaction<T>(
    conn: &rusqlite::Connection,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match f() {
        Ok(value) => {
            conn.execute_batch("COMMIT")?;
            Ok(value)
        }
        Err(e) => {
            if let Err(rb_err) = conn.execute_batch("ROLLBACK") {
                tracing::warn!(error = %rb_err, "transaction rollback failed");
            }
            Err(e)
        }
    }
}

/// Pass-through trait impl for `SqlBackend`. Each method body delegates to
/// the inherent method of the same name on `Index`. Rust's method-resolution
/// rules pick the inherent method over the trait method when called via
/// `self.<method>(...)`, so these bodies are not self-recursive. The compiler
/// also catches accidental recursion via the on-by-default
/// `unconditional_recursion` lint if an inherent method is ever removed
/// without updating the trait body. PRD 00134 cycle-1 review C1 task #8.
impl crate::traits::SqlBackend for Index {
    fn sql_conn(&self) -> &rusqlite::Connection {
        &self.conn
    }

    fn query_raw_with_columns(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        self.query_raw_with_columns(sql)
    }

    fn rematerialize_type(
        &self,
        type_name: &str,
        source: &dyn crate::traits::DoogatSource,
    ) -> Result<()> {
        self.rematerialize_type(type_name, source)
    }

    fn materialize_single(
        &self,
        schema: &crate::types::TableSchema,
        id: &str,
        parsed: &crate::types::ParsedDoogat,
    ) -> Result<()> {
        self.materialize_single(schema, id, parsed)
    }

    fn populate_junction_tables(
        &self,
        schema: &crate::types::TableSchema,
        id: &str,
        parsed: &crate::types::ParsedDoogat,
    ) -> Result<()> {
        self.populate_junction_tables(schema, id, parsed)
    }

    fn sync_junction_tables_for_columns(
        &self,
        schema: &crate::types::TableSchema,
        id: &str,
        parsed: &crate::types::ParsedDoogat,
        changed_cols: &[&str],
    ) -> Result<()> {
        self.sync_junction_tables_for_columns(schema, id, parsed, changed_cols)
    }

    fn type_uses_folder(&self, type_name: &str, source: &dyn crate::traits::DoogatSource) -> bool {
        self.type_uses_folder(type_name, source)
    }

    fn backlinks_by_target(
        &self,
        target_id: &str,
        target_path: &str,
    ) -> Result<Vec<(String, String)>> {
        self.backlinks_by_target(target_id, target_path)
    }

    fn check_restrict_blocks_delete(
        &self,
        source: &dyn crate::traits::DoogatSource,
        deleted_id: &str,
    ) -> Result<()> {
        self.check_restrict_blocks_delete(source, deleted_id)
    }
}

/// Pass-through trait impl for `DoogatIndex`. Same dispatch contract as the
/// `SqlBackend` impl above: bodies delegate to inherent methods on `Index`,
/// recursion would be caught by `unconditional_recursion`. PRD 00134
/// cycle-1 review C1 task #8.
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

/// Collect scalar frontmatter extra values into a space-separated string
/// for the FTS5 `fields` column. Skips internal keys that have dedicated tables.
fn collect_fts_fields(extras: &std::collections::BTreeMap<String, crate::types::Value>) -> String {
    const SKIP_KEYS: &[&str] = &["aliases", "attachments"];
    let mut parts = Vec::new();
    for (key, value) in extras {
        if SKIP_KEYS.contains(&key.as_str()) {
            continue;
        }
        collect_value_strings(value, &mut parts);
    }
    parts.join(" ")
}

/// Recursively extract string representations from a Value tree.
fn collect_value_strings(value: &crate::types::Value, out: &mut Vec<String>) {
    match value {
        crate::types::Value::String(s) => out.push(s.clone()),
        crate::types::Value::Number(n) => out.push(n.to_string()),
        crate::types::Value::Bool(b) => out.push(b.to_string()),
        crate::types::Value::Map(map) => {
            for v in map.values() {
                collect_value_strings(v, out);
            }
        }
        crate::types::Value::List(list) => {
            for v in list {
                collect_value_strings(v, out);
            }
        }
    }
}

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
