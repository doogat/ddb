use regex::Regex;
use rusqlite::params;
use sqlparser::ast::{
    AlterTableOperation, AssignmentTarget, CharacterLength, ColumnOption, DataType, Expr,
    FromTable, ObjectType, SetExpr, Statement, Value as SqlValue,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use std::collections::BTreeMap;
use std::sync::OnceLock;

fn re_set_zone() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+SET\s+ZONE\s+(frontmatter|body|reference)\s+FOR\s+(?:"([^"]+)"|(\w[\w-]*))\s*;?\s*$"#).unwrap()
    })
}

fn re_set_title_template() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+SET\s+TITLE\s+TEMPLATE\s+'([^']+)'\s*;?\s*$"#).unwrap()
    })
}

fn re_drop_title_template() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)^\s*ALTER\s+TABLE\s+(?:"([^"]+)"|(\w[\w-]*))\s+DROP\s+TITLE\s+TEMPLATE\s*;?\s*$"#).unwrap()
    })
}

fn re_unfilled_placeholder() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{[^}]+\}").unwrap())
}

use crate::error::{Result, ZettelError};
use crate::indexer::Index;
use crate::parser;
use crate::traits::ZettelStore;
use crate::types::{
    ColumnDef, InlineField, Link, ParsedZettel, TableSchema, Value, ZettelId, ZettelMeta, Zone,
};

/// Strip surrounding double-quotes from a SQL identifier.
/// sqlparser preserves quotes in `to_string()` for identifiers like `"meeting-minutes"`.
fn unquote_identifier(s: &str) -> String {
    s.trim_matches('"').to_lowercase()
}

#[derive(Debug)]
pub enum SqlResult {
    Rows {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Affected(usize),
    Ok(String),
}

pub struct PendingWrite {
    pub path: String,
    pub content: String,
}

pub struct PendingDelete {
    pub path: String,
    pub zettel_id: String,
}

#[derive(Default)]
pub struct TransactionBuffer {
    pub writes: Vec<PendingWrite>,
    pub deletes: Vec<PendingDelete>,
}

pub struct SqlEngine<'a> {
    index: &'a Index,
    repo: &'a dyn ZettelStore,
    txn: Option<TransactionBuffer>,
}

/// Reserved table names that cannot be used for CREATE TABLE.
fn is_reserved_table(name: &str) -> bool {
    name == "zettels" || name.starts_with("_zdb_") || name.starts_with("sqlite_")
}

impl Drop for SqlEngine<'_> {
    fn drop(&mut self) {
        if self.txn.take().is_some() {
            if let Err(e) = self.index.conn.execute("ROLLBACK TO zdb_txn", []) {
                tracing::warn!(error = %e, "sql_engine drop: rollback failed");
            }
            if let Err(e) = self.index.conn.execute("RELEASE zdb_txn", []) {
                tracing::warn!(error = %e, "sql_engine drop: release failed");
            }
        }
    }
}

impl<'a> SqlEngine<'a> {
    pub fn new(index: &'a Index, repo: &'a dyn ZettelStore) -> Self {
        Self {
            index,
            repo,
            txn: None,
        }
    }

    /// Restore a previously extracted transaction buffer.
    /// The caller is responsible for ensuring the SAVEPOINT is still active
    /// on `index.conn` (i.e. the same connection that created it).
    pub fn resume_transaction(&mut self, buf: TransactionBuffer) {
        self.txn = Some(buf);
    }

    /// Extract the transaction buffer without triggering Drop's rollback.
    /// Returns `None` if no transaction is active.
    pub fn suspend_transaction(&mut self) -> Option<TransactionBuffer> {
        self.txn.take()
    }

    /// Generate a unique ZettelId, waiting if same-second collision detected.
    fn unique_id(&mut self) -> Result<ZettelId> {
        self.unique_ids(1).map(|mut v| v.remove(0))
    }

    /// Generate `count` unique ZettelIds without sleeping between them.
    ///
    /// Gets a base timestamp via `generate_unique_id`, then increments by 1
    /// second for each subsequent ID, skipping any that already exist in the
    /// index.
    fn unique_ids(&mut self, count: usize) -> Result<Vec<ZettelId>> {
        use chrono::NaiveDateTime;

        let mut ids = Vec::with_capacity(count);
        let first = parser::generate_unique_id(|candidate| {
            self.index
                .conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM zettels WHERE id = ?1",
                    params![candidate],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false)
        });

        let mut ts = NaiveDateTime::parse_from_str(&first.0, "%Y%m%d%H%M%S").map_err(|e| {
            ZettelError::SqlEngine(format!("failed to parse generated id timestamp: {e}"))
        })?;
        ids.push(first);

        for _ in 1..count {
            loop {
                ts += chrono::Duration::seconds(1);
                let candidate = ts.format("%Y%m%d%H%M%S").to_string();
                let exists: bool = self
                    .index
                    .conn
                    .query_row(
                        "SELECT COUNT(*) > 0 FROM zettels WHERE id = ?1",
                        params![&candidate],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);
                if !exists {
                    ids.push(ZettelId(candidate));
                    break;
                }
            }
        }

        Ok(ids)
    }

    #[cfg_attr(feature = "profiling", tracing::instrument(skip_all))]
    pub fn execute(&mut self, sql: &str) -> Result<SqlResult> {
        // Pre-parse interception for custom DDL that sqlparser can't handle
        if let Some(result) = self.try_custom_ddl(sql)? {
            return Ok(result);
        }

        let mut results = self.execute_batch(sql)?;
        if results.len() != 1 {
            return Err(ZettelError::SqlEngine(
                "expected exactly one SQL statement".into(),
            ));
        }
        Ok(results.remove(0))
    }

    fn try_custom_ddl(&mut self, sql: &str) -> Result<Option<SqlResult>> {
        if let Some(caps) = re_set_zone().captures(sql) {
            let table = caps.get(1).or(caps.get(2)).unwrap().as_str();
            let zone = caps.get(3).unwrap().as_str();
            let column = caps.get(4).or(caps.get(5)).unwrap().as_str();
            return Ok(Some(self.handle_set_zone(table, zone, column)?));
        }
        if let Some(caps) = re_set_title_template().captures(sql) {
            let table = caps.get(1).or(caps.get(2)).unwrap().as_str();
            let template = caps.get(3).unwrap().as_str();
            return Ok(Some(self.handle_title_template(table, Some(template))?));
        }
        if let Some(caps) = re_drop_title_template().captures(sql) {
            let table = caps.get(1).or(caps.get(2)).unwrap().as_str();
            return Ok(Some(self.handle_title_template(table, None)?));
        }
        Ok(None)
    }

    pub fn execute_batch(&mut self, sql: &str) -> Result<Vec<SqlResult>> {
        // Pre-parse interception for custom DDL that sqlparser can't handle
        if let Some(result) = self.try_custom_ddl(sql)? {
            return Ok(vec![result]);
        }

        let dialect = GenericDialect {};
        let statements = Parser::parse_sql(&dialect, sql)
            .map_err(|e| ZettelError::SqlEngine(format!("parse: {e}")))?;

        if statements.is_empty() {
            return Err(ZettelError::SqlEngine("no SQL statements".into()));
        }

        let mut results = Vec::with_capacity(statements.len());
        for stmt in &statements {
            results.push(self.execute_statement(stmt)?);
        }
        Ok(results)
    }

    fn execute_statement(&mut self, stmt: &Statement) -> Result<SqlResult> {
        match stmt {
            Statement::CreateTable(ct) => self.handle_create_table(ct),
            Statement::Insert(ins) => self.handle_insert(ins),
            Statement::Update { table, assignments, from, selection, .. } => {
                if from.is_some() {
                    return Err(ZettelError::SqlEngine(
                        "UPDATE...FROM not supported: ambiguous join-to-document mapping; decompose into SELECT + individual UPDATEs".into(),
                    ));
                }
                self.handle_update(table, assignments, selection)
            }
            Statement::Delete(del) => self.handle_delete(del),
            Statement::AlterTable { name, operations, .. } => {
                self.handle_alter_table(name, operations)
            }
            Statement::Drop { object_type: ObjectType::Index, .. } => {
                Err(ZettelError::SqlEngine(
                    "DROP INDEX not supported: indexes are managed automatically and rebuilt on reindex".into(),
                ))
            }
            Statement::Drop { object_type: ObjectType::View, .. } => {
                Err(ZettelError::SqlEngine(
                    "DROP VIEW not supported: views cannot be created".into(),
                ))
            }
            Statement::Drop { object_type, if_exists, names, cascade, .. } => {
                self.handle_drop(object_type, *if_exists, names, *cascade)
            }
            Statement::CreateIndex(_) => {
                Err(ZettelError::SqlEngine(
                    "CREATE INDEX not supported: indexes on the materialized cache are rebuilt from zettel data on reindex".into(),
                ))
            }
            Statement::CreateView { .. } => {
                Err(ZettelError::SqlEngine(
                    "CREATE VIEW not supported: views are not stored as zettels and are lost on reindex".into(),
                ))
            }
            Statement::CreateVirtualTable { .. } => {
                Err(ZettelError::SqlEngine(
                    "CREATE VIRTUAL TABLE not supported: virtual tables have no zettel representation".into(),
                ))
            }
            Statement::CreateTrigger { .. } => {
                Err(ZettelError::SqlEngine(
                    "CREATE TRIGGER not supported: triggers fire on cache mutations, not git commits".into(),
                ))
            }
            Statement::AlterIndex { .. } => {
                Err(ZettelError::SqlEngine(
                    "ALTER INDEX not supported: indexes are managed automatically and rebuilt on reindex".into(),
                ))
            }
            Statement::StartTransaction { .. } => self.handle_begin(),
            Statement::Commit { .. } => self.handle_commit(),
            Statement::Rollback { .. } => self.handle_rollback(),
            _ => {
                // Pass through (SELECT and anything else) to raw query
                let sql_str = stmt.to_string();
                let (columns, rows) = self.index.query_raw_with_columns(&sql_str)?;
                Ok(SqlResult::Rows { columns, rows })
            }
        }
    }

    /// Read file content, checking transaction buffer first (latest write wins).
    fn read_content(&self, path: &str) -> Result<String> {
        if let Some(ref buf) = self.txn {
            // Search in reverse for latest buffered write
            for w in buf.writes.iter().rev() {
                if w.path == path {
                    return Ok(w.content.clone());
                }
            }
            // Check if it was deleted in the buffer
            for d in buf.deletes.iter().rev() {
                if d.path == path {
                    return Err(ZettelError::NotFound(format!(
                        "deleted in transaction: {path}"
                    )));
                }
            }
        }
        self.repo.read_file(path)
    }

    fn handle_begin(&mut self) -> Result<SqlResult> {
        if self.txn.is_some() {
            return Err(ZettelError::SqlEngine("transaction already active".into()));
        }
        self.index
            .conn
            .execute("SAVEPOINT zdb_txn", [])
            .map_err(|e| ZettelError::SqlEngine(format!("savepoint: {e}")))?;
        self.txn = Some(TransactionBuffer::default());
        Ok(SqlResult::Ok("BEGIN".into()))
    }

    fn handle_commit(&mut self) -> Result<SqlResult> {
        let buf = self
            .txn
            .as_ref()
            .ok_or_else(|| ZettelError::SqlEngine("no active transaction".into()))?;

        // Flush buffered writes/deletes to git in a single commit.
        // Cancelled operations: if a path was written then deleted, skip both
        // (the file may not exist in git if it was created within the txn).
        let delete_paths: std::collections::HashSet<&str> =
            buf.deletes.iter().map(|d| d.path.as_str()).collect();

        let writes: Vec<(&str, &str)> = buf
            .writes
            .iter()
            .filter(|w| !delete_paths.contains(w.path.as_str()))
            .map(|w| (w.path.as_str(), w.content.as_str()))
            .collect();
        // Only delete files that exist in git (not buffer-only creations)
        let deletes: Vec<&str> = buf
            .deletes
            .iter()
            .filter(|d| self.repo.read_file(&d.path).is_ok())
            .map(|d| d.path.as_str())
            .collect();

        if !writes.is_empty() || !deletes.is_empty() {
            self.repo.commit_batch(&writes, &deletes, "transaction")?;
        }

        self.index
            .conn
            .execute("RELEASE zdb_txn", [])
            .map_err(|e| ZettelError::SqlEngine(format!("release: {e}")))?;
        // Clear txn only after both git commit and RELEASE succeed
        self.txn.take();
        Ok(SqlResult::Ok("COMMIT".into()))
    }

    fn handle_rollback(&mut self) -> Result<SqlResult> {
        if self.txn.is_none() {
            return Err(ZettelError::SqlEngine("no active transaction".into()));
        }
        self.index
            .conn
            .execute("ROLLBACK TO zdb_txn", [])
            .map_err(|e| ZettelError::SqlEngine(format!("rollback: {e}")))?;
        self.index
            .conn
            .execute("RELEASE zdb_txn", [])
            .map_err(|e| ZettelError::SqlEngine(format!("release: {e}")))?;
        // Only clear txn after SQLite ops succeed — Drop still cleans up on failure
        self.txn.take();
        Ok(SqlResult::Ok("ROLLBACK".into()))
    }

    fn handle_create_table(&mut self, ct: &sqlparser::ast::CreateTable) -> Result<SqlResult> {
        let table_name = unquote_identifier(&ct.name.to_string());

        if is_reserved_table(&table_name) {
            return Err(ZettelError::SqlEngine(format!(
                "reserved table name: {table_name}"
            )));
        }

        // Check if typedef already exists
        let existing: Option<String> = self
            .index
            .conn
            .query_row(
                "SELECT id FROM zettels WHERE type = '_typedef' AND title = ?1",
                params![table_name],
                |row| row.get(0),
            )
            .ok();
        if existing.is_some() {
            if ct.if_not_exists {
                return Ok(SqlResult::Ok(format!(
                    "table already exists, skipped: {table_name}"
                )));
            }
            return Err(ZettelError::SqlEngine(format!(
                "table already exists: {table_name}"
            )));
        }

        // Extract columns
        let columns = self.extract_columns(&ct.columns)?;
        let schema = TableSchema {
            table_name: table_name.clone(),
            columns,
            crdt_strategy: None,
            template_sections: vec![],
            folder: false,
            stale_after_days: None,
            title_template: None,
            origin: Some("ddl".into()),
        };

        // Build and commit typedef zettel
        let id = self.unique_id()?;
        let schema_zettel = build_typedef_zettel(&id, &schema);
        let content = parser::serialize(&schema_zettel);
        let path = format!("zettelkasten/_typedef/{}.md", id.0);
        self.repo
            .commit_file(&path, &content, &format!("create table {table_name}"))?;

        // Index the typedef zettel
        let parsed = parser::parse(&content, &path)?;
        self.index.index_zettel(&parsed)?;

        // Create materialized SQLite table
        self.create_materialized_table(&schema)?;

        Ok(SqlResult::Ok(format!("table {table_name} created")))
    }

    fn extract_columns(&mut self, cols: &[sqlparser::ast::ColumnDef]) -> Result<Vec<ColumnDef>> {
        let mut out = Vec::new();
        for col in cols {
            let name = col.name.value.to_lowercase();
            if name == "id" || name == "type" || name == "title" {
                continue; // implicit columns, skip
            }
            let data_type = data_type_to_string(&col.data_type);
            let references = extract_references(&col.options);
            let zone = if references.is_some() {
                Some(Zone::Reference)
            } else if is_numeric_type(&data_type) || is_short_string_type(&col.data_type) {
                Some(Zone::Frontmatter)
            } else {
                Some(Zone::Body)
            };
            let allowed_values = extract_allowed_values(&col.data_type);
            let default_value = extract_default(&col.options);
            out.push(ColumnDef {
                name,
                data_type,
                references,
                zone,
                required: false,
                search_boost: None,
                allowed_values,
                default_value,
            });
        }
        Ok(out)
    }

    fn create_materialized_table(&mut self, schema: &TableSchema) -> Result<()> {
        let mut col_defs = vec!["id TEXT PRIMARY KEY".to_string()];
        for col in &schema.columns {
            let sql_type = match col.data_type.to_uppercase().as_str() {
                "INTEGER" => "INTEGER",
                "REAL" => "REAL",
                "BOOLEAN" => "INTEGER", // SQLite stores booleans as integers
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
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS \"{}\" ({})",
            schema.table_name,
            col_defs.join(", ")
        );
        self.index.conn.execute(&sql, [])?;

        // Create junction tables for REFERENCES columns
        for col in &schema.columns {
            if col.references.is_some() {
                self.index.conn.execute(
                    &format!(
                        "CREATE TABLE IF NOT EXISTS \"{t}_{c}\" (\"{t}_id\" TEXT NOT NULL, \"{c}_id\" TEXT NOT NULL, PRIMARY KEY (\"{t}_id\", \"{c}_id\"))",
                        t = schema.table_name,
                        c = col.name
                    ),
                    [],
                )?;
            }
        }

        Ok(())
    }

    fn handle_insert(&mut self, ins: &sqlparser::ast::Insert) -> Result<SqlResult> {
        // Reject REPLACE/UPSERT variants that bypass git
        if ins.replace_into {
            return Err(ZettelError::SqlEngine(
                "REPLACE INTO not supported: bypasses git storage; use explicit DELETE + INSERT instead".into(),
            ));
        }
        if ins.or.is_some() {
            return Err(ZettelError::SqlEngine(
                "INSERT OR REPLACE/UPSERT not supported: bypasses git storage; use explicit INSERT + UPDATE instead".into(),
            ));
        }
        if ins.on.is_some() {
            return Err(ZettelError::SqlEngine(
                "INSERT...ON CONFLICT not supported: bypasses git storage; use explicit INSERT + UPDATE instead".into(),
            ));
        }

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
                        rows.push(extract_values(row)?);
                    }
                    rows
                }
                _ => {
                    return Err(ZettelError::SqlEngine(
                        "only VALUES clause supported".into(),
                    ))
                }
            },
            None => return Err(ZettelError::SqlEngine("missing VALUES clause".into())),
        };

        // Generate all IDs upfront
        let ids = self.unique_ids(rows.len())?;

        // Collect which referenced types use folder storage (for path-qualified wikilinks)
        let ref_folder_types: std::collections::HashSet<String> = schema
            .columns
            .iter()
            .filter_map(|c| c.references.as_ref())
            .filter(|ref_table| self.index.type_uses_folder(ref_table, self.repo))
            .cloned()
            .collect();

        let mut created_ids = Vec::with_capacity(rows.len());
        let mut files: Vec<(String, String)> = Vec::with_capacity(rows.len());

        for (row_values, id) in rows.iter().zip(ids.into_iter()) {
            if col_names.len() != row_values.len() {
                return Err(ZettelError::SqlEngine(
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
                        col_values.insert(col_def.name.clone(), default.clone());
                    }
                }
            }

            // Validate allowed_values constraints
            for col_def in &schema.columns {
                if let Some(ref allowed) = col_def.allowed_values {
                    if let Some(val) = col_values.get(&col_def.name) {
                        if !val.is_empty() && !allowed.contains(val) {
                            return Err(ZettelError::Validation(format!(
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
                                    "SELECT COUNT(*) > 0 FROM zettels WHERE id = ?1",
                                    params![ref_id],
                                    |row| row.get(0),
                                )
                                .unwrap_or(false);
                            if !exists {
                                return Err(ZettelError::SqlEngine(format!(
                                    "referenced zettel not found: {}",
                                    ref_id
                                )));
                            }
                        }
                    }
                }
            }

            // Build zettel
            let zettel = build_data_zettel(&id, &schema, &col_values, &ref_folder_types);
            let content = parser::serialize(&zettel);
            let path = if table_name == "zettels" {
                format!("zettelkasten/{}.md", id.0)
            } else {
                crate::git_ops::zettel_path(&id.0, Some(&table_name), schema.folder)
            };

            // Index the zettel
            let parsed = parser::parse(&content, &path)?;
            self.index.index_zettel(&parsed)?;

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

        Ok(SqlResult::Ok(created_ids.join(",")))
    }

    fn handle_update(
        &mut self,
        table: &sqlparser::ast::TableWithJoins,
        assignments: &[sqlparser::ast::Assignment],
        selection: &Option<Expr>,
    ) -> Result<SqlResult> {
        let table_name = unquote_identifier(&table.relation.to_string());
        let schema = self.load_schema(&table_name)?;

        // Build assignment map
        let mut updates: BTreeMap<String, String> = BTreeMap::new();
        for assignment in assignments {
            let col_name = match &assignment.target {
                AssignmentTarget::ColumnName(name) => name.to_string().to_lowercase(),
                AssignmentTarget::Tuple(names) => names
                    .iter()
                    .map(|n| n.to_string().to_lowercase())
                    .collect::<Vec<_>>()
                    .join("."),
            };
            let val = expr_to_string(&assignment.value)?;
            updates.insert(col_name, val);
        }

        // Validate allowed_values constraints
        for col_def in &schema.columns {
            if let Some(ref allowed) = col_def.allowed_values {
                if let Some(val) = updates.get(&col_def.name) {
                    if !val.is_empty() && !allowed.contains(val) {
                        return Err(ZettelError::Validation(format!(
                            "column '{}': value '{}' not in allowed values {:?}",
                            col_def.name, val, allowed
                        )));
                    }
                }
            }
        }

        // Fast path: single-row WHERE id = '...'
        if let Ok(zettel_id) = extract_where_id(selection) {
            let path = self.index.resolve_path(&zettel_id)?;
            let content = self.read_content(&path)?;
            let mut parsed = parser::parse(&content, &path)?;
            apply_updates_to_zettel(&mut parsed, &schema, &updates);
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
                    &format!("update {table_name} {zettel_id}"),
                )?;
            }
            let reparsed = parser::parse(&new_content, &path)?;
            self.index.index_zettel(&reparsed)?;
            self.update_materialized_row(&schema, &zettel_id, &updates)?;
            return Ok(SqlResult::Affected(1));
        }

        // Bulk path: resolve matching rows via SQLite
        let matches = self.resolve_matching_ids(&table_name, selection)?;
        if matches.is_empty() {
            return Ok(SqlResult::Affected(0));
        }

        let mut files: Vec<(String, String)> = Vec::with_capacity(matches.len());
        for (_, path) in &matches {
            let content = self.read_content(path)?;
            let mut parsed = parser::parse(&content, path)?;
            apply_updates_to_zettel(&mut parsed, &schema, &updates);
            files.push((path.clone(), parser::serialize(&parsed)));
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
        for (id, path) in &matches {
            let content = self.read_content(path)?;
            let reparsed = parser::parse(&content, path)?;
            self.index.index_zettel(&reparsed)?;
            self.update_materialized_row(&schema, id, &updates)?;
        }

        Ok(SqlResult::Affected(matches.len()))
    }

    fn handle_delete(&mut self, del: &sqlparser::ast::Delete) -> Result<SqlResult> {
        let from_tables = match &del.from {
            FromTable::WithFromKeyword(tables) | FromTable::WithoutKeyword(tables) => tables,
        };
        let table_name = from_tables
            .first()
            .map(|f| unquote_identifier(&f.relation.to_string()))
            .ok_or_else(|| ZettelError::SqlEngine("missing table in DELETE".into()))?;

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
        if let Ok(zettel_id) = extract_where_id(&del.selection) {
            let path = self.index.resolve_path(&zettel_id)?;
            if let Some(ref mut buf) = self.txn {
                buf.deletes.push(PendingDelete {
                    path: path.clone(),
                    zettel_id: zettel_id.clone(),
                });
            } else {
                self.repo
                    .delete_file(&path, &format!("delete from {table_name} {zettel_id}"))?;
            }
            self.index.remove_zettel(&zettel_id)?;
            self.index.conn.execute(
                &format!("DELETE FROM \"{}\" WHERE id = ?1", table_name),
                params![zettel_id],
            )?;
            return Ok(SqlResult::Affected(1));
        }

        // Bulk path: resolve matching rows via SQLite
        let matches = self.resolve_matching_ids(&table_name, &del.selection)?;
        if matches.is_empty() {
            return Ok(SqlResult::Affected(0));
        }

        if let Some(ref mut buf) = self.txn {
            for (id, path) in &matches {
                buf.deletes.push(PendingDelete {
                    path: path.clone(),
                    zettel_id: id.clone(),
                });
            }
        } else {
            let paths: Vec<&str> = matches.iter().map(|(_, p)| p.as_str()).collect();
            self.repo
                .delete_files(&paths, &format!("bulk delete from {table_name}"))?;
        }

        for (id, _) in &matches {
            self.index.remove_zettel(id)?;
            self.index.conn.execute(
                &format!("DELETE FROM \"{}\" WHERE id = ?1", table_name),
                params![id],
            )?;
        }

        Ok(SqlResult::Affected(matches.len()))
    }

    fn handle_alter_table(
        &mut self,
        name: &sqlparser::ast::ObjectName,
        operations: &[AlterTableOperation],
    ) -> Result<SqlResult> {
        let table_name = unquote_identifier(&name.to_string());
        let (typedef_id, typedef_path) = self.load_typedef_location(&table_name)?;
        let mut schema = self.load_schema(&table_name)?;

        for op in operations {
            match op {
                AlterTableOperation::AddColumn { column_def, .. } => {
                    let col_name = column_def.name.value.to_lowercase();
                    if schema.columns.iter().any(|c| c.name == col_name) {
                        return Err(ZettelError::SqlEngine(format!(
                            "column already exists: {col_name}"
                        )));
                    }
                    let dt = data_type_to_string(&column_def.data_type);
                    let refs = extract_references(&column_def.options);
                    let zone = if refs.is_some() {
                        Some(Zone::Reference)
                    } else if is_numeric_type(&dt)
                        || is_short_string_type(&column_def.data_type)
                    {
                        Some(Zone::Frontmatter)
                    } else {
                        Some(Zone::Body)
                    };
                    schema.columns.push(ColumnDef {
                        name: col_name,
                        data_type: dt,
                        zone,
                        required: false,
                        search_boost: None,
                        references: refs,
                        allowed_values: extract_allowed_values(&column_def.data_type),
                        default_value: extract_default(&column_def.options),
                    });
                }
                AlterTableOperation::DropColumn {
                    column_name,
                    if_exists,
                    ..
                } => {
                    let col_name = column_name.value.to_lowercase();
                    let pos = schema.columns.iter().position(|c| c.name == col_name);
                    match pos {
                        Some(i) => {
                            schema.columns.remove(i);
                        }
                        None if *if_exists => {}
                        None => {
                            return Err(ZettelError::SqlEngine(format!(
                                "column not found: {col_name}"
                            )));
                        }
                    }
                }
                AlterTableOperation::RenameColumn {
                    old_column_name,
                    new_column_name,
                } => {
                    return self.handle_rename_column(
                        &table_name,
                        &typedef_id,
                        &typedef_path,
                        &mut schema,
                        &old_column_name.value.to_lowercase(),
                        &new_column_name.value.to_lowercase(),
                    );
                }
                other => {
                    return Err(ZettelError::SqlEngine(format!(
                        "unsupported ALTER TABLE operation: {other}"
                    )));
                }
            }
        }

        self.update_typedef(&table_name, &schema)?;
        self.index.rematerialize_type(&table_name, self.repo)?;

        Ok(SqlResult::Ok(format!("table {table_name} altered")))
    }

    /// Serialize a modified TableSchema back to its typedef zettel, commit to Git, re-index.
    fn update_typedef(&mut self, table_name: &str, schema: &TableSchema) -> Result<()> {
        let (typedef_id, typedef_path) = self.load_typedef_location(table_name)?;
        let id = ZettelId(typedef_id);
        let schema_zettel = build_typedef_zettel(&id, schema);
        let content = parser::serialize(&schema_zettel);
        self.repo.commit_file(
            &typedef_path,
            &content,
            &format!("alter table {table_name}"),
        )?;
        let parsed = parser::parse(&content, &typedef_path)?;
        self.index.index_zettel(&parsed)?;
        Ok(())
    }

    fn handle_set_zone(
        &mut self,
        table_name: &str,
        zone_str: &str,
        column_name: &str,
    ) -> Result<SqlResult> {
        let mut schema = self.load_schema(table_name)?;
        let col_lower = column_name.to_lowercase();
        let col = schema
            .columns
            .iter_mut()
            .find(|c| c.name == col_lower)
            .ok_or_else(|| {
                ZettelError::SqlEngine(format!("column not found: {column_name}"))
            })?;
        let zone = match zone_str.to_lowercase().as_str() {
            "frontmatter" => Zone::Frontmatter,
            "body" => Zone::Body,
            "reference" => Zone::Reference,
            _ => {
                return Err(ZettelError::SqlEngine(format!(
                    "invalid zone: {zone_str} (use frontmatter, body, or reference)"
                )))
            }
        };
        col.zone = Some(zone);
        self.update_typedef(table_name, &schema)?;
        self.index.rematerialize_type(table_name, self.repo)?;
        Ok(SqlResult::Ok(format!(
            "zone set to {zone_str} for {col_lower} in {table_name}"
        )))
    }

    fn handle_title_template(
        &mut self,
        table_name: &str,
        template: Option<&str>,
    ) -> Result<SqlResult> {
        let mut schema = self.load_schema(table_name)?;
        schema.title_template = template.map(String::from);
        self.update_typedef(table_name, &schema)?;
        let action = if template.is_some() { "set" } else { "dropped" };
        Ok(SqlResult::Ok(format!(
            "title template {action} for {table_name}"
        )))
    }

    fn handle_rename_column(
        &mut self,
        table_name: &str,
        typedef_id: &str,
        typedef_path: &str,
        schema: &mut TableSchema,
        old_name: &str,
        new_name: &str,
    ) -> Result<SqlResult> {
        if schema.columns.iter().any(|c| c.name == new_name) {
            return Err(ZettelError::SqlEngine(format!(
                "column already exists: {new_name}"
            )));
        }
        let col = schema
            .columns
            .iter_mut()
            .find(|c| c.name == old_name)
            .ok_or_else(|| ZettelError::SqlEngine(format!("column not found: {old_name}")))?;
        let zone = effective_zone(col);
        col.name = new_name.to_string();

        let id = ZettelId(typedef_id.to_string());
        let schema_zettel = build_typedef_zettel(&id, schema);
        let typedef_content = parser::serialize(&schema_zettel);

        let data_zettels = self.resolve_matching_ids(table_name, &None)?;
        let mut files: Vec<(String, String)> = Vec::with_capacity(data_zettels.len() + 1);
        files.push((typedef_path.to_string(), typedef_content.clone()));

        for (_, path) in &data_zettels {
            let content = self.repo.read_file(path)?;
            let mut parsed = parser::parse(&content, path)?;
            rename_key_in_zettel(&mut parsed, old_name, new_name, &zone);
            files.push((path.clone(), parser::serialize(&parsed)));
        }

        let file_refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        self.repo.commit_files(
            &file_refs,
            &format!("alter table {table_name} rename {old_name} to {new_name}"),
        )?;

        let parsed_typedef = parser::parse(&typedef_content, typedef_path)?;
        self.index.index_zettel(&parsed_typedef)?;
        for (_, path) in &data_zettels {
            let content = self.repo.read_file(path)?;
            let parsed = parser::parse(&content, path)?;
            self.index.index_zettel(&parsed)?;
        }
        self.index.rematerialize_type(table_name, self.repo)?;

        Ok(SqlResult::Ok(format!(
            "renamed {old_name} to {new_name} in {table_name}"
        )))
    }

    fn handle_drop(
        &mut self,
        object_type: &ObjectType,
        if_exists: bool,
        names: &[sqlparser::ast::ObjectName],
        cascade: bool,
    ) -> Result<SqlResult> {
        if *object_type != ObjectType::Table {
            return Err(ZettelError::SqlEngine(format!(
                "DROP {} not supported, only DROP TABLE",
                object_type
            )));
        }

        for name in names {
            let table_name = unquote_identifier(&name.to_string());
            self.handle_drop_table(&table_name, if_exists, cascade)?;
        }

        Ok(SqlResult::Ok(format!("dropped {} table(s)", names.len())))
    }

    fn handle_drop_table(
        &mut self,
        table_name: &str,
        if_exists: bool,
        cascade: bool,
    ) -> Result<()> {
        // Locate typedef
        let typedef_loc = match self.load_typedef_location(table_name) {
            Ok(loc) => loc,
            Err(_) if if_exists => return Ok(()),
            Err(e) => return Err(e),
        };
        let (typedef_id, typedef_path) = typedef_loc;

        // Load schema before git deletes (needed for junction table cleanup)
        let schema = self.load_schema(table_name).ok();

        // Find all data zettels of this type
        let data_zettels: Vec<(String, String)> = {
            let mut stmt = self
                .index
                .conn
                .prepare("SELECT id, path FROM zettels WHERE type = ?1")?;
            let rows: Vec<(String, String)> = stmt
                .query_map(params![table_name], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .filter_map(|r| r.ok())
                .collect();
            rows
        };

        if cascade {
            // Delete typedef + all data zettels
            let mut paths: Vec<&str> = vec![&typedef_path];
            for (_, path) in &data_zettels {
                paths.push(path);
            }
            self.repo
                .delete_files(&paths, &format!("drop table {table_name} cascade"))?;

            // Remove from index
            self.index.remove_zettel(&typedef_id)?;
            for (id, _) in &data_zettels {
                self.index.remove_zettel(id)?;
            }
        } else {
            // Rewrite data zettels to remove type field, then delete typedef
            let mut writes: Vec<(String, String)> = Vec::new();
            for (_, path) in &data_zettels {
                let content = self.repo.read_file(path)?;
                let mut parsed = parser::parse(&content, path)?;
                parsed.meta.zettel_type = None;
                writes.push((path.clone(), parser::serialize(&parsed)));
            }

            let write_refs: Vec<(&str, &str)> = writes
                .iter()
                .map(|(p, c)| (p.as_str(), c.as_str()))
                .collect();
            self.repo.commit_batch(
                &write_refs,
                &[&typedef_path],
                &format!("drop table {table_name}"),
            )?;

            // Re-index modified data zettels
            for (_, path) in &data_zettels {
                let content = self.repo.read_file(path)?;
                let parsed = parser::parse(&content, path)?;
                self.index.index_zettel(&parsed)?;
            }
            // Remove typedef from index
            self.index.remove_zettel(&typedef_id)?;
        }

        // Drop junction tables for REFERENCES columns
        if let Some(ref schema) = schema {
            for col in &schema.columns {
                if col.references.is_some() {
                    self.index.conn.execute(
                        &format!("DROP TABLE IF EXISTS \"{table_name}_{col_name}\"", col_name = col.name),
                        [],
                    )?;
                }
            }
        }

        // Drop materialized SQLite table
        self.index
            .conn
            .execute(&format!("DROP TABLE IF EXISTS \"{table_name}\""), [])?;

        Ok(())
    }

    fn load_typedef_location(&mut self, table_name: &str) -> Result<(String, String)> {
        self.index
            .conn
            .query_row(
                "SELECT id, path FROM zettels WHERE type = '_typedef' AND title = ?1",
                params![table_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| ZettelError::SqlEngine(format!("table not found: {table_name}")))
    }

    /// Resolve zettel ids and paths matching a WHERE clause via SQLite.
    /// When `selection` is None, returns all rows of the table.
    fn resolve_matching_ids(
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
            ZettelError::SqlEngine(format!(
                "invalid WHERE clause{}: {e}",
                where_clause
                    .as_deref()
                    .map(|c| format!(" ({c})"))
                    .unwrap_or_default()
            ))
        })?;
        let ids: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| ZettelError::SqlEngine(format!("query failed: {e}")))?
            .filter_map(|r| r.ok())
            .collect();

        let mut result = Vec::with_capacity(ids.len());
        for id in ids {
            let path = self.index.resolve_path(&id)?;
            result.push((id, path));
        }
        Ok(result)
    }

    fn load_schema(&mut self, table_name: &str) -> Result<TableSchema> {
        let (_id, path) = self.load_typedef_location(table_name)?;
        let content = self.repo.read_file(&path)?;
        let parsed = parser::parse(&content, &path)?;
        schema_from_parsed(&parsed)
    }

    /// Check if `table_name` is a junction table (`{type}_{col}` where type is
    /// a known typedef and col is a REFERENCES column in that typedef).
    /// Returns `Some((type_name, col_name))` if it is.
    fn resolve_junction_table(&mut self, table_name: &str) -> Result<Option<(String, String)>> {
        // Try each possible split point of `type_col`
        for (i, _) in table_name.match_indices('_') {
            let candidate_type = &table_name[..i];
            let candidate_col = &table_name[i + 1..];
            if candidate_type.is_empty() || candidate_col.is_empty() {
                continue;
            }
            // Check if candidate_type is a known typedef
            if self.load_typedef_location(candidate_type).is_ok() {
                let schema = self.load_schema(candidate_type)?;
                // Check if candidate_col is a REFERENCES column
                if schema
                    .columns
                    .iter()
                    .any(|c| c.name == candidate_col && c.references.is_some())
                {
                    return Ok(Some((candidate_type.to_string(), candidate_col.to_string())));
                }
            }
        }
        Ok(None)
    }

    /// Handle INSERT into a junction table by appending reference lines to the
    /// parent zettel and re-indexing.
    fn handle_junction_insert(
        &mut self,
        ins: &sqlparser::ast::Insert,
        type_name: &str,
        col_name: &str,
    ) -> Result<SqlResult> {
        let col_names: Vec<String> = ins.columns.iter().map(|c| c.value.to_lowercase()).collect();
        let type_id_col = format!("{type_name}_id");
        let ref_id_col = format!("{col_name}_id");

        let rows = match ins.source.as_ref() {
            Some(query) => match query.body.as_ref() {
                SetExpr::Values(v) => {
                    let mut rows = Vec::with_capacity(v.rows.len());
                    for row in &v.rows {
                        rows.push(extract_values(row)?);
                    }
                    rows
                }
                _ => {
                    return Err(ZettelError::SqlEngine(
                        "only VALUES clause supported for junction INSERT".into(),
                    ))
                }
            },
            None => {
                return Err(ZettelError::SqlEngine(
                    "missing VALUES clause".into(),
                ))
            }
        };

        let schema = self.load_schema(type_name)?;
        let ref_col = schema
            .columns
            .iter()
            .find(|c| c.name == col_name && c.references.is_some())
            .ok_or_else(|| {
                ZettelError::SqlEngine(format!(
                    "column {col_name} not found or not a REFERENCES column"
                ))
            })?;
        let ref_folder_types: std::collections::HashSet<String> = schema
            .columns
            .iter()
            .filter_map(|c| c.references.as_ref())
            .filter(|ref_table| self.index.type_uses_folder(ref_table, self.repo))
            .cloned()
            .collect();

        let mut affected = 0;
        for row_values in &rows {
            let parent_id_idx = col_names
                .iter()
                .position(|c| *c == type_id_col)
                .ok_or_else(|| {
                    ZettelError::SqlEngine(format!("missing {type_id_col} column in INSERT"))
                })?;
            let target_id_idx = col_names
                .iter()
                .position(|c| *c == ref_id_col)
                .ok_or_else(|| {
                    ZettelError::SqlEngine(format!("missing {ref_id_col} column in INSERT"))
                })?;

            let parent_id = &row_values[parent_id_idx];
            let target_id = &row_values[target_id_idx];

            // Read parent zettel
            let path = self.index.resolve_path(parent_id)?;
            let content = self.repo.read_file(&path)?;
            let mut parsed = parser::parse(&content, &path)?;

            // Build the reference line with folder-qualified link if needed
            let link_target =
                if let Some(ref ref_table) = ref_col.references {
                    if ref_folder_types.contains(ref_table) {
                        format!("zettelkasten/{ref_table}/{target_id}.md")
                    } else {
                        target_id.clone()
                    }
                } else {
                    target_id.clone()
                };
            let ref_line = format!("- {}:: [[{}]]", col_name, link_target);

            // Append to reference section
            let trimmed = parsed.reference_section.trim_end();
            parsed.reference_section = if trimmed.is_empty() {
                format!("{ref_line}\n")
            } else {
                format!("{trimmed}\n{ref_line}\n")
            };

            // Serialize and commit
            let new_content = parser::serialize(&parsed);
            self.repo.commit_file(
                &path,
                &new_content,
                &format!("add {col_name} ref {target_id} to {type_name} {parent_id}"),
            )?;

            // Re-index this zettel
            let re_parsed = parser::parse(&new_content, &path)?;
            self.index.index_zettel(&re_parsed)?;
            self.index
                .materialize_single(&schema, parent_id, &re_parsed)?;

            affected += 1;
        }

        Ok(SqlResult::Affected(affected))
    }

    /// Handle DELETE from a junction table by removing matching reference lines
    /// from the parent zettel and re-indexing.
    fn handle_junction_delete(
        &mut self,
        type_name: &str,
        col_name: &str,
        parent_id: &str,
        target_id: &str,
    ) -> Result<SqlResult> {
        let schema = self.load_schema(type_name)?;
        let ref_col = schema
            .columns
            .iter()
            .find(|c| c.name == col_name && c.references.is_some())
            .ok_or_else(|| {
                ZettelError::SqlEngine(format!(
                    "column {col_name} not found or not a REFERENCES column"
                ))
            })?;
        let ref_folder_types: std::collections::HashSet<String> = schema
            .columns
            .iter()
            .filter_map(|c| c.references.as_ref())
            .filter(|ref_table| self.index.type_uses_folder(ref_table, self.repo))
            .cloned()
            .collect();

        // Read parent zettel
        let path = self.index.resolve_path(parent_id)?;
        let content = self.repo.read_file(&path)?;
        let mut parsed = parser::parse(&content, &path)?;

        // Build the reference line pattern to remove
        let link_target =
            if let Some(ref ref_table) = ref_col.references {
                if ref_folder_types.contains(ref_table) {
                    format!("zettelkasten/{ref_table}/{target_id}.md")
                } else {
                    target_id.to_string()
                }
            } else {
                target_id.to_string()
            };
        let ref_line = format!("- {}:: [[{}]]", col_name, link_target);

        // Remove matching line from reference section
        let old_section = parsed.reference_section.clone();
        let new_lines: Vec<&str> = old_section
            .lines()
            .filter(|line| line.trim() != ref_line.trim())
            .collect();
        parsed.reference_section = if new_lines.is_empty() {
            String::new()
        } else {
            format!("{}\n", new_lines.join("\n"))
        };

        // Serialize and commit
        let new_content = parser::serialize(&parsed);
        self.repo.commit_file(
            &path,
            &new_content,
            &format!("remove {col_name} ref {target_id} from {type_name} {parent_id}"),
        )?;

        // Re-index
        let re_parsed = parser::parse(&new_content, &path)?;
        self.index.index_zettel(&re_parsed)?;
        self.index
            .materialize_single(&schema, parent_id, &re_parsed)?;

        Ok(SqlResult::Affected(1))
    }

    fn insert_materialized_row(
        &mut self,
        schema: &TableSchema,
        id: &str,
        col_values: &BTreeMap<String, String>,
    ) -> Result<()> {
        let mut col_names = vec!["id".to_string()];
        let mut placeholders = vec!["?1".to_string()];
        let mut vals: Vec<Option<String>> = vec![Some(id.to_string())];

        for (i, col) in schema.columns.iter().enumerate() {
            col_names.push(format!("\"{}\"", col.name));
            placeholders.push(format!("?{}", i + 2));
            let val = col_values.get(&col.name).cloned().unwrap_or_default();
            vals.push(if val.is_empty() { None } else { Some(val) });
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
        let valid_cols: Vec<&String> = schema.columns.iter().map(|c| &c.name).collect();
        let mut set_clauses = Vec::new();
        let mut vals: Vec<String> = Vec::new();

        for (col, val) in updates {
            if valid_cols.contains(&col) {
                vals.push(val.clone());
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

// --- Helper functions ---

fn data_type_to_string(dt: &DataType) -> String {
    match dt {
        DataType::Char(Some(CharacterLength::IntegerLength { length, .. })) => {
            format!("CHAR({length})")
        }
        DataType::Char(None) => "CHAR".into(),
        DataType::Character(Some(CharacterLength::IntegerLength { length, .. })) => {
            format!("CHAR({length})")
        }
        DataType::Character(None) => "CHAR".into(),
        DataType::Varchar(Some(CharacterLength::IntegerLength { length, .. }))
        | DataType::CharVarying(Some(CharacterLength::IntegerLength { length, .. })) => {
            format!("VARCHAR({length})")
        }
        DataType::Varchar(_) | DataType::CharVarying(_) => "VARCHAR".into(),
        DataType::TinyText => "TINYTEXT".into(),
        DataType::Text => "TEXT".into(),
        DataType::MediumText => "MEDIUMTEXT".into(),
        DataType::LongText => "LONGTEXT".into(),
        DataType::TinyBlob => "TINYBLOB".into(),
        DataType::Blob(_) => "BLOB".into(),
        DataType::MediumBlob => "MEDIUMBLOB".into(),
        DataType::LongBlob => "LONGBLOB".into(),
        DataType::Binary(_) => "BINARY".into(),
        DataType::Varbinary(_) => "VARBINARY".into(),
        DataType::Enum(..) | DataType::Set(_) => "TEXT".into(),
        DataType::Integer(_) | DataType::Int(_) | DataType::BigInt(_) | DataType::SmallInt(_) => {
            "INTEGER".into()
        }
        DataType::Real | DataType::Float(_) | DataType::Double(_) | DataType::DoublePrecision => {
            "REAL".into()
        }
        DataType::Boolean => "BOOLEAN".into(),
        _ => "TEXT".into(),
    }
}

fn extract_references(options: &[sqlparser::ast::ColumnOptionDef]) -> Option<String> {
    for opt in options {
        if let ColumnOption::ForeignKey { foreign_table, .. } = &opt.option {
            return Some(unquote_identifier(&foreign_table.to_string()));
        }
    }
    None
}

fn extract_allowed_values(dt: &DataType) -> Option<Vec<String>> {
    match dt {
        DataType::Enum(members, _) => {
            let vals: Vec<String> = members
                .iter()
                .map(|m| match m {
                    sqlparser::ast::EnumMember::Name(n) => n.clone(),
                    sqlparser::ast::EnumMember::NamedValue(n, _) => n.clone(),
                })
                .collect();
            if vals.is_empty() {
                None
            } else {
                Some(vals)
            }
        }
        DataType::Set(vals) => {
            if vals.is_empty() {
                None
            } else {
                Some(vals.clone())
            }
        }
        _ => None,
    }
}

fn extract_default(options: &[sqlparser::ast::ColumnOptionDef]) -> Option<String> {
    for opt in options {
        if let ColumnOption::Default(expr) = &opt.option {
            return expr_to_string(expr).ok();
        }
    }
    None
}

fn extract_values(exprs: &[Expr]) -> Result<Vec<String>> {
    exprs.iter().map(expr_to_string).collect()
}

fn expr_to_string(expr: &Expr) -> Result<String> {
    match expr {
        Expr::Value(v) => match &v.value {
            SqlValue::SingleQuotedString(s) => Ok(s.clone()),
            SqlValue::DoubleQuotedString(s) => Ok(s.clone()),
            SqlValue::Number(n, _) => Ok(n.clone()),
            SqlValue::Boolean(b) => Ok(b.to_string()),
            SqlValue::Null => Ok(String::new()),
            _ => Err(ZettelError::SqlEngine(format!("unsupported value: {v}"))),
        },
        Expr::UnaryOp { op, expr } => {
            let inner = expr_to_string(expr)?;
            Ok(format!("{op}{inner}"))
        }
        _ => Err(ZettelError::SqlEngine(format!(
            "unsupported expression: {expr}"
        ))),
    }
}

fn extract_where_id(selection: &Option<Expr>) -> Result<String> {
    match selection {
        Some(Expr::BinaryOp { left, op, right }) => {
            if format!("{op}") != "=" {
                return Err(ZettelError::SqlEngine(
                    "only WHERE id = '<value>' supported".into(),
                ));
            }
            let col = match left.as_ref() {
                Expr::Identifier(ident) => ident.value.to_lowercase(),
                _ => {
                    return Err(ZettelError::SqlEngine(
                        "WHERE clause must be id = '<value>'".into(),
                    ))
                }
            };
            if col != "id" {
                return Err(ZettelError::SqlEngine(
                    "only WHERE id = '<value>' supported".into(),
                ));
            }
            expr_to_string(right)
        }
        _ => Err(ZettelError::SqlEngine(
            "WHERE id = '<value>' required".into(),
        )),
    }
}

/// Extract two column values from a WHERE clause like
/// `{col1} = 'val1' AND {col2} = 'val2'`.
fn extract_junction_where(
    selection: &Option<Expr>,
    col1_name: &str,
    col2_name: &str,
) -> Result<(String, String)> {
    match selection {
        Some(Expr::BinaryOp { left, op, right }) if format!("{op}") == "AND" => {
            let mut val1 = None;
            let mut val2 = None;
            for side in [left.as_ref(), right.as_ref()] {
                if let Expr::BinaryOp {
                    left: inner_left,
                    op: inner_op,
                    right: inner_right,
                } = side
                {
                    if format!("{inner_op}") == "=" {
                        if let Expr::Identifier(ident) = inner_left.as_ref() {
                            let col = ident.value.to_lowercase();
                            if col == col1_name {
                                val1 = Some(expr_to_string(inner_right)?);
                            } else if col == col2_name {
                                val2 = Some(expr_to_string(inner_right)?);
                            }
                        }
                    }
                }
            }
            match (val1, val2) {
                (Some(v1), Some(v2)) => Ok((v1, v2)),
                _ => Err(ZettelError::SqlEngine(format!(
                    "junction DELETE requires WHERE {col1_name} = '...' AND {col2_name} = '...'"
                ))),
            }
        }
        _ => Err(ZettelError::SqlEngine(format!(
            "junction DELETE requires WHERE {col1_name} = '...' AND {col2_name} = '...'"
        ))),
    }
}

/// Build a _typedef zettel from a TableSchema.
pub fn build_typedef_zettel(id: &ZettelId, schema: &TableSchema) -> ParsedZettel {
    let mut extra = BTreeMap::new();

    let columns_yaml: Vec<Value> = schema
        .columns
        .iter()
        .map(|col| {
            let mut map = BTreeMap::new();
            map.insert("name".to_string(), Value::String(col.name.clone()));
            map.insert(
                "data_type".to_string(),
                Value::String(col.data_type.clone()),
            );
            if let Some(ref zone) = col.zone {
                let zone_str = match zone {
                    Zone::Frontmatter => "frontmatter",
                    Zone::Body => "body",
                    Zone::Reference => "reference",
                };
                map.insert("zone".to_string(), Value::String(zone_str.into()));
            }
            if col.required {
                map.insert("required".to_string(), Value::Bool(true));
            }
            if let Some(boost) = col.search_boost {
                map.insert("search_boost".to_string(), Value::Number(boost));
            }
            if let Some(ref r) = col.references {
                map.insert("references".to_string(), Value::String(r.clone()));
            }
            if let Some(ref vals) = col.allowed_values {
                map.insert(
                    "allowed_values".to_string(),
                    Value::List(vals.iter().map(|v| Value::String(v.clone())).collect()),
                );
            }
            if let Some(ref default) = col.default_value {
                map.insert("default_value".to_string(), Value::String(default.clone()));
            }
            Value::Map(map)
        })
        .collect();

    extra.insert("columns".to_string(), Value::List(columns_yaml));

    if let Some(ref strategy) = schema.crdt_strategy {
        extra.insert("crdt_strategy".to_string(), Value::String(strategy.clone()));
    }

    if !schema.template_sections.is_empty() {
        extra.insert(
            "template_sections".to_string(),
            Value::List(
                schema
                    .template_sections
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
    }

    if schema.folder {
        extra.insert("folder".to_string(), Value::Bool(true));
    }

    if let Some(ref tt) = schema.title_template {
        extra.insert("title_template".to_string(), Value::String(tt.clone()));
    }

    if let Some(ref o) = schema.origin {
        extra.insert("origin".to_string(), Value::String(o.clone()));
    }

    ParsedZettel {
        meta: ZettelMeta {
            id: Some(id.clone()),
            title: Some(schema.table_name.clone()),
            date: None,
            zettel_type: Some("_typedef".into()),
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
        path: format!("zettelkasten/_typedef/{}.md", id.0),
    }
}

/// Build a data zettel from column values according to the schema's zone mapping.
fn build_data_zettel(
    id: &ZettelId,
    schema: &TableSchema,
    col_values: &BTreeMap<String, String>,
    ref_folder_types: &std::collections::HashSet<String>,
) -> ParsedZettel {
    let mut extra = BTreeMap::new();
    let mut body_sections: Vec<String> = Vec::new();
    let mut ref_lines: Vec<String> = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let mut inline_fields: Vec<InlineField> = Vec::new();

    // Priority 1: explicit title from INSERT column list
    let mut title_value: Option<String> = col_values.get("title").cloned();

    // Priority 2: title_template interpolation
    if title_value.is_none() {
        if let Some(ref tmpl) = schema.title_template {
            let mut rendered = tmpl.clone();
            for (key, val) in col_values {
                rendered = rendered.replace(&format!("{{{key}}}"), val);
            }
            let rendered = re_unfilled_placeholder()
                .replace_all(&rendered, "")
                .trim()
                .to_string();
            if !rendered.is_empty() {
                title_value = Some(rendered);
            }
        }
    }

    // Track first frontmatter string column for Priority 4 fallback
    let mut first_fm_string: Option<String> = None;

    for col in &schema.columns {
        let val = match col_values.get(&col.name) {
            Some(v) => v.clone(),
            None => continue,
        };

        match effective_zone(col) {
            Zone::Reference => {
                let link_target = if let Some(ref ref_table) = col.references {
                    if ref_folder_types.contains(ref_table) {
                        format!("zettelkasten/{ref_table}/{val}.md")
                    } else {
                        val.clone()
                    }
                } else {
                    val.clone()
                };
                ref_lines.push(format!("- {}:: [[{}]]", col.name, link_target));
                links.push(Link {
                    target: link_target.clone(),
                    display: None,
                    section: None,
                    kind: crate::types::LinkKind::WikiLink,
                    zone: Zone::Reference,
                });
                inline_fields.push(InlineField {
                    key: col.name.clone(),
                    value: link_target.clone(),
                    zone: Zone::Reference,
                });
            }
            Zone::Frontmatter => {
                // Priority 4: track first frontmatter string column
                if first_fm_string.is_none() && !is_numeric_type(&col.data_type) {
                    first_fm_string = Some(val.clone());
                }
                extra.insert(col.name.clone(), to_yaml_value(&val, &col.data_type));
            }
            Zone::Body => {
                // Priority 3: first body column value
                if title_value.is_none() {
                    title_value = Some(val.clone());
                }
                body_sections.push(format!("## {}\n\n{}", col.name, val));
            }
        }
    }

    // Priority 4: first frontmatter string column
    if title_value.is_none() {
        title_value = first_fm_string;
    }

    // Priority 5: "{type} {id}" fallback
    if title_value.is_none() {
        title_value = Some(format!("{} {}", schema.table_name, id.0));
    }

    let body = if body_sections.is_empty() {
        String::new()
    } else {
        format!("\n{}\n", body_sections.join("\n\n"))
    };

    let reference_section = if ref_lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", ref_lines.join("\n"))
    };

    ParsedZettel {
        meta: ZettelMeta {
            id: Some(id.clone()),
            title: title_value,
            date: None,
            zettel_type: Some(schema.table_name.clone()),
            tags: vec![],
            extra,
        },
        body,
        sections: vec![],
        reference_section,
        inline_fields,
        links,
        body_tags: vec![],
        checkboxes: vec![],
        path: format!("zettelkasten/{}.md", id.0),
    }
}

fn is_numeric_type(dt: &str) -> bool {
    matches!(dt.to_uppercase().as_str(), "INTEGER" | "REAL" | "BOOLEAN")
}

/// Determine if a SQL data type represents a short string (<=255 chars) that
/// should default to frontmatter zone rather than body.
fn is_short_string_type(dt: &DataType) -> bool {
    match dt {
        DataType::Char(_) | DataType::Character(_) | DataType::TinyText => true,
        DataType::Varchar(Some(CharacterLength::IntegerLength { length, .. }))
        | DataType::CharVarying(Some(CharacterLength::IntegerLength { length, .. })) => {
            *length <= 255
        }
        // No size specified — assume short
        DataType::Varchar(None) | DataType::CharVarying(None) => true,
        DataType::Enum(..) | DataType::Set(_) => true,
        _ => false,
    }
}

/// Check if a stringified data type represents a short string (for effective_zone fallback).
fn is_short_string_type_str(dt: &str) -> bool {
    let upper = dt.to_uppercase();
    if matches!(upper.as_str(), "CHAR" | "TINYTEXT" | "VARCHAR") {
        return true;
    }
    if let Some(rest) = upper.strip_prefix("CHAR(") {
        return rest.ends_with(')');
    }
    if let Some(rest) = upper.strip_prefix("VARCHAR(") {
        if let Some(num_str) = rest.strip_suffix(')') {
            return num_str.parse::<u64>().is_ok_and(|n| n <= 255);
        }
    }
    false
}

/// Resolve the effective zone for a column, falling back to type-based inference.
fn effective_zone(col: &ColumnDef) -> Zone {
    if let Some(ref zone) = col.zone {
        return zone.clone();
    }
    if col.references.is_some() {
        Zone::Reference
    } else if is_numeric_type(&col.data_type)
        || is_short_string_type_str(&col.data_type)
        || col.allowed_values.is_some()
    {
        Zone::Frontmatter
    } else {
        Zone::Body
    }
}

fn to_yaml_value(val: &str, data_type: &str) -> Value {
    match data_type.to_uppercase().as_str() {
        "INTEGER" => val
            .parse::<i64>()
            .map(|i| Value::Number(i as f64))
            .unwrap_or_else(|_| Value::String(val.into())),
        "REAL" => val
            .parse::<f64>()
            .map(Value::Number)
            .unwrap_or_else(|_| Value::String(val.into())),
        "BOOLEAN" => {
            let b = matches!(val.to_lowercase().as_str(), "true" | "1" | "yes");
            Value::Bool(b)
        }
        _ => Value::String(val.into()),
    }
}

/// Extract a TableSchema from a parsed _typedef zettel.
pub fn schema_from_parsed(zettel: &ParsedZettel) -> Result<TableSchema> {
    let table_name = zettel
        .meta
        .title
        .as_deref()
        .ok_or_else(|| ZettelError::SqlEngine("typedef zettel missing title".into()))?
        .to_string();

    let columns_val = zettel
        .meta
        .extra
        .get("columns")
        .ok_or_else(|| ZettelError::SqlEngine("typedef zettel missing columns".into()))?;

    let columns_seq = columns_val
        .as_sequence()
        .ok_or_else(|| ZettelError::SqlEngine("columns must be a sequence".into()))?;

    let mut columns = Vec::new();
    for item in columns_seq {
        let map = item
            .as_mapping()
            .ok_or_else(|| ZettelError::SqlEngine("column must be a mapping".into()))?;
        let name = map
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZettelError::SqlEngine("column missing name".into()))?
            .to_string();
        let data_type = map
            .get("data_type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ZettelError::SqlEngine("column missing data_type".into()))?
            .to_string();
        let references = map
            .get("references")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let zone = map.get("zone").and_then(|v| v.as_str()).map(|s| match s {
            "frontmatter" => Zone::Frontmatter,
            "body" => Zone::Body,
            "reference" => Zone::Reference,
            _ => Zone::Body,
        });
        let required = map
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let search_boost = map.get("search_boost").and_then(|v| v.as_f64());
        let allowed_values = map
            .get("allowed_values")
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            });
        let default_value = map
            .get("default_value")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        columns.push(ColumnDef {
            name,
            data_type,
            references,
            zone,
            required,
            search_boost,
            allowed_values,
            default_value,
        });
    }

    let crdt_strategy = zettel
        .meta
        .extra
        .get("crdt_strategy")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let template_sections = zettel
        .meta
        .extra
        .get("template_sections")
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let folder = zettel
        .meta
        .extra
        .get("folder")
        .map(|v| matches!(v, crate::types::Value::Bool(true)) || v.as_str() == Some("true"))
        .unwrap_or(false);

    let stale_after_days = zettel
        .meta
        .extra
        .get("stale_after_days")
        .and_then(|v| v.as_f64())
        .map(|n| n as u32);

    let title_template = zettel
        .meta
        .extra
        .get("title_template")
        .and_then(|v| v.as_str())
        .map(String::from);

    let origin = zettel
        .meta
        .extra
        .get("origin")
        .and_then(|v| v.as_str())
        .map(String::from);

    Ok(TableSchema {
        table_name,
        columns,
        crdt_strategy,
        template_sections,
        folder,
        stale_after_days,
        title_template,
        origin,
    })
}

/// Apply UPDATE SET assignments to a ParsedZettel according to schema zone mapping.
fn apply_updates_to_zettel(
    zettel: &mut ParsedZettel,
    schema: &TableSchema,
    updates: &BTreeMap<String, String>,
) {
    for (col_name, new_val) in updates {
        let col_def = schema.columns.iter().find(|c| c.name == *col_name);
        let col_def = match col_def {
            Some(c) => c,
            None => continue,
        };

        match effective_zone(col_def) {
            Zone::Reference => {
                update_reference_line(&mut zettel.reference_section, col_name, new_val);
            }
            Zone::Frontmatter => {
                zettel
                    .meta
                    .extra
                    .insert(col_name.clone(), to_yaml_value(new_val, &col_def.data_type));
            }
            Zone::Body => {
                update_body_section(&mut zettel.body, col_name, new_val);
                if let Some(first_body) = schema
                    .columns
                    .iter()
                    .find(|c| effective_zone(c) == Zone::Body)
                {
                    if first_body.name == *col_name {
                        zettel.meta.title = Some(new_val.clone());
                    }
                }
            }
        }
    }
}

fn update_body_section(body: &mut String, section_name: &str, new_val: &str) {
    let heading = format!("## {section_name}");
    let lines: Vec<&str> = body.lines().collect();
    let mut result = Vec::new();
    let mut i = 0;
    let mut found = false;

    while i < lines.len() {
        if lines[i].trim() == heading {
            found = true;
            result.push(lines[i]);
            // Skip blank line after heading
            i += 1;
            if i < lines.len() && lines[i].trim().is_empty() {
                result.push("");
            }
            i += 1;
            // Skip old content until next heading or end
            while i < lines.len() && !lines[i].starts_with("## ") {
                i += 1;
            }
            // Insert new value
            result.push(new_val);
            // Add blank line before next section if there is one
            if i < lines.len() {
                result.push("");
            }
        } else {
            result.push(lines[i]);
            i += 1;
        }
    }

    if !found {
        // Append new section
        if !result.is_empty() && !result.last().is_none_or(|l| l.trim().is_empty()) {
            result.push("");
        }
        result.push(&heading);
        result.push("");
        result.push(new_val);
    }

    *body = result.join("\n");
}

fn update_reference_line(reference: &mut String, key: &str, new_val: &str) {
    let prefix = format!("- {key}::");
    let new_line = format!("- {key}:: [[{new_val}]]");
    let lines: Vec<&str> = reference.lines().collect();
    let mut result = Vec::new();
    let mut found = false;

    for line in &lines {
        if line.starts_with(&prefix) {
            result.push(new_line.as_str());
            found = true;
        } else {
            result.push(line);
        }
    }

    if !found {
        result.push(&new_line);
    }

    *reference = format!("{}\n", result.join("\n"));
}

/// Rename a key in a parsed zettel within the appropriate zone.
fn rename_key_in_zettel(zettel: &mut ParsedZettel, old_name: &str, new_name: &str, zone: &Zone) {
    match zone {
        Zone::Frontmatter => {
            if let Some(val) = zettel.meta.extra.remove(old_name) {
                zettel.meta.extra.insert(new_name.to_string(), val);
            }
        }
        Zone::Body => {
            let old_heading = format!("## {old_name}");
            let new_heading = format!("## {new_name}");
            zettel.body = zettel.body.replace(&old_heading, &new_heading);
        }
        Zone::Reference => {
            let old_prefix = format!("- {old_name}::");
            let new_prefix = format!("- {new_name}::");
            zettel.reference_section = zettel.reference_section.replace(&old_prefix, &new_prefix);
        }
    }
}

// Test helpers
#[cfg(test)]
fn engine_exec_ok(repo: &crate::git_ops::GitRepo, index: &crate::indexer::Index, sql: &str) {
    let mut engine = SqlEngine::new(index, repo);
    engine.execute(sql).unwrap();
}

#[cfg(test)]
fn engine_exec_id(
    repo: &crate::git_ops::GitRepo,
    index: &crate::indexer::Index,
    sql: &str,
) -> String {
    let mut engine = SqlEngine::new(index, repo);
    match engine.execute(sql).unwrap() {
        SqlResult::Ok(id) => id,
        _ => panic!("expected Ok"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_ops::GitRepo;
    use crate::indexer::Index;
    use tempfile::TempDir;

    fn setup() -> (TempDir, GitRepo, Index) {
        let dir = TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        let db_path = dir.path().join(".zdb/index.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let index = Index::open(&db_path).unwrap();
        (dir, repo, index)
    }

    #[test]
    fn create_table_produces_typedef_zettel() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        let result = engine
            .execute("CREATE TABLE projects (name TEXT, priority INTEGER)")
            .unwrap();

        match result {
            SqlResult::Ok(msg) => assert!(msg.contains("projects")),
            _ => panic!("expected Ok"),
        }

        // Typedef zettel should be in index
        let rows = index
            .query_raw("SELECT title, type FROM zettels WHERE type = '_typedef'")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "projects");
        assert_eq!(rows[0][1], "_typedef");

        // Materialized table should exist
        let rows = index.query_raw("SELECT COUNT(*) FROM projects").unwrap();
        assert_eq!(rows[0][0], "0");
    }

    #[test]
    fn create_table_rejects_reserved_names() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        let err = engine
            .execute("CREATE TABLE zettels (name TEXT)")
            .unwrap_err();
        assert!(format!("{err}").contains("reserved"));

        let err = engine
            .execute("CREATE TABLE _zdb_foo (name TEXT)")
            .unwrap_err();
        assert!(format!("{err}").contains("reserved"));
    }

    #[test]
    fn create_table_rejects_duplicate() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine.execute("CREATE TABLE projects (name TEXT)").unwrap();
        let err = engine
            .execute("CREATE TABLE projects (name TEXT)")
            .unwrap_err();
        assert!(format!("{err}").contains("already exists"));
    }

    #[test]
    fn create_table_if_not_exists_is_idempotent() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE IF NOT EXISTS projects (name TEXT)")
            .unwrap();
        // Second call with IF NOT EXISTS should succeed (no-op)
        let result = engine
            .execute("CREATE TABLE IF NOT EXISTS projects (name TEXT)")
            .unwrap();
        match &result {
            SqlResult::Ok(msg) => assert!(msg.contains("skipped")),
            other => panic!("expected SqlResult::Ok, got {other:?}"),
        }

        // Without IF NOT EXISTS should still error
        let err = engine
            .execute("CREATE TABLE projects (name TEXT)")
            .unwrap_err();
        assert!(format!("{err}").contains("already exists"));
    }

    #[test]
    fn create_table_with_references() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine.execute("CREATE TABLE people (name TEXT)").unwrap();
        engine
            .execute("CREATE TABLE tasks (name TEXT, assignee TEXT REFERENCES people(id))")
            .unwrap();

        // Check materialized table has correct columns
        let rows = index.query_raw("PRAGMA table_info(tasks)").unwrap();
        let col_names: Vec<&str> = rows.iter().map(|r| r[1].as_str()).collect();
        assert!(col_names.contains(&"id"));
        assert!(col_names.contains(&"name"));
        assert!(col_names.contains(&"assignee"));
    }

    #[test]
    fn insert_creates_zettel_and_materialized_row() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE projects (name TEXT, status TEXT, priority INTEGER)")
            .unwrap();

        let result = engine
            .execute("INSERT INTO projects (name, status, priority) VALUES ('Alpha', 'active', 1)")
            .unwrap();

        let zettel_id = match result {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok with id"),
        };

        // Check materialized table
        let rows = index
            .query_raw("SELECT name, status, priority FROM projects")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "Alpha");
        assert_eq!(rows[0][1], "active");
        assert_eq!(rows[0][2], "1");

        // Check zettel exists in index
        let rows = index
            .query_raw(&format!(
                "SELECT title, type FROM zettels WHERE id = '{zettel_id}'"
            ))
            .unwrap();
        assert_eq!(rows[0][0], "Alpha"); // title = first TEXT column value
        assert_eq!(rows[0][1], "projects");

        // Check zettel file in Git (no folder: true → flat path)
        let path = index.resolve_path(&zettel_id).unwrap();
        assert!(path.starts_with("zettelkasten/") && !path.contains("projects/"));
        let content = repo.read_file(&path).unwrap();
        assert!(content.contains("type: projects"));
        assert!(content.contains("priority: 1"));
        assert!(content.contains("## name"));
        assert!(content.contains("Alpha"));
    }

    #[test]
    fn insert_multi_row_creates_n_zettels() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE items (name TEXT, score INTEGER)")
            .unwrap();

        let result = engine
            .execute(
                "INSERT INTO items (name, score) VALUES ('alpha', 10), ('beta', 20), ('gamma', 30)",
            )
            .unwrap();

        // Returns comma-separated IDs
        let ids_str = match result {
            SqlResult::Ok(ids) => ids,
            _ => panic!("expected Ok with ids"),
        };
        let ids: Vec<&str> = ids_str.split(',').collect();
        assert_eq!(ids.len(), 3, "should return 3 IDs");

        // All IDs are distinct 14-digit timestamps
        for id in &ids {
            assert_eq!(id.len(), 14, "ID should be 14 digits: {id}");
            assert!(
                id.chars().all(|c| c.is_ascii_digit()),
                "ID should be numeric: {id}"
            );
        }
        let unique: std::collections::HashSet<&&str> = ids.iter().collect();
        assert_eq!(unique.len(), 3, "all IDs should be unique");

        // 3 rows in materialized table
        let rows = index
            .query_raw("SELECT name, score FROM items ORDER BY name")
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0][0], "alpha");
        assert_eq!(rows[0][1], "10");
        assert_eq!(rows[1][0], "beta");
        assert_eq!(rows[1][1], "20");
        assert_eq!(rows[2][0], "gamma");
        assert_eq!(rows[2][1], "30");

        // 3 zettels in index
        let count = index
            .query_raw("SELECT COUNT(*) FROM zettels WHERE type = 'items'")
            .unwrap();
        assert_eq!(count[0][0], "3");
    }

    #[test]
    fn insert_multi_row_single_commit() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine.execute("CREATE TABLE things (label TEXT)").unwrap();

        let head_before = repo.head_oid().unwrap();

        engine
            .execute("INSERT INTO things (label) VALUES ('a'), ('b'), ('c')")
            .unwrap();

        let head_after = repo.head_oid().unwrap();
        // Head moved (commit happened)
        assert_ne!(head_before.0, head_after.0);

        // The single commit contains all 3 files
        let diff = repo.diff_paths(&head_before.0, &head_after.0).unwrap();
        assert_eq!(diff.len(), 3, "single commit should contain 3 new files");
    }

    #[test]
    fn select_returns_materialized_data() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE items (name TEXT, count INTEGER)")
            .unwrap();
        engine
            .execute("INSERT INTO items (name, count) VALUES ('Widget', 42)")
            .unwrap();

        let result = engine.execute("SELECT name, count FROM items").unwrap();
        match result {
            SqlResult::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], "Widget");
                assert_eq!(rows[0][1], "42");
            }
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn update_modifies_zettel_and_materialized_row() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE projects (name TEXT, priority INTEGER)")
            .unwrap();
        let id = match engine
            .execute("INSERT INTO projects (name, priority) VALUES ('Alpha', 1)")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        engine
            .execute(&format!(
                "UPDATE projects SET priority = 5 WHERE id = '{id}'"
            ))
            .unwrap();

        // Check materialized table
        let rows = index.query_raw("SELECT priority FROM projects").unwrap();
        assert_eq!(rows[0][0], "5");

        // Check zettel file (resolve via index since typed → subfolder)
        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(content.contains("priority: 5"));
    }

    #[test]
    fn delete_removes_zettel_and_materialized_row() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine.execute("CREATE TABLE projects (name TEXT)").unwrap();
        let id = match engine
            .execute("INSERT INTO projects (name) VALUES ('Alpha')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        engine
            .execute(&format!("DELETE FROM projects WHERE id = '{id}'"))
            .unwrap();

        // Materialized table should be empty
        let rows = index.query_raw("SELECT COUNT(*) FROM projects").unwrap();
        assert_eq!(rows[0][0], "0");

        // Zettel should be gone from index
        let rows = index
            .query_raw(&format!("SELECT COUNT(*) FROM zettels WHERE id = '{id}'"))
            .unwrap();
        assert_eq!(rows[0][0], "0");

        // File should be gone from Git
        let result = repo.read_file(&format!("zettelkasten/projects/{id}.md"));
        assert!(result.is_err());
    }

    #[test]
    fn full_create_insert_select_update_delete_cycle() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        // CREATE
        engine
            .execute("CREATE TABLE tasks (name TEXT, status TEXT, priority INTEGER)")
            .unwrap();

        // INSERT
        let id = match engine
            .execute(
                "INSERT INTO tasks (name, status, priority) VALUES ('Build feature', 'todo', 3)",
            )
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        // SELECT
        let result = engine
            .execute("SELECT name, status, priority FROM tasks")
            .unwrap();
        match result {
            SqlResult::Rows { rows, .. } => {
                assert_eq!(rows[0][0], "Build feature");
                assert_eq!(rows[0][1], "todo");
                assert_eq!(rows[0][2], "3");
            }
            _ => panic!("expected Rows"),
        }

        // UPDATE
        engine
            .execute(&format!(
                "UPDATE tasks SET status = 'done', priority = 1 WHERE id = '{id}'"
            ))
            .unwrap();

        let result = engine
            .execute("SELECT status, priority FROM tasks")
            .unwrap();
        match result {
            SqlResult::Rows { rows, .. } => {
                assert_eq!(rows[0][0], "done");
                assert_eq!(rows[0][1], "1");
            }
            _ => panic!("expected Rows"),
        }

        // DELETE
        engine
            .execute(&format!("DELETE FROM tasks WHERE id = '{id}'"))
            .unwrap();
        let result = engine.execute("SELECT COUNT(*) FROM tasks").unwrap();
        match result {
            SqlResult::Rows { rows, .. } => assert_eq!(rows[0][0], "0"),
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn insert_with_fk_validates_reference() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine.execute("CREATE TABLE people (name TEXT)").unwrap();
        engine
            .execute("CREATE TABLE tasks (name TEXT, assignee TEXT REFERENCES people(id))")
            .unwrap();

        // Insert with non-existent reference should fail
        let err = engine
            .execute("INSERT INTO tasks (name, assignee) VALUES ('Fix bug', '99999999999999')")
            .unwrap_err();
        assert!(format!("{err}").contains("referenced zettel not found"));
    }

    #[test]
    fn insert_produces_correct_zone_mapping() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE projects (name TEXT, status TEXT, priority INTEGER)")
            .unwrap();

        let id = match engine
            .execute("INSERT INTO projects (name, status, priority) VALUES ('Alpha', 'active', 1)")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();

        // priority (INTEGER) → frontmatter
        assert!(content.contains("priority: 1"));
        // name (TEXT) → body section
        assert!(content.contains("## name\n\nAlpha"));
        // status (TEXT) → body section
        assert!(content.contains("## status\n\nactive"));
        // type should be table name
        assert!(content.contains("type: projects"));
        // title should be first TEXT column value
        assert!(content.contains("title: Alpha"));
    }

    #[test]
    fn typed_zettel_stored_in_subfolder_and_crud_works() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine.execute("CREATE TABLE docs (name TEXT)").unwrap();

        // Add folder: true to the typedef
        let typedef_rows = index
            .query_raw("SELECT id, path FROM zettels WHERE type = '_typedef' AND title = 'docs'")
            .unwrap();
        let typedef_path = &typedef_rows[0][1];
        let typedef_content = repo.read_file(typedef_path).unwrap();
        let updated = typedef_content.replace("type: _typedef", "type: _typedef\nfolder: true");
        repo.commit_file(typedef_path, &updated, "add folder to docs typedef")
            .unwrap();
        let parsed = crate::parser::parse(&updated, typedef_path).unwrap();
        index.index_zettel(&parsed).unwrap();
        // Recreate engine to pick up updated typedef
        let mut engine = SqlEngine::new(&index, &repo);

        // INSERT → should go to zettelkasten/docs/{id}.md (folder: true)
        let id = match engine
            .execute("INSERT INTO docs (name) VALUES ('Guide')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };
        let path = index.resolve_path(&id).unwrap();
        assert!(
            path.starts_with("zettelkasten/docs/"),
            "path should be in type subfolder: {path}"
        );

        // UPDATE via SQL → should find it in subfolder
        engine
            .execute(&format!(
                "UPDATE docs SET name = 'Manual' WHERE id = '{id}'"
            ))
            .unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(content.contains("Manual"));

        // DELETE via SQL → should remove from subfolder
        engine
            .execute(&format!("DELETE FROM docs WHERE id = '{id}'"))
            .unwrap();
        assert!(repo.read_file(&path).is_err());
    }

    #[test]
    fn insert_fills_default_value() {
        let (_dir, repo, index) = setup();

        // Manually create typedef with allowed_values + default_value
        let typedef = "---\nid: 20260301110000\ntitle: task\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n    allowed_values:\n      - todo\n      - doing\n      - done\n    default_value: todo\n  - name: name\n    data_type: TEXT\n    zone: frontmatter\n---\n";
        let typedef_path = "zettelkasten/_typedef/20260301110000.md";
        repo.commit_file(typedef_path, typedef, "add typedef")
            .unwrap();
        let parsed = crate::parser::parse(typedef, typedef_path).unwrap();
        index.index_zettel(&parsed).unwrap();
        index.materialize_all_types(&repo).unwrap();

        let mut engine = SqlEngine::new(&index, &repo);

        // INSERT omitting status → should get default "todo"
        let id = match engine
            .execute("INSERT INTO task (name) VALUES ('Write tests')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };
        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(
            content.contains("status: todo"),
            "expected default status in:\n{content}"
        );
    }

    #[test]
    fn insert_rejects_invalid_allowed_value() {
        let (_dir, repo, index) = setup();

        let typedef = "---\nid: 20260301110100\ntitle: task2\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n    allowed_values:\n      - todo\n      - doing\n      - done\n  - name: name\n    data_type: TEXT\n    zone: frontmatter\n---\n";
        let typedef_path = "zettelkasten/_typedef/20260301110100.md";
        repo.commit_file(typedef_path, typedef, "add typedef")
            .unwrap();
        let parsed = crate::parser::parse(typedef, typedef_path).unwrap();
        index.index_zettel(&parsed).unwrap();
        index.materialize_all_types(&repo).unwrap();

        let mut engine = SqlEngine::new(&index, &repo);

        // INSERT with invalid value → should error
        let result = engine.execute("INSERT INTO task2 (name, status) VALUES ('Test', 'invalid')");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not in allowed values"),
            "expected validation error: {err}"
        );
    }

    #[test]
    fn update_rejects_invalid_allowed_value() {
        let (_dir, repo, index) = setup();

        let typedef = "---\nid: 20260301110200\ntitle: task3\ntype: _typedef\ncolumns:\n  - name: status\n    data_type: TEXT\n    zone: frontmatter\n    allowed_values:\n      - todo\n      - doing\n      - done\n    default_value: todo\n  - name: name\n    data_type: TEXT\n    zone: frontmatter\n---\n";
        let typedef_path = "zettelkasten/_typedef/20260301110200.md";
        repo.commit_file(typedef_path, typedef, "add typedef")
            .unwrap();
        let parsed = crate::parser::parse(typedef, typedef_path).unwrap();
        index.index_zettel(&parsed).unwrap();
        index.materialize_all_types(&repo).unwrap();

        let mut engine = SqlEngine::new(&index, &repo);

        // INSERT valid
        let id = match engine
            .execute("INSERT INTO task3 (name, status) VALUES ('Test', 'todo')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        // UPDATE with invalid value
        let result = engine.execute(&format!(
            "UPDATE task3 SET status = 'bad' WHERE id = '{id}'"
        ));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not in allowed values"),
            "expected validation error: {err}"
        );
    }

    #[test]
    fn drop_table_cascade_deletes_all() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine.execute("CREATE TABLE dropme (name TEXT)").unwrap();
        engine
            .execute("INSERT INTO dropme (name) VALUES ('a')")
            .unwrap();
        engine
            .execute("INSERT INTO dropme (name) VALUES ('b')")
            .unwrap();

        engine.execute("DROP TABLE dropme CASCADE").unwrap();

        // Typedef gone
        let rows = index
            .query_raw("SELECT id FROM zettels WHERE type = '_typedef' AND title = 'dropme'")
            .unwrap();
        assert!(rows.is_empty());

        // Data zettels gone
        let rows = index
            .query_raw("SELECT id FROM zettels WHERE type = 'dropme'")
            .unwrap();
        assert!(rows.is_empty());

        // Materialized table gone
        let result = index.query_raw("SELECT * FROM dropme");
        assert!(result.is_err());
    }

    #[test]
    fn drop_table_strips_type_from_data_zettels() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine.execute("CREATE TABLE stripme (name TEXT)").unwrap();
        let id = match engine
            .execute("INSERT INTO stripme (name) VALUES ('keep')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        engine.execute("DROP TABLE stripme").unwrap();

        // Typedef gone
        let rows = index
            .query_raw("SELECT id FROM zettels WHERE type = '_typedef' AND title = 'stripme'")
            .unwrap();
        assert!(rows.is_empty());

        // Data zettel still exists but type is cleared
        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(!content.contains("type: stripme"));
    }

    #[test]
    fn drop_table_removes_typedef_and_materialized() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE removeme (status TEXT)")
            .unwrap();
        engine
            .execute("INSERT INTO removeme (status) VALUES ('x')")
            .unwrap();

        // Materialized table exists before drop
        assert!(index.query_raw("SELECT * FROM removeme").is_ok());

        engine.execute("DROP TABLE removeme").unwrap();

        // Typedef removed from index
        let rows = index
            .query_raw("SELECT id FROM zettels WHERE type = '_typedef' AND title = 'removeme'")
            .unwrap();
        assert!(rows.is_empty(), "typedef should be removed");

        // Materialized table dropped
        assert!(
            index.query_raw("SELECT * FROM removeme").is_err(),
            "materialized table should be dropped"
        );
    }

    #[test]
    fn drop_table_if_exists_no_error() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        let result = engine.execute("DROP TABLE IF EXISTS nonexistent");
        assert!(result.is_ok());
    }

    #[test]
    fn drop_table_rejects_non_table() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        let result = engine.execute("DROP VIEW something");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not supported"));
    }

    #[test]
    fn alter_table_add_column_extends_schema() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine.execute("CREATE TABLE addcol (name TEXT)").unwrap();
        engine
            .execute("ALTER TABLE addcol ADD COLUMN priority INTEGER")
            .unwrap();

        // Verify column exists in materialized table
        let result = engine.execute("SELECT * FROM addcol").unwrap();
        match result {
            SqlResult::Rows { columns, .. } => {
                assert!(columns.contains(&"priority".to_string()));
            }
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn alter_table_add_column_infers_zone_and_allowed_values() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE altadd (name VARCHAR(100))")
            .unwrap();
        engine
            .execute(
                "ALTER TABLE altadd ADD COLUMN status ENUM('todo','doing','done') DEFAULT 'todo'",
            )
            .unwrap();
        engine
            .execute("ALTER TABLE altadd ADD COLUMN notes TEXT")
            .unwrap();

        let schema = engine.load_schema("altadd").unwrap();

        let status = schema.columns.iter().find(|c| c.name == "status").unwrap();
        assert_eq!(status.zone, Some(Zone::Frontmatter));
        assert_eq!(
            status.allowed_values,
            Some(vec!["todo".into(), "doing".into(), "done".into()])
        );
        assert_eq!(status.default_value.as_deref(), Some("todo"));

        let notes = schema.columns.iter().find(|c| c.name == "notes").unwrap();
        assert_eq!(notes.zone, Some(Zone::Body));
    }

    #[test]
    fn alter_table_add_column_existing_data_gets_null() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine.execute("CREATE TABLE addcol2 (name TEXT)").unwrap();
        engine
            .execute("INSERT INTO addcol2 (name) VALUES ('test')")
            .unwrap();
        engine
            .execute("ALTER TABLE addcol2 ADD COLUMN score INTEGER")
            .unwrap();

        let result = engine.execute("SELECT name, score FROM addcol2").unwrap();
        match result {
            SqlResult::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], "test");
                assert_eq!(rows[0][1], "NULL"); // NULL column
            }
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn alter_table_drop_column_removes_from_schema() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE dropcol (name TEXT, extra TEXT)")
            .unwrap();
        engine
            .execute("ALTER TABLE dropcol DROP COLUMN extra")
            .unwrap();

        let result = engine.execute("SELECT * FROM dropcol").unwrap();
        match result {
            SqlResult::Rows { columns, .. } => {
                assert!(!columns.contains(&"extra".to_string()));
                assert!(columns.contains(&"name".to_string()));
            }
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn bulk_delete_removes_matching_rows() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE bulkdel (name TEXT, status TEXT)")
            .unwrap();
        engine
            .execute("INSERT INTO bulkdel (name, status) VALUES ('a', 'done')")
            .unwrap();
        engine
            .execute("INSERT INTO bulkdel (name, status) VALUES ('b', 'todo')")
            .unwrap();
        engine
            .execute("INSERT INTO bulkdel (name, status) VALUES ('c', 'done')")
            .unwrap();

        let result = engine
            .execute("DELETE FROM bulkdel WHERE status = 'done'")
            .unwrap();
        match result {
            SqlResult::Affected(n) => assert_eq!(n, 2),
            _ => panic!("expected Affected"),
        }

        let rows = engine.execute("SELECT name FROM bulkdel").unwrap();
        match rows {
            SqlResult::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], "b");
            }
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn bulk_delete_all_rows_when_no_where() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine.execute("CREATE TABLE bulkdel2 (name TEXT)").unwrap();
        engine
            .execute("INSERT INTO bulkdel2 (name) VALUES ('a')")
            .unwrap();
        engine
            .execute("INSERT INTO bulkdel2 (name) VALUES ('b')")
            .unwrap();

        let result = engine.execute("DELETE FROM bulkdel2").unwrap();
        match result {
            SqlResult::Affected(n) => assert_eq!(n, 2),
            _ => panic!("expected Affected"),
        }

        let rows = engine.execute("SELECT * FROM bulkdel2").unwrap();
        match rows {
            SqlResult::Rows { rows, .. } => assert!(rows.is_empty()),
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn bulk_update_modifies_matching_rows() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE bulkupd (name TEXT, priority INTEGER)")
            .unwrap();
        engine
            .execute("INSERT INTO bulkupd (name, priority) VALUES ('a', 1)")
            .unwrap();
        engine
            .execute("INSERT INTO bulkupd (name, priority) VALUES ('b', 2)")
            .unwrap();
        engine
            .execute("INSERT INTO bulkupd (name, priority) VALUES ('c', 1)")
            .unwrap();

        let result = engine
            .execute("UPDATE bulkupd SET priority = 9 WHERE priority = 1")
            .unwrap();
        match result {
            SqlResult::Affected(n) => assert_eq!(n, 2),
            _ => panic!("expected Affected"),
        }

        let rows = engine
            .execute("SELECT name, priority FROM bulkupd ORDER BY name")
            .unwrap();
        match rows {
            SqlResult::Rows { rows, .. } => {
                assert_eq!(rows[0][1], "9"); // a: was 1 → 9
                assert_eq!(rows[1][1], "2"); // b: unchanged
                assert_eq!(rows[2][1], "9"); // c: was 1 → 9
            }
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn bulk_update_all_rows_when_no_where() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE bulkupd2 (name TEXT, flag TEXT)")
            .unwrap();
        engine
            .execute("INSERT INTO bulkupd2 (name, flag) VALUES ('a', 'old')")
            .unwrap();
        engine
            .execute("INSERT INTO bulkupd2 (name, flag) VALUES ('b', 'old')")
            .unwrap();

        let result = engine.execute("UPDATE bulkupd2 SET flag = 'new'").unwrap();
        match result {
            SqlResult::Affected(n) => assert_eq!(n, 2),
            _ => panic!("expected Affected"),
        }

        let rows = engine.execute("SELECT flag FROM bulkupd2").unwrap();
        match rows {
            SqlResult::Rows { rows, .. } => {
                assert!(rows.iter().all(|r| r[0] == "new"));
            }
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn alter_table_rename_column_rewrites_frontmatter() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE renamefm (status TEXT, priority INTEGER)")
            .unwrap();
        let id = match engine
            .execute("INSERT INTO renamefm (status, priority) VALUES ('active', 5)")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        engine
            .execute("ALTER TABLE renamefm RENAME COLUMN priority TO importance")
            .unwrap();

        // Verify zettel file has renamed key
        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(
            content.contains("importance: 5"),
            "expected renamed key in frontmatter: {content}"
        );
        assert!(
            !content.contains("priority:"),
            "old key should be gone: {content}"
        );

        // Verify materialized table has renamed column
        let result = engine.execute("SELECT importance FROM renamefm").unwrap();
        match result {
            SqlResult::Rows { rows, .. } => {
                assert_eq!(rows[0][0], "5");
            }
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn alter_table_rename_column_rewrites_body_heading() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        // Body zone column (TEXT, first column = body zone by default)
        engine
            .execute("CREATE TABLE renamebody (description TEXT)")
            .unwrap();
        let id = match engine
            .execute("INSERT INTO renamebody (description) VALUES ('hello world')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        engine
            .execute("ALTER TABLE renamebody RENAME COLUMN description TO summary")
            .unwrap();

        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(
            content.contains("## summary"),
            "expected renamed heading: {content}"
        );
        assert!(
            !content.contains("## description"),
            "old heading should be gone: {content}"
        );
    }

    #[test]
    fn alter_table_rename_column_rewrites_reference() {
        let (_dir, repo, index) = setup();

        // Create referenced type first
        engine_exec_ok(&repo, &index, "CREATE TABLE person (name TEXT)");
        let person_id = engine_exec_id(&repo, &index, "INSERT INTO person (name) VALUES ('Alice')");

        // Create type with reference column and insert with the person's zettel id
        engine_exec_ok(
            &repo,
            &index,
            "CREATE TABLE task (title TEXT, assignee TEXT REFERENCES person)",
        );
        let id = engine_exec_id(
            &repo,
            &index,
            &format!("INSERT INTO task (title, assignee) VALUES ('Fix bug', '{person_id}')"),
        );

        let mut engine = SqlEngine::new(&index, &repo);
        engine
            .execute("ALTER TABLE task RENAME COLUMN assignee TO owner")
            .unwrap();

        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(
            content.contains("- owner::"),
            "expected renamed reference key: {content}"
        );
        assert!(
            !content.contains("- assignee::"),
            "old reference key should be gone: {content}"
        );
    }

    #[test]
    fn alter_table_rename_column_rejects_collision() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        engine
            .execute("CREATE TABLE coltest (name TEXT, status TEXT)")
            .unwrap();

        let err = engine
            .execute("ALTER TABLE coltest RENAME COLUMN name TO status")
            .unwrap_err();
        assert!(
            err.to_string().contains("column already exists: status"),
            "{err}"
        );
    }

    /// Count git commits by walking the HEAD log.
    fn count_commits(repo: &GitRepo) -> usize {
        let git = git2::Repository::open(&repo.path).unwrap();
        let mut revwalk = git.revwalk().unwrap();
        revwalk.push_head().unwrap();
        revwalk.count()
    }

    #[test]
    fn begin_commit_batches_writes() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        engine.execute("CREATE TABLE items (name TEXT)").unwrap();
        let before = count_commits(&repo);

        engine.execute("BEGIN").unwrap();
        engine
            .execute("INSERT INTO items (name) VALUES ('a')")
            .unwrap();
        engine
            .execute("INSERT INTO items (name) VALUES ('b')")
            .unwrap();
        engine.execute("COMMIT").unwrap();

        let after = count_commits(&repo);
        // Should produce exactly one additional git commit for the transaction
        assert_eq!(
            after - before,
            1,
            "expected single git commit for transaction"
        );

        let rows = index
            .query_raw("SELECT name FROM items ORDER BY name")
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], "a");
        assert_eq!(rows[1][0], "b");
    }

    #[test]
    fn begin_rollback_discards() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        engine.execute("CREATE TABLE items (name TEXT)").unwrap();
        let before = count_commits(&repo);

        engine.execute("BEGIN").unwrap();
        engine
            .execute("INSERT INTO items (name) VALUES ('gone')")
            .unwrap();
        engine.execute("ROLLBACK").unwrap();

        let after = count_commits(&repo);
        assert_eq!(after, before, "rollback should not produce git commits");

        let rows = index.query_raw("SELECT name FROM items").unwrap();
        assert!(rows.is_empty(), "rollback should discard inserts");
    }

    #[test]
    fn read_your_writes_within_txn() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        engine.execute("CREATE TABLE items (name TEXT)").unwrap();

        engine.execute("BEGIN").unwrap();
        engine
            .execute("INSERT INTO items (name) VALUES ('visible')")
            .unwrap();

        // SELECT within the same transaction should see the inserted row
        let result = engine.execute("SELECT name FROM items").unwrap();
        match result {
            SqlResult::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], "visible");
            }
            _ => panic!("expected Rows"),
        }

        engine.execute("COMMIT").unwrap();
    }

    #[test]
    fn drop_auto_rollback() {
        let (_dir, repo, index) = setup();
        {
            let mut engine = SqlEngine::new(&index, &repo);
            engine.execute("CREATE TABLE items (name TEXT)").unwrap();
            engine.execute("BEGIN").unwrap();
            engine
                .execute("INSERT INTO items (name) VALUES ('orphan')")
                .unwrap();
            // engine dropped here without COMMIT
        }

        // After drop, SQLite savepoint should be rolled back
        let rows = index.query_raw("SELECT name FROM items").unwrap();
        assert!(rows.is_empty(), "drop should auto-rollback");
    }

    #[test]
    fn nested_begin_rejected() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine.execute("BEGIN").unwrap();
        let err = engine.execute("BEGIN").unwrap_err();
        assert!(err.to_string().contains("already active"), "{err}");
        engine.execute("ROLLBACK").unwrap();
    }

    #[test]
    fn insert_then_update_within_txn() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        engine.execute("CREATE TABLE items (name TEXT)").unwrap();

        engine.execute("BEGIN").unwrap();
        let id = match engine
            .execute("INSERT INTO items (name) VALUES ('old')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };
        engine
            .execute(&format!("UPDATE items SET name = 'new' WHERE id = '{id}'"))
            .unwrap();
        engine.execute("COMMIT").unwrap();

        let rows = index.query_raw("SELECT name FROM items").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "new");

        // Verify git also has the updated content
        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(content.contains("new"), "git should have updated content");
    }

    #[test]
    fn insert_then_delete_within_txn() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        engine.execute("CREATE TABLE items (name TEXT)").unwrap();

        engine.execute("BEGIN").unwrap();
        let id = match engine
            .execute("INSERT INTO items (name) VALUES ('temp')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };
        engine
            .execute(&format!("DELETE FROM items WHERE id = '{id}'"))
            .unwrap();
        engine.execute("COMMIT").unwrap();

        let rows = index.query_raw("SELECT name FROM items").unwrap();
        assert!(rows.is_empty(), "insert+delete should cancel out");
    }

    #[test]
    fn error_preserves_active_txn() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        engine.execute("CREATE TABLE items (name TEXT)").unwrap();

        engine.execute("BEGIN").unwrap();
        engine
            .execute("INSERT INTO items (name) VALUES ('keep')")
            .unwrap();

        // Trigger an error (insert into nonexistent table)
        let err = engine.execute("INSERT INTO nonexistent (name) VALUES ('fail')");
        assert!(err.is_err());

        // Transaction should still be active — can still ROLLBACK
        engine.execute("ROLLBACK").unwrap();

        let rows = index.query_raw("SELECT name FROM items").unwrap();
        assert!(rows.is_empty(), "rollback after error should discard all");
    }

    #[test]
    fn insert_delete_read_content_returns_not_found() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        engine.execute("CREATE TABLE items (name TEXT)").unwrap();

        engine.execute("BEGIN").unwrap();
        let id = match engine
            .execute("INSERT INTO items (name) VALUES ('ghost')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        // Delete within same txn
        engine
            .execute(&format!("DELETE FROM items WHERE id = '{id}'"))
            .unwrap();

        // SELECT should return no rows (SQLite already removed)
        let result = engine.execute("SELECT name FROM items").unwrap();
        match result {
            SqlResult::Rows { rows, .. } => {
                assert!(rows.is_empty(), "deleted row should not appear in SELECT")
            }
            _ => panic!("expected Rows"),
        }

        engine.execute("COMMIT").unwrap();

        // Git should have no commit for cancelled write+delete
        let rows = index.query_raw("SELECT name FROM items").unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn two_inserts_one_deleted_commits_survivor() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        engine.execute("CREATE TABLE items (name TEXT)").unwrap();

        engine.execute("BEGIN").unwrap();

        let id1 = match engine
            .execute("INSERT INTO items (name) VALUES ('keep')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };
        std::thread::sleep(std::time::Duration::from_secs(1));
        let id2 = match engine
            .execute("INSERT INTO items (name) VALUES ('remove')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        engine
            .execute(&format!("DELETE FROM items WHERE id = '{id2}'"))
            .unwrap();
        engine.execute("COMMIT").unwrap();

        // Only first insert should survive
        let rows = index.query_raw("SELECT name FROM items").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "keep");

        // Verify git file exists for survivor (no folder: true → flat path)
        assert!(repo.read_file(&format!("zettelkasten/{id1}.md")).is_ok());
        // Deleted zettel should not be in git (it was buffer-only)
        assert!(repo.read_file(&format!("zettelkasten/{id2}.md")).is_err());
    }

    #[test]
    fn create_index_rejected_with_reason() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        let err = engine
            .execute("CREATE INDEX idx ON zettels(title)")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("CREATE INDEX not supported"), "{msg}");
    }

    #[test]
    fn create_view_rejected_with_reason() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        let err = engine
            .execute("CREATE VIEW v AS SELECT * FROM zettels")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("CREATE VIEW not supported"), "{msg}");
    }

    #[test]
    fn create_trigger_rejected_with_reason() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        let err = engine
            .execute(
                "CREATE TRIGGER t AFTER INSERT ON zettels FOR EACH ROW EXECUTE PROCEDURE noop()",
            )
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("CREATE TRIGGER not supported"), "{msg}");
    }

    #[test]
    fn create_virtual_table_rejected_with_reason() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        let err = engine
            .execute("CREATE VIRTUAL TABLE vt USING fts5(content)")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("CREATE VIRTUAL TABLE not supported"), "{msg}");
    }

    #[test]
    fn drop_index_rejected() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        let err = engine.execute("DROP INDEX idx").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("DROP INDEX not supported"), "{msg}");
    }

    #[test]
    fn drop_view_rejected() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        let err = engine.execute("DROP VIEW v").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("DROP VIEW not supported"), "{msg}");
    }

    #[test]
    fn insert_or_replace_rejected() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        engine.execute("CREATE TABLE items (name TEXT)").unwrap();
        let err = engine
            .execute("INSERT OR REPLACE INTO items (name) VALUES ('x')")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not supported"), "{msg}");
    }

    #[test]
    fn update_from_rejected() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        engine.execute("CREATE TABLE items (name TEXT)").unwrap();
        engine.execute("CREATE TABLE src (name TEXT)").unwrap();
        let err = engine
            .execute("UPDATE items SET name = src.name FROM src WHERE items.id = src.id")
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("UPDATE...FROM not supported"), "{msg}");
    }

    #[test]
    fn delete_from_hyphenated_table() {
        let (_dir, repo, index) = setup();
        engine_exec_ok(&repo, &index, "CREATE TABLE \"my-items\" (name TEXT)");
        let id = engine_exec_id(
            &repo,
            &index,
            r#"INSERT INTO "my-items" (name) VALUES ('test')"#,
        );
        let mut engine = SqlEngine::new(&index, &repo);
        let result = engine
            .execute(&format!(r#"DELETE FROM "my-items" WHERE id = '{id}'"#))
            .unwrap();
        match result {
            SqlResult::Affected(n) => assert_eq!(n, 1),
            _ => panic!("expected Affected"),
        }
    }

    #[test]
    fn references_to_hyphenated_table() {
        let (_dir, repo, index) = setup();
        engine_exec_ok(&repo, &index, "CREATE TABLE \"my-people\" (name TEXT)");
        engine_exec_ok(
            &repo,
            &index,
            r#"CREATE TABLE tasks (title TEXT, assignee TEXT REFERENCES "my-people")"#,
        );
        // Verify the typedef stored unquoted reference target
        let mut engine = SqlEngine::new(&index, &repo);
        let schema = engine.load_schema("tasks").unwrap();
        let ref_col = schema
            .columns
            .iter()
            .find(|c| c.name == "assignee")
            .expect("assignee column");
        assert_eq!(
            ref_col.references.as_deref(),
            Some("my-people"),
            "reference target should be unquoted"
        );
    }

    #[test]
    fn select_still_passes_through() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);
        let result = engine.execute("SELECT 1 AS val").unwrap();
        match result {
            SqlResult::Rows { columns, rows } => {
                assert_eq!(columns, vec!["val"]);
                assert_eq!(rows.len(), 1);
            }
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn schema_roundtrips_title_template_and_origin() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE widgets (name VARCHAR(100), weight REAL)")
            .unwrap();

        // Load the typedef, patch in title_template and origin, rewrite, reload
        let (_td_id, td_path) = engine.load_typedef_location("widgets").unwrap();
        let content = repo.read_file(&td_path).unwrap();
        let mut parsed = parser::parse(&content, &td_path).unwrap();
        parsed.meta.extra.insert(
            "title_template".to_string(),
            Value::String("name-widget".into()),
        );
        parsed.meta.extra.insert(
            "origin".to_string(),
            Value::String("prd-00030".into()),
        );
        let new_content = parser::serialize(&parsed);
        repo.commit_file(&td_path, &new_content, "add title_template and origin")
            .unwrap();
        let reparsed = parser::parse(&new_content, &td_path).unwrap();
        index.index_zettel(&reparsed).unwrap();

        let schema = engine.load_schema("widgets").unwrap();
        assert_eq!(schema.title_template.as_deref(), Some("name-widget"));
        assert_eq!(schema.origin.as_deref(), Some("prd-00030"));
    }

    #[test]
    fn zone_inference_by_sql_type() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute(
                "CREATE TABLE items (\
                 short_name VARCHAR(100), \
                 description TEXT, \
                 priority INTEGER, \
                 active BOOLEAN, \
                 score REAL, \
                 bio MEDIUMTEXT\
                 )",
            )
            .unwrap();

        let schema = engine.load_schema("items").unwrap();
        let zone_of = |name: &str| -> Zone {
            schema
                .columns
                .iter()
                .find(|c| c.name == name)
                .unwrap()
                .zone
                .clone()
                .unwrap()
        };

        assert_eq!(zone_of("short_name"), Zone::Frontmatter); // VARCHAR(100) → frontmatter
        assert_eq!(zone_of("description"), Zone::Body); // TEXT → body
        assert_eq!(zone_of("priority"), Zone::Frontmatter); // INTEGER → frontmatter
        assert_eq!(zone_of("active"), Zone::Frontmatter); // BOOLEAN → frontmatter
        assert_eq!(zone_of("score"), Zone::Frontmatter); // REAL → frontmatter
        assert_eq!(zone_of("bio"), Zone::Body); // MEDIUMTEXT → body
    }

    #[test]
    fn varchar_255_boundary() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE boundary (short VARCHAR(255), long VARCHAR(256))")
            .unwrap();

        let schema = engine.load_schema("boundary").unwrap();
        let short = schema.columns.iter().find(|c| c.name == "short").unwrap();
        let long = schema.columns.iter().find(|c| c.name == "long").unwrap();

        assert_eq!(short.zone, Some(Zone::Frontmatter));
        assert_eq!(short.data_type, "VARCHAR(255)");
        assert_eq!(long.zone, Some(Zone::Body));
        assert_eq!(long.data_type, "VARCHAR(256)");
    }

    #[test]
    fn char_types_frontmatter() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE chars (code CHAR(10), tiny TINYTEXT)")
            .unwrap();

        let schema = engine.load_schema("chars").unwrap();
        let code = schema.columns.iter().find(|c| c.name == "code").unwrap();
        let tiny = schema.columns.iter().find(|c| c.name == "tiny").unwrap();

        assert_eq!(code.zone, Some(Zone::Frontmatter));
        assert_eq!(code.data_type, "CHAR(10)");
        assert_eq!(tiny.zone, Some(Zone::Frontmatter));
        assert_eq!(tiny.data_type, "TINYTEXT");
    }

    #[test]
    fn enum_creates_allowed_values() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute(
                "CREATE TABLE tasks (summary VARCHAR(200), status ENUM('todo','doing','done') DEFAULT 'todo')",
            )
            .unwrap();

        let schema = engine.load_schema("tasks").unwrap();
        let status = schema.columns.iter().find(|c| c.name == "status").unwrap();

        assert_eq!(status.zone, Some(Zone::Frontmatter));
        assert_eq!(
            status.allowed_values,
            Some(vec!["todo".into(), "doing".into(), "done".into()])
        );
        assert_eq!(status.default_value.as_deref(), Some("todo"));
    }

    #[test]
    fn set_creates_allowed_values() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE prefs (tags SET('x','y','z'))")
            .unwrap();

        let schema = engine.load_schema("prefs").unwrap();
        let tags = schema.columns.iter().find(|c| c.name == "tags").unwrap();

        assert_eq!(tags.zone, Some(Zone::Frontmatter));
        assert_eq!(
            tags.allowed_values,
            Some(vec!["x".into(), "y".into(), "z".into()])
        );
    }

    #[test]
    fn blob_types_body() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE binaries (data BLOB, big MEDIUMBLOB, huge LONGBLOB)")
            .unwrap();

        let schema = engine.load_schema("binaries").unwrap();
        for col_name in &["data", "big", "huge"] {
            let col = schema
                .columns
                .iter()
                .find(|c| c.name == *col_name)
                .unwrap_or_else(|| panic!("missing column {col_name}"));
            assert_eq!(
                col.zone,
                Some(Zone::Body),
                "{col_name} should be body zone"
            );
        }
    }

    #[test]
    fn data_type_to_string_preserves_sizes() {
        use sqlparser::ast::{CharacterLength, DataType};

        let cases = vec![
            (DataType::Varchar(Some(CharacterLength::IntegerLength { length: 100, unit: None })), "VARCHAR(100)"),
            (DataType::Varchar(None), "VARCHAR"),
            (DataType::Char(Some(CharacterLength::IntegerLength { length: 1, unit: None })), "CHAR(1)"),
            (DataType::Char(None), "CHAR"),
            (DataType::Text, "TEXT"),
            (DataType::TinyText, "TINYTEXT"),
            (DataType::MediumText, "MEDIUMTEXT"),
            (DataType::LongText, "LONGTEXT"),
            (DataType::Blob(None), "BLOB"),
            (DataType::TinyBlob, "TINYBLOB"),
            (DataType::MediumBlob, "MEDIUMBLOB"),
            (DataType::LongBlob, "LONGBLOB"),
            (DataType::Boolean, "BOOLEAN"),
            (DataType::Integer(None), "INTEGER"),
            (DataType::Real, "REAL"),
        ];

        for (dt, expected) in cases {
            assert_eq!(super::data_type_to_string(&dt), expected, "for {dt:?}");
        }
    }

    #[test]
    fn alter_table_set_zone() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE ztest (description TEXT, priority INTEGER)")
            .unwrap();

        // TEXT defaults to body; change to frontmatter
        engine
            .execute("ALTER TABLE ztest SET ZONE frontmatter FOR description")
            .unwrap();

        let schema = engine.load_schema("ztest").unwrap();
        let desc = schema.columns.iter().find(|c| c.name == "description").unwrap();
        assert_eq!(desc.zone, Some(Zone::Frontmatter));
    }

    #[test]
    fn alter_table_set_zone_invalid_column() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE ztest2 (name TEXT)")
            .unwrap();
        let err = engine
            .execute("ALTER TABLE ztest2 SET ZONE frontmatter FOR nonexistent")
            .unwrap_err();
        assert!(format!("{err}").contains("column not found"));
    }

    #[test]
    fn alter_table_set_zone_to_reference() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE ztest3 (link VARCHAR(100))")
            .unwrap();
        engine
            .execute("ALTER TABLE ztest3 SET ZONE reference FOR link")
            .unwrap();

        let schema = engine.load_schema("ztest3").unwrap();
        let link = schema.columns.iter().find(|c| c.name == "link").unwrap();
        assert_eq!(link.zone, Some(Zone::Reference));
    }

    #[test]
    fn alter_table_set_zone_case_insensitive() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE ztest4 (notes TEXT)")
            .unwrap();
        engine
            .execute("alter table ztest4 set zone FRONTMATTER for notes")
            .unwrap();

        let schema = engine.load_schema("ztest4").unwrap();
        let notes = schema.columns.iter().find(|c| c.name == "notes").unwrap();
        assert_eq!(notes.zone, Some(Zone::Frontmatter));
    }

    #[test]
    fn alter_table_set_zone_rematerializes() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE ztest5 (url VARCHAR(100), priority INTEGER)")
            .unwrap();

        // Insert with url in frontmatter zone (VARCHAR(100) → frontmatter)
        engine
            .execute("INSERT INTO ztest5 (url, priority) VALUES ('https://example.com', 1)")
            .unwrap();

        // Change url to body — this triggers rematerialization
        engine
            .execute("ALTER TABLE ztest5 SET ZONE body FOR url")
            .unwrap();

        // Materialized table should still exist (rematerialize succeeded)
        let result = engine.execute("SELECT COUNT(*) FROM ztest5").unwrap();
        match result {
            SqlResult::Rows { rows, .. } => assert_eq!(rows[0][0], "1"),
            _ => panic!("expected Rows"),
        }
    }

    #[test]
    fn alter_table_title_template() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE tmpl (name VARCHAR(100))")
            .unwrap();

        // SET
        engine
            .execute("ALTER TABLE tmpl SET TITLE TEMPLATE 'name-template'")
            .unwrap();
        let schema = engine.load_schema("tmpl").unwrap();
        assert_eq!(schema.title_template.as_deref(), Some("name-template"));

        // DROP
        engine
            .execute("ALTER TABLE tmpl DROP TITLE TEMPLATE")
            .unwrap();
        let schema = engine.load_schema("tmpl").unwrap();
        assert_eq!(schema.title_template, None);
    }

    #[test]
    fn alter_table_title_template_persists() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE tmpl2 (name VARCHAR(100))")
            .unwrap();
        engine
            .execute("ALTER TABLE tmpl2 SET TITLE TEMPLATE 'my-template'")
            .unwrap();

        // Create a new engine to verify persistence
        let mut engine2 = SqlEngine::new(&index, &repo);
        let schema = engine2.load_schema("tmpl2").unwrap();
        assert_eq!(schema.title_template.as_deref(), Some("my-template"));
    }

    #[test]
    fn alter_table_set_zone_quoted_identifiers() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE \"my-items\" (\"long-desc\" TEXT)")
            .unwrap();
        engine
            .execute("ALTER TABLE \"my-items\" SET ZONE frontmatter FOR \"long-desc\"")
            .unwrap();

        let schema = engine.load_schema("my-items").unwrap();
        let col = schema.columns.iter().find(|c| c.name == "long-desc").unwrap();
        assert_eq!(col.zone, Some(Zone::Frontmatter));
    }

    #[test]
    fn insert_explicit_title_wins() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE contact (name TEXT, role VARCHAR(100))")
            .unwrap();
        let id = match engine
            .execute("INSERT INTO contact (title, name, role) VALUES ('Dr. Alice', 'Alice', 'doctor')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(content.contains("title: Dr. Alice"), "explicit title should win: {content}");
    }

    #[test]
    fn insert_title_from_template() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE person (name VARCHAR(100), role VARCHAR(100))")
            .unwrap();
        engine
            .execute("ALTER TABLE person SET TITLE TEMPLATE 'name-role'")
            .unwrap();
        let id = match engine
            .execute("INSERT INTO person (name, role) VALUES ('Alice', 'engineer')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        // Template doesn't have {placeholders} because of YAML quoting limitation,
        // so it uses the literal template string. Testing non-interpolated template.
        assert!(content.contains("title: name-role"), "template title: {content}");
    }

    #[test]
    fn insert_title_body_fallback() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE article (description TEXT, priority INTEGER)")
            .unwrap();
        let id = match engine
            .execute("INSERT INTO article (description, priority) VALUES ('My Article', 1)")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(content.contains("title: My Article"), "body fallback: {content}");
    }

    #[test]
    fn insert_title_frontmatter_fallback() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        // Table with only frontmatter columns (no body columns)
        engine
            .execute("CREATE TABLE tag (label VARCHAR(50), priority INTEGER)")
            .unwrap();
        let id = match engine
            .execute("INSERT INTO tag (label, priority) VALUES ('important', 1)")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(content.contains("title: important"), "frontmatter string fallback: {content}");
    }

    #[test]
    fn insert_title_fallback_type_id() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        // Table with only numeric columns — no string source for title
        engine
            .execute("CREATE TABLE counter (count INTEGER, active BOOLEAN)")
            .unwrap();
        let id = match engine
            .execute("INSERT INTO counter (count, active) VALUES (42, true)")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        let expected = format!("title: counter {}", id);
        assert!(content.contains(&expected), "type+id fallback: {content}");
    }

    #[test]
    fn insert_explicit_title_overrides_template() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE widget (name VARCHAR(100))")
            .unwrap();
        engine
            .execute("ALTER TABLE widget SET TITLE TEMPLATE 'template-name'")
            .unwrap();
        let id = match engine
            .execute("INSERT INTO widget (title, name) VALUES ('Explicit Title', 'Widget A')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            _ => panic!("expected Ok"),
        };

        let path = index.resolve_path(&id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(content.contains("title: Explicit Title"), "explicit overrides template: {content}");
    }

    #[test]
    fn create_table_stamps_origin_ddl() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE origtest (name VARCHAR(100))")
            .unwrap();
        let schema = engine.load_schema("origtest").unwrap();
        assert_eq!(schema.origin.as_deref(), Some("ddl"));
    }

    #[test]
    fn origin_ddl_persists_in_yaml() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE origpersist (name VARCHAR(100))")
            .unwrap();

        // Read the typedef zettel content directly
        let (_, path) = engine.load_typedef_location("origpersist").unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(content.contains("origin: ddl"), "YAML should contain origin: ddl\n{content}");
    }

    #[test]
    fn origin_preserved_after_alter() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE origalter (name VARCHAR(100), desc TEXT)")
            .unwrap();

        // ALTER TABLE should preserve origin
        engine
            .execute("ALTER TABLE origalter SET ZONE frontmatter FOR desc")
            .unwrap();
        let schema = engine.load_schema("origalter").unwrap();
        assert_eq!(schema.origin.as_deref(), Some("ddl"), "origin should survive ALTER TABLE");
    }

    #[test]
    fn insert_into_junction_writes_through() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        // Create a type with a REFERENCES column
        engine
            .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
            .unwrap();

        // Create a category type
        engine
            .execute("CREATE TABLE category (label VARCHAR(100))")
            .unwrap();

        // Insert a bookmark
        let bm_id = match engine
            .execute("INSERT INTO bookmark (url) VALUES ('https://example.com')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            other => panic!("expected Ok, got {other:?}"),
        };

        // Insert a category
        let cat_id = match engine
            .execute("INSERT INTO category (label) VALUES ('tech')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            other => panic!("expected Ok, got {other:?}"),
        };

        // INSERT into junction table
        let result = engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            ))
            .unwrap();
        assert!(matches!(result, SqlResult::Affected(1)));

        // Verify reference line in zettel
        let path = index.resolve_path(&bm_id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(
            content.contains(&format!("- category:: [[{cat_id}]]")),
            "zettel should contain reference line: {content}"
        );

        // Verify junction table row
        let rows = index
            .query_raw(&format!(
                "SELECT category_id FROM bookmark_category WHERE bookmark_id = '{bm_id}'"
            ))
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], cat_id);
    }

    #[test]
    fn delete_from_junction_writes_through() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
            .unwrap();
        engine
            .execute("CREATE TABLE category (label VARCHAR(100))")
            .unwrap();

        let bm_id = match engine
            .execute("INSERT INTO bookmark (url) VALUES ('https://example.com')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            other => panic!("expected Ok, got {other:?}"),
        };
        let cat_id = match engine
            .execute("INSERT INTO category (label) VALUES ('tech')")
            .unwrap()
        {
            SqlResult::Ok(id) => id,
            other => panic!("expected Ok, got {other:?}"),
        };

        // Add reference via junction INSERT
        engine
            .execute(&format!(
                "INSERT INTO bookmark_category (bookmark_id, category_id) VALUES ('{bm_id}', '{cat_id}')"
            ))
            .unwrap();

        // Verify it's there
        let path = index.resolve_path(&bm_id).unwrap();
        let content = repo.read_file(&path).unwrap();
        assert!(content.contains(&format!("- category:: [[{cat_id}]]")));

        // DELETE from junction
        let result = engine
            .execute(&format!(
                "DELETE FROM bookmark_category WHERE bookmark_id = '{bm_id}' AND category_id = '{cat_id}'"
            ))
            .unwrap();
        assert!(matches!(result, SqlResult::Affected(1)));

        // Verify reference line removed from zettel
        let content = repo.read_file(&path).unwrap();
        assert!(
            !content.contains(&format!("- category:: [[{cat_id}]]")),
            "reference line should be removed: {content}"
        );

        // Verify junction table empty
        let rows = index
            .query_raw(&format!(
                "SELECT category_id FROM bookmark_category WHERE bookmark_id = '{bm_id}'"
            ))
            .unwrap();
        assert_eq!(rows.len(), 0, "junction table should be empty");
    }

    #[test]
    fn drop_table_cascades_junction_tables() {
        let (_dir, repo, index) = setup();
        let mut engine = SqlEngine::new(&index, &repo);

        engine
            .execute("CREATE TABLE bookmark (url TEXT, category TEXT REFERENCES category)")
            .unwrap();
        engine
            .execute("CREATE TABLE category (label VARCHAR(100))")
            .unwrap();

        // Verify junction table exists
        let tables = index
            .query_raw("SELECT name FROM sqlite_master WHERE type='table' AND name='bookmark_category'")
            .unwrap();
        assert_eq!(tables.len(), 1, "junction table should exist before drop");

        // DROP TABLE CASCADE
        engine
            .execute("DROP TABLE bookmark CASCADE")
            .unwrap();

        // Junction table should be gone
        let tables = index
            .query_raw("SELECT name FROM sqlite_master WHERE type='table' AND name='bookmark_category'")
            .unwrap();
        assert_eq!(tables.len(), 0, "junction table should be dropped after cascade");

        // Main table should also be gone
        let tables = index
            .query_raw("SELECT name FROM sqlite_master WHERE type='table' AND name='bookmark'")
            .unwrap();
        assert_eq!(tables.len(), 0, "main table should be dropped");
    }
}
